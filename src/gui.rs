use std::cell::{Cell, OnceCell, RefCell};
use std::ffi::c_void;
use std::process::Command;
use std::ptr::null_mut;

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{AnyThread, DefinedClass, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSAlert, NSAlertFirstButtonReturn, NSAlertStyle, NSApplication, NSApplicationActivationPolicy,
    NSApplicationDelegate, NSBackingStoreType, NSBezelStyle, NSBorderType, NSButton, NSButtonType,
    NSColor, NSControl, NSControlSize, NSControlTextEditingDelegate, NSEvent,
    NSFloatingWindowLevel, NSFocusRingType, NSFont, NSImage, NSImageScaling, NSImageView,
    NSLineBreakMode, NSMenu, NSMenuDelegate, NSMenuItem, NSNormalWindowLevel, NSPopUpButton,
    NSScreen, NSScrollView, NSStatusBar, NSStatusItem, NSTextAlignment, NSTextField,
    NSTextFieldDelegate, NSTextView, NSVariableStatusItemLength, NSView,
    NSVisualEffectBlendingMode, NSVisualEffectMaterial, NSVisualEffectState, NSVisualEffectView,
    NSWindow, NSWindowDelegate, NSWindowStyleMask, NSWindowTitleVisibility,
};
use objc2_foundation::{
    MainThreadMarker, NSArray, NSData, NSNotification, NSObject, NSObjectProtocol, NSPoint, NSRect,
    NSSize, NSString, NSTimer,
};

use crate::clipboard;
use crate::sensitive;
use crate::storage::{self, AppSettings, HistoryEntry, Language, RichHistoryEntry, Store};

const CAPTURE_MAX_BYTES: usize = 256 * 1024;
const CAPTURE_MAX_RICH_BYTES: usize = 10 * 1024 * 1024;
const MIN_PREVIEW_UNITS: usize = 6;
const MENU_WIDTH_TEXT_PADDING: usize = 56;
const MENU_WIDTH_UNIT_PIXELS: usize = 5;
const ELLIPSIS: &str = "...";
const ELLIPSIS_UNITS: usize = 3;
const MENU_SCREEN_MARGIN: f64 = 8.0;
const SEARCH_PANEL_WIDTH: f64 = 660.0;
const SEARCH_PANEL_HEIGHT: f64 = 560.0;
const SEARCH_PANEL_PADDING: f64 = 18.0;
const SEARCH_ROW_HEIGHT: f64 = 42.0;
const SEARCH_MAX_CANDIDATES: usize = 20;
const PREVIEW_PANEL_SIZE: f64 = 360.0;
const PREFERENCES_PANEL_WIDTH: f64 = 700.0;
const PREFERENCES_PANEL_HEIGHT: f64 = 560.0;
const MENU_ITEM_HEIGHT_ESTIMATE: f64 = 22.0;
const MENU_VERTICAL_PADDING_ESTIMATE: f64 = 16.0;

type OSStatus = i32;
type EventTargetRef = *mut c_void;
type EventHandlerCallRef = *mut c_void;
type EventRef = *mut c_void;
type EventHandlerRef = *mut c_void;
type EventHotKeyRef = *mut c_void;

#[repr(C)]
#[derive(Clone, Copy)]
struct EventTypeSpec {
    event_class: u32,
    event_kind: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct EventHotKeyID {
    signature: u32,
    id: u32,
}

type EventHandlerUPP = unsafe extern "C" fn(EventHandlerCallRef, EventRef, *mut c_void) -> OSStatus;

const fn fourcc(bytes: &[u8; 4]) -> u32 {
    ((bytes[0] as u32) << 24)
        | ((bytes[1] as u32) << 16)
        | ((bytes[2] as u32) << 8)
        | bytes[3] as u32
}

const NO_ERR: OSStatus = 0;
const K_EVENT_CLASS_KEYBOARD: u32 = fourcc(b"keyb");
const K_EVENT_HOT_KEY_PRESSED: u32 = 5;
const K_EVENT_HOT_KEY_SIGNATURE: u32 = fourcc(b"clrs");
const K_EVENT_HOT_KEY_ID: u32 = 1;

const CMD_KEY: u32 = 1 << 8;
const SHIFT_KEY: u32 = 1 << 9;
const KEY_CODE_V: u32 = 0x09;

#[link(name = "Carbon", kind = "framework")]
unsafe extern "C" {
    fn GetApplicationEventTarget() -> EventTargetRef;
    fn InstallEventHandler(
        target: EventTargetRef,
        handler: EventHandlerUPP,
        num_types: u32,
        list: *const EventTypeSpec,
        user_data: *mut c_void,
        out_ref: *mut EventHandlerRef,
    ) -> OSStatus;
    fn RemoveEventHandler(handler_ref: EventHandlerRef) -> OSStatus;
    fn RegisterEventHotKey(
        hot_key_code: u32,
        hot_key_modifiers: u32,
        hot_key_id: EventHotKeyID,
        target: EventTargetRef,
        options: u32,
        out_ref: *mut EventHotKeyRef,
    ) -> OSStatus;
    fn UnregisterEventHotKey(hot_key_ref: EventHotKeyRef) -> OSStatus;
}

#[derive(Default)]
struct MenuDelegateIvars {
    status_item: OnceCell<Retained<NSStatusItem>>,
    timer: OnceCell<Retained<NSTimer>>,
    store: OnceCell<Store>,
    last_pasteboard_change: Cell<i64>,
    last_error: RefCell<Option<String>>,
    hotkey_ref: Cell<EventHotKeyRef>,
    handler_ref: Cell<EventHandlerRef>,
    preview_window: OnceCell<Retained<NSWindow>>,
    preview_image_view: OnceCell<Retained<NSImageView>>,
    image_previews: RefCell<std::collections::HashMap<isize, Vec<u8>>>,
    preferences_window: RefCell<Option<Retained<NSWindow>>>,
    preferences_controls: RefCell<Option<PreferencesControls>>,
    search_window: OnceCell<Retained<NSWindow>>,
    search_field: OnceCell<Retained<NSTextField>>,
    search_results_view: OnceCell<Retained<NSView>>,
    search_entries: RefCell<Vec<HistoryEntry>>,
}

define_class!(
    #[unsafe(super = NSObject)]
    #[thread_kind = MainThreadOnly]
    #[ivars = MenuDelegateIvars]
    struct MenuDelegate;

    unsafe impl NSObjectProtocol for MenuDelegate {}

    unsafe impl NSMenuDelegate for MenuDelegate {
        #[unsafe(method(menuWillOpen:))]
        fn menu_will_open(&self, _menu: &NSMenu) {}

        #[unsafe(method(menu:willHighlightItem:))]
        fn menu_will_highlight_item(&self, _menu: &NSMenu, item: Option<&NSMenuItem>) {
            match item {
                Some(item) => self.update_image_preview(item.tag()),
                None => self.hide_image_preview(),
            }
        }

        #[unsafe(method(menuDidClose:))]
        fn menu_did_close(&self, _menu: &NSMenu) {
            self.hide_image_preview();
        }
    }

    unsafe impl NSWindowDelegate for MenuDelegate {
        #[unsafe(method(windowDidResignKey:))]
        fn window_did_resign_key(&self, _notification: &NSNotification) {
            self.close_search_panel();
        }
    }

    unsafe impl NSApplicationDelegate for MenuDelegate {
        #[unsafe(method(applicationDidFinishLaunching:))]
        fn did_finish_launching(&self, _notification: &NSNotification) {
            self.finish_launching();
        }

        #[unsafe(method(applicationWillTerminate:))]
        fn will_terminate(&self, _notification: &NSNotification) {
            self.unregister_hotkey();
        }
    }

    impl MenuDelegate {
        #[unsafe(method(pollClipboard:))]
        fn poll_clipboard(&self, _timer: &NSTimer) {
            self.capture_current_clipboard(false);
        }

        #[unsafe(method(captureNow:))]
        fn capture_now(&self, _sender: &NSMenuItem) {
            self.capture_current_clipboard(true);
            self.rebuild_menu();
        }

        #[unsafe(method(refreshMenu:))]
        fn refresh_menu(&self, _sender: &NSMenuItem) {
            self.rebuild_menu();
        }

        #[unsafe(method(copyHistoryItem:))]
        fn copy_history_item(&self, sender: &NSMenuItem) {
            let id = sender.tag();
            if id <= 0 {
                self.set_error("invalid history item id".to_string());
                return;
            }

            if let Some(store) = self.store() {
                match copy_history_entry(store, id as u64, true) {
                    Ok(()) => self.clear_error(),
                    Err(err) => self.report_paste_error(err),
                }
            }
            self.rebuild_menu();
        }

        #[unsafe(method(copyRichHistoryItem:))]
        fn copy_rich_history_item(&self, sender: &NSMenuItem) {
            let id = sender.tag();
            if id <= 0 {
                self.set_error("invalid rich history item id".to_string());
                return;
            }

            if let Some(store) = self.store() {
                match copy_rich_history_entry(store, id as u64, true) {
                    Ok(()) => self.clear_error(),
                    Err(err) => self.report_paste_error(err),
                }
            }
            self.rebuild_menu();
        }

        #[unsafe(method(clearHistory:))]
        fn clear_history(&self, _sender: &NSMenuItem) {
            if let Some(store) = self.store() {
                if let Err(err) = store.save_history(&[]) {
                    self.set_error(err);
                } else if let Err(err) = store.save_rich_history(&[]) {
                    self.set_error(err);
                } else {
                    self.clear_error();
                }
            }
            self.rebuild_menu();
        }

        #[unsafe(method(toggleHistoryPin:))]
        fn toggle_history_pin(&self, sender: &NSMenuItem) {
            let id = sender.tag();
            if id <= 0 {
                self.set_error("invalid history item id".to_string());
                return;
            }

            if let Some(store) = self.store()
                && let Err(err) = toggle_history_pin(store, id as u64)
            {
                self.set_error(err);
            }
            self.rebuild_menu();
        }

        #[unsafe(method(setLanguageEnglish:))]
        fn set_language_english(&self, _sender: &NSMenuItem) {
            self.update_settings(|settings| settings.language = Language::English);
        }

        #[unsafe(method(setLanguageChinese:))]
        fn set_language_chinese(&self, _sender: &NSMenuItem) {
            self.update_settings(|settings| settings.language = Language::Chinese);
        }

        #[unsafe(method(toggleRichClipboard:))]
        fn toggle_rich_clipboard(&self, _sender: &NSMenuItem) {
            self.update_settings(|settings| {
                settings.capture_rich_clipboard = !settings.capture_rich_clipboard
            });
        }

        #[unsafe(method(openPreferences:))]
        fn open_preferences(&self, _sender: &NSMenuItem) {
            self.show_preferences_panel();
        }

        #[unsafe(method(savePreferences:))]
        fn save_preferences(&self, _sender: &NSButton) {
            let settings = self
                .ivars()
                .preferences_controls
                .borrow()
                .as_ref()
                .map(read_preferences_controls);
            if let Some(settings) = settings {
                self.persist_settings(settings);
            }
            self.close_preferences_panel();
        }

        #[unsafe(method(cancelPreferences:))]
        fn cancel_preferences(&self, _sender: &NSButton) {
            self.close_preferences_panel();
        }

        #[unsafe(method(openNotes:))]
        fn open_notes(&self, _sender: &NSMenuItem) {
            match Command::new("open").args(["-a", "Notes"]).status() {
                Ok(status) if status.success() => self.clear_error(),
                Ok(status) => self.set_error(format!("failed to open Notes: {status}")),
                Err(err) => self.set_error(format!("failed to open Notes: {err}")),
            }
        }

        #[unsafe(method(showClipyMenu:))]
        fn show_clipy_menu(&self, _sender: &NSMenuItem) {
            self.show_status_menu();
        }

        #[unsafe(method(quit:))]
        fn quit(&self, _sender: &NSMenuItem) {
            let app = NSApplication::sharedApplication(self.mtm());
            app.terminate(None);
        }

        #[unsafe(method(openSearchPanel:))]
        fn open_search_panel_action(&self, _sender: &NSMenuItem) {
            self.close_status_menu();
            self.open_search_panel();
        }

        #[unsafe(method(cancelOperation:))]
        fn cancel_operation(&self, _sender: Option<&AnyObject>) {
            self.close_search_panel();
        }

        #[unsafe(method(searchPanelRowClicked:))]
        fn search_panel_row_clicked(&self, sender: &NSButton) {
            let id = sender.tag();
            if id > 0 {
                self.activate_search_entry(id as u64);
            }
        }
    }

    unsafe impl NSControlTextEditingDelegate for MenuDelegate {
        #[unsafe(method(controlTextDidChange:))]
        fn control_text_did_change(&self, _notification: &NSNotification) {
            self.refresh_search_candidates();
        }

        // 处理搜索框里的特殊按键：回车激活首个候选项、Esc 关闭浮层。
        #[unsafe(method(control:textView:doCommandBySelector:))]
        fn control_do_command(
            &self,
            _control: &NSControl,
            _text_view: &NSTextView,
            command: objc2::runtime::Sel,
        ) -> objc2::runtime::Bool {
            if command == sel!(insertNewline:) {
                self.activate_first_search_candidate();
                return objc2::runtime::Bool::YES;
            }
            if command == sel!(cancelOperation:) {
                self.close_search_panel();
                return objc2::runtime::Bool::YES;
            }
            objc2::runtime::Bool::NO
        }
    }

    unsafe impl NSTextFieldDelegate for MenuDelegate {}
);

impl MenuDelegate {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(MenuDelegateIvars::default());
        unsafe { msg_send![super(this), init] }
    }

    fn finish_launching(&self) {
        let app = NSApplication::sharedApplication(self.mtm());
        app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);

        match Store::open_default() {
            Ok(store) => {
                let _ = self.ivars().store.set(store);
            }
            Err(err) => self.set_error(err),
        }

        self.create_status_item();
        self.rebuild_menu();
        self.capture_current_clipboard(true);
        self.start_polling();

        if let Err(err) = self.register_hotkey() {
            self.set_error(err);
            self.rebuild_menu();
        }
    }

    fn create_status_item(&self) {
        let status_bar = NSStatusBar::systemStatusBar();
        let item = status_bar.statusItemWithLength(NSVariableStatusItemLength);
        if let Some(button) = item.button(self.mtm()) {
            // 优先使用内嵌的 logo 作为状态栏图标；macOS 会按当前菜单栏背景
            // 自动以模板图（template image）形式重新着色，呈现单色 logo。
            // 加载失败时退回到文字标题，保证菜单栏始终有可见入口。
            if let Some(image) = load_status_bar_image() {
                button.setImage(Some(&image));
                button.setTitle(&NSString::from_str(""));
            } else {
                button.setTitle(&NSString::from_str("Clip"));
            }
        }

        let menu =
            NSMenu::initWithTitle(NSMenu::alloc(self.mtm()), &NSString::from_str("clipy-rs"));
        menu.setDelegate(Some(ProtocolObject::from_ref(self)));
        item.setMenu(Some(&menu));

        let _ = self.ivars().status_item.set(item);
    }

    fn start_polling(&self) {
        let target = unsafe { as_any_object(self) };
        let timer = unsafe {
            NSTimer::scheduledTimerWithTimeInterval_target_selector_userInfo_repeats(
                0.75,
                target,
                sel!(pollClipboard:),
                None,
                true,
            )
        };
        let _ = self.ivars().timer.set(timer);
    }

    fn register_hotkey(&self) -> Result<(), String> {
        let mut handler_ref = null_mut();
        let event_spec = EventTypeSpec {
            event_class: K_EVENT_CLASS_KEYBOARD,
            event_kind: K_EVENT_HOT_KEY_PRESSED,
        };
        let user_data = self as *const Self as *mut c_void;

        let target = unsafe { GetApplicationEventTarget() };
        let handler_status = unsafe {
            InstallEventHandler(
                target,
                hotkey_handler,
                1,
                &event_spec,
                user_data,
                &mut handler_ref,
            )
        };
        if handler_status != NO_ERR {
            return Err(format!(
                "failed to install hotkey handler: {handler_status}"
            ));
        }
        self.ivars().handler_ref.set(handler_ref);

        let mut hotkey_ref = null_mut();
        let hotkey_id = EventHotKeyID {
            signature: K_EVENT_HOT_KEY_SIGNATURE,
            id: K_EVENT_HOT_KEY_ID,
        };
        let hotkey_status = unsafe {
            RegisterEventHotKey(
                KEY_CODE_V,
                CMD_KEY | SHIFT_KEY,
                hotkey_id,
                target,
                0,
                &mut hotkey_ref,
            )
        };
        if hotkey_status != NO_ERR {
            if !handler_ref.is_null() {
                let _ = unsafe { RemoveEventHandler(handler_ref) };
                self.ivars().handler_ref.set(null_mut());
            }
            return Err(format!(
                "failed to register global hotkey Cmd+Shift+V: {hotkey_status}"
            ));
        }

        self.ivars().hotkey_ref.set(hotkey_ref);
        Ok(())
    }

    fn unregister_hotkey(&self) {
        let hotkey_ref = self.ivars().hotkey_ref.replace(null_mut());
        if !hotkey_ref.is_null() {
            let _ = unsafe { UnregisterEventHotKey(hotkey_ref) };
        }

        let handler_ref = self.ivars().handler_ref.replace(null_mut());
        if !handler_ref.is_null() {
            let _ = unsafe { RemoveEventHandler(handler_ref) };
        }
    }

    fn capture_current_clipboard(&self, force_status: bool) {
        let Some(store) = self.store() else {
            return;
        };

        // 注意：不要在真正读到内容之前就更新 last_pasteboard_change，
        // 否则若此次轮询正好赶在剪贴板写入间隙（read_text 偶发为空），
        // 该 change_count 会被记成"已处理"，下次轮询直接跳过，
        // 导致历史第一项永远缺失。
        let current_change = clipboard::change_count().ok();
        if let Some(change_count) = current_change
            && !force_status
            && self.ivars().last_pasteboard_change.get() == change_count
        {
            return;
        }

        let settings = self.settings();
        if settings.capture_rich_clipboard {
            match clipboard::read_rich_clipboard(CAPTURE_MAX_RICH_BYTES) {
                Ok(Some(entry)) => {
                    match capture_rich(store, entry) {
                        Ok(CaptureStatus::Changed) => {
                            self.clear_error();
                            self.rebuild_menu();
                        }
                        Ok(CaptureStatus::Unchanged | CaptureStatus::Ignored) => {}
                        Err(err) => self.set_error(err),
                    }
                    if let Some(change_count) = current_change {
                        self.ivars().last_pasteboard_change.set(change_count);
                    }
                    return;
                }
                Ok(None) => {}
                Err(err) if force_status => self.set_error(err),
                Err(_) => {}
            }
        }

        match clipboard::read_text() {
            Ok(text) => {
                let normalized = normalize_clipboard_text(text.clone());
                if normalized.is_empty() {
                    // 剪贴板尚未就绪或非纯文本类型；不要消耗本次 change_count，
                    // 等下一次轮询再尝试，避免历史丢失。
                    if force_status && let Some(change_count) = current_change {
                        self.ivars().last_pasteboard_change.set(change_count);
                    }
                    return;
                }
                match capture_text(store, text) {
                    Ok(CaptureStatus::Changed) => {
                        self.clear_error();
                        self.rebuild_menu();
                    }
                    Ok(CaptureStatus::Unchanged | CaptureStatus::Ignored) => {}
                    Err(err) => self.set_error(err),
                }
                if let Some(change_count) = current_change {
                    self.ivars().last_pasteboard_change.set(change_count);
                }
            }
            Err(err) if force_status => self.set_error(err),
            Err(_) => {}
        }
    }

    fn rebuild_menu(&self) {
        let Some(status_item) = self.ivars().status_item.get() else {
            return;
        };
        let Some(menu) = status_item.menu(self.mtm()) else {
            return;
        };
        menu.removeAllItems();
        self.populate_menu(&menu);
    }

    fn populate_menu(&self, menu: &NSMenu) {
        let settings = self.settings();
        let lang = settings.language;
        self.configure_menu_appearance(menu);

        self.add_disabled_item(menu, t(lang, "history"));
        if let Some(store) = self.store() {
            match store.load_history() {
                Ok(entries) => self.add_history_items(menu, entries, lang),
                Err(err) => {
                    let prefix = format!("{}: ", t(lang, "load_history_failed"));
                    let preview_units = preview_units_for_prefix(settings.menu_width, &prefix);
                    let (err_preview, truncated) = preview_with_truncation(&err, preview_units);
                    let title = format!("{prefix}{err_preview}");
                    self.add_disabled_item_with_tooltip(
                        menu,
                        &title,
                        truncated.then_some(err.as_str()),
                    );
                }
            }
        } else {
            self.add_disabled_item(menu, t(lang, "storage_unavailable"));
        }

        menu.addItem(&NSMenuItem::separatorItem(self.mtm()));
        self.add_action_item(menu, t(lang, "search_history"), sel!(openSearchPanel:), 0);
        self.add_favorites_menu(menu, lang);
        self.add_rich_history_menu(menu, lang);
        self.add_action_item(menu, t(lang, "notes"), sel!(openNotes:), 0);
        menu.addItem(&NSMenuItem::separatorItem(self.mtm()));

        self.add_action_item(menu, t(lang, "preferences"), sel!(openPreferences:), 0);
        self.add_action_item(menu, t(lang, "clear_history"), sel!(clearHistory:), 0);
        self.add_action_item(menu, t(lang, "quit"), sel!(quit:), 0);
    }

    fn add_history_items(&self, menu: &NSMenu, entries: Vec<HistoryEntry>, lang: Language) {
        let settings = self.settings();
        // 收藏项只在「收藏」子菜单里展示，不在主文本历史中重复出现。
        let entries = sorted_history(entries)
            .into_iter()
            .filter(|entry| !entry.pinned)
            .collect::<Vec<_>>();
        let limited_entries = entries
            .into_iter()
            .take(settings.max_history_items)
            .collect::<Vec<_>>();
        if limited_entries.is_empty() {
            self.add_disabled_item(menu, t(lang, "no_history"));
            return;
        }
        let direct_count = settings
            .visible_history_items
            .min(settings.max_history_items)
            .min(limited_entries.len());
        for (idx, entry) in limited_entries.iter().take(direct_count).enumerate() {
            let (title, truncated) = history_item_title(idx, entry, settings.menu_width);
            self.add_action_item_with_tooltip(
                menu,
                &title,
                sel!(copyHistoryItem:),
                entry.id as isize,
                truncated.then_some(entry.content.as_str()),
            );
        }

        if direct_count >= limited_entries.len() {
            return;
        }

        for (chunk_index, chunk) in limited_entries[direct_count..]
            .chunks(settings.visible_history_items)
            .enumerate()
        {
            let chunk_start = direct_count + (chunk_index * settings.visible_history_items);
            let chunk_end = chunk_start + chunk.len();
            let submenu_item = self.new_menu_item(&format!("{} - {}", chunk_start + 1, chunk_end));
            let submenu = NSMenu::initWithTitle(
                NSMenu::alloc(self.mtm()),
                &NSString::from_str(&format!("{} - {}", chunk_start + 1, chunk_end)),
            );
            self.configure_menu_appearance(&submenu);

            for (offset, entry) in chunk.iter().enumerate() {
                let (title, truncated) =
                    history_item_title(chunk_start + offset, entry, settings.menu_width);
                self.add_action_item_with_tooltip(
                    &submenu,
                    &title,
                    sel!(copyHistoryItem:),
                    entry.id as isize,
                    truncated.then_some(entry.content.as_str()),
                );
            }

            submenu_item.setSubmenu(Some(&submenu));
            menu.addItem(&submenu_item);
        }
    }

    fn add_favorites_menu(&self, menu: &NSMenu, lang: Language) {
        let settings = self.settings();
        let submenu_item = self.new_menu_item(t(lang, "favorites"));
        let submenu = NSMenu::initWithTitle(
            NSMenu::alloc(self.mtm()),
            &NSString::from_str(t(lang, "favorites")),
        );
        self.configure_menu_appearance(&submenu);

        if let Some(store) = self.store() {
            let entries = sorted_history(store.load_history().unwrap_or_default());
            let favorites = entries
                .iter()
                .filter(|entry| entry.pinned)
                .take(settings.max_history_items)
                .collect::<Vec<_>>();
            let candidates = entries
                .iter()
                .filter(|entry| !entry.pinned)
                .take(settings.max_history_items)
                .collect::<Vec<_>>();

            if favorites.is_empty() {
                self.add_disabled_item(&submenu, t(lang, "no_favorites"));
            } else {
                for (idx, entry) in favorites.iter().enumerate() {
                    self.add_history_reference_item(
                        &submenu,
                        idx,
                        entry,
                        sel!(copyHistoryItem:),
                        settings.menu_width,
                    );
                }
            }

            submenu.addItem(&NSMenuItem::separatorItem(self.mtm()));
            self.add_history_toggle_submenu(
                &submenu,
                t(lang, "add_favorite"),
                &candidates,
                t(lang, "no_favorite_candidates"),
                settings.menu_width,
            );
            self.add_history_toggle_submenu(
                &submenu,
                t(lang, "remove_favorite"),
                &favorites,
                t(lang, "no_favorites"),
                settings.menu_width,
            );
        } else {
            self.add_disabled_item(&submenu, t(lang, "storage_unavailable"));
        }

        submenu_item.setSubmenu(Some(&submenu));
        menu.addItem(&submenu_item);
    }

    fn add_history_toggle_submenu(
        &self,
        menu: &NSMenu,
        title: &str,
        entries: &[&HistoryEntry],
        empty_title: &str,
        menu_width: usize,
    ) {
        let submenu_item = self.new_menu_item(title);
        let submenu = NSMenu::initWithTitle(NSMenu::alloc(self.mtm()), &NSString::from_str(title));
        self.configure_menu_appearance(&submenu);

        if entries.is_empty() {
            self.add_disabled_item(&submenu, empty_title);
        } else {
            for (idx, entry) in entries.iter().enumerate() {
                self.add_history_reference_item(
                    &submenu,
                    idx,
                    entry,
                    sel!(toggleHistoryPin:),
                    menu_width,
                );
            }
        }

        submenu_item.setSubmenu(Some(&submenu));
        menu.addItem(&submenu_item);
    }

    fn add_history_reference_item(
        &self,
        menu: &NSMenu,
        idx: usize,
        entry: &HistoryEntry,
        action: objc2::runtime::Sel,
        menu_width: usize,
    ) {
        let (title, truncated) = history_item_title(idx, entry, menu_width);
        self.add_action_item_with_tooltip(
            menu,
            &title,
            action,
            entry.id as isize,
            truncated.then_some(entry.content.as_str()),
        );
    }

    fn add_rich_history_menu(&self, menu: &NSMenu, lang: Language) {
        let settings = self.settings();
        let submenu_item = self.new_menu_item(t(lang, "rich_history"));
        let submenu = NSMenu::initWithTitle(
            NSMenu::alloc(self.mtm()),
            &NSString::from_str(t(lang, "rich_history")),
        );
        self.configure_menu_appearance(&submenu);
        submenu.setDelegate(Some(ProtocolObject::from_ref(self)));
        self.ivars().image_previews.borrow_mut().clear();

        if let Some(store) = self.store() {
            let entries = sorted_rich_history(store.load_rich_history().unwrap_or_default());
            let limited = entries
                .into_iter()
                .take(settings.max_rich_history_items)
                .collect::<Vec<_>>();
            if limited.is_empty() {
                self.add_disabled_item(&submenu, t(lang, "no_rich_history"));
            } else {
                let direct_count = settings
                    .visible_rich_history_items
                    .min(settings.max_rich_history_items)
                    .min(limited.len());

                for entry in limited.iter().take(direct_count) {
                    self.add_rich_entry_item(&submenu, entry, lang, settings.menu_width);
                }

                if direct_count < limited.len() {
                    for (chunk_index, chunk) in limited[direct_count..]
                        .chunks(settings.visible_rich_history_items)
                        .enumerate()
                    {
                        let chunk_start =
                            direct_count + (chunk_index * settings.visible_rich_history_items);
                        let chunk_end = chunk_start + chunk.len();
                        let title = format!("{} - {}", chunk_start + 1, chunk_end);
                        let chunk_item = self.new_menu_item(&title);
                        let chunk_menu = NSMenu::initWithTitle(
                            NSMenu::alloc(self.mtm()),
                            &NSString::from_str(&title),
                        );
                        self.configure_menu_appearance(&chunk_menu);
                        chunk_menu.setDelegate(Some(ProtocolObject::from_ref(self)));
                        for entry in chunk {
                            self.add_rich_entry_item(&chunk_menu, entry, lang, settings.menu_width);
                        }
                        chunk_item.setSubmenu(Some(&chunk_menu));
                        submenu.addItem(&chunk_item);
                    }
                }
            }
        } else {
            self.add_disabled_item(&submenu, t(lang, "storage_unavailable"));
        }

        submenu_item.setSubmenu(Some(&submenu));
        menu.addItem(&submenu_item);
    }

    fn add_rich_entry_item(
        &self,
        menu: &NSMenu,
        entry: &RichHistoryEntry,
        lang: Language,
        menu_width: usize,
    ) {
        let kind = match entry.kind {
            storage::RichClipboardKind::Image => t(lang, "kind_image"),
            storage::RichClipboardKind::File => t(lang, "kind_file"),
        };
        if entry.kind == storage::RichClipboardKind::Image
            && let Some(data) = preview_image_data(entry)
        {
            self.ivars()
                .image_previews
                .borrow_mut()
                .insert(entry.id as isize, data);
        }
        let prefix = format!("{kind}: ");
        let preview_units = preview_units_for_prefix(menu_width, &prefix);
        let (label, truncated) = preview_with_truncation(&entry.label, preview_units);
        let title = format!("{prefix}{label}");
        self.add_action_item_with_tooltip(
            menu,
            &title,
            sel!(copyRichHistoryItem:),
            entry.id as isize,
            truncated.then_some(entry.label.as_str()),
        );
    }

    fn ensure_preview_window(&self) -> (Retained<NSWindow>, Retained<NSImageView>) {
        let window = self
            .ivars()
            .preview_window
            .get_or_init(|| {
                let rect = NSRect::new(
                    NSPoint::new(0.0, 0.0),
                    NSSize::new(PREVIEW_PANEL_SIZE, PREVIEW_PANEL_SIZE),
                );
                let window: Retained<NSWindow> = unsafe {
                    NSWindow::initWithContentRect_styleMask_backing_defer(
                        NSWindow::alloc(self.mtm()),
                        rect,
                        NSWindowStyleMask::Borderless,
                        NSBackingStoreType::Buffered,
                        false,
                    )
                };
                window.setLevel(NSFloatingWindowLevel);
                window.setIgnoresMouseEvents(true);
                window.setHasShadow(true);
                window.setOpaque(false);
                window.setBackgroundColor(Some(&NSColor::clearColor()));

                let root =
                    NSVisualEffectView::initWithFrame(NSVisualEffectView::alloc(self.mtm()), rect);
                root.setMaterial(NSVisualEffectMaterial::HUDWindow);
                root.setBlendingMode(NSVisualEffectBlendingMode::BehindWindow);
                root.setState(NSVisualEffectState::Active);
                root.setWantsLayer(true);
                if let Some(layer) = root.layer() {
                    layer.setCornerRadius(18.0);
                    layer.setMasksToBounds(true);
                }
                window.setContentView(Some(&root));
                window
            })
            .clone();

        let image_view = self
            .ivars()
            .preview_image_view
            .get_or_init(|| {
                let inset = 16.0;
                let rect = NSRect::new(
                    NSPoint::new(inset, inset),
                    NSSize::new(
                        PREVIEW_PANEL_SIZE - inset * 2.0,
                        PREVIEW_PANEL_SIZE - inset * 2.0,
                    ),
                );
                let view = NSImageView::initWithFrame(NSImageView::alloc(self.mtm()), rect);
                view.setImageScaling(NSImageScaling::ScaleProportionallyUpOrDown);
                view.setWantsLayer(true);
                if let Some(layer) = view.layer() {
                    layer.setCornerRadius(12.0);
                    layer.setMasksToBounds(true);
                    layer.setBackgroundColor(Some(&NSColor::controlBackgroundColor().CGColor()));
                }
                if let Some(content) = window.contentView() {
                    content.addSubview(&view);
                }
                view
            })
            .clone();

        (window, image_view)
    }

    fn update_image_preview(&self, tag: isize) {
        if tag <= 0 {
            self.hide_image_preview();
            return;
        }
        let data = match self.ivars().image_previews.borrow().get(&tag) {
            Some(bytes) => bytes.clone(),
            None => {
                self.hide_image_preview();
                return;
            }
        };
        let ns_data = unsafe {
            NSData::dataWithBytes_length(data.as_ptr().cast::<c_void>(), data.len() as _)
        };
        let image_opt = NSImage::initWithData(NSImage::alloc(), &ns_data);
        let Some(image) = image_opt else {
            self.hide_image_preview();
            return;
        };

        let (window, image_view) = self.ensure_preview_window();
        image_view.setImage(Some(&image));
        position_preview_window(&window, self.mtm());
        window.orderFront(None);
    }

    fn hide_image_preview(&self) {
        if let Some(window) = self.ivars().preview_window.get() {
            window.orderOut(None);
        }
    }

    fn configure_menu_appearance(&self, menu: &NSMenu) {
        menu.setMinimumWidth(self.settings().menu_width as f64);
    }

    fn add_disabled_item(&self, menu: &NSMenu, title: &str) {
        self.add_disabled_item_with_tooltip(menu, title, None);
    }

    fn add_disabled_item_with_tooltip(&self, menu: &NSMenu, title: &str, tooltip: Option<&str>) {
        let item = self.new_disabled_item(title);
        if let Some(tooltip) = tooltip {
            set_menu_item_tooltip(&item, tooltip);
        }
        menu.addItem(&item);
    }

    fn new_menu_item(&self, title: &str) -> Retained<NSMenuItem> {
        unsafe {
            NSMenuItem::initWithTitle_action_keyEquivalent(
                NSMenuItem::alloc(self.mtm()),
                &NSString::from_str(title),
                None,
                &NSString::from_str(""),
            )
        }
    }

    fn new_disabled_item(&self, title: &str) -> Retained<NSMenuItem> {
        let item = self.new_menu_item(title);
        item.setEnabled(false);
        item
    }

    fn add_action_item(&self, menu: &NSMenu, title: &str, action: objc2::runtime::Sel, tag: isize) {
        self.add_action_item_with_tooltip(menu, title, action, tag, None);
    }

    fn add_action_item_with_tooltip(
        &self,
        menu: &NSMenu,
        title: &str,
        action: objc2::runtime::Sel,
        tag: isize,
        tooltip: Option<&str>,
    ) {
        let item = unsafe {
            NSMenuItem::initWithTitle_action_keyEquivalent(
                NSMenuItem::alloc(self.mtm()),
                &NSString::from_str(title),
                Some(action),
                &NSString::from_str(""),
            )
        };
        item.setTag(tag);
        if let Some(tooltip) = tooltip {
            set_menu_item_tooltip(&item, tooltip);
        }
        let target = unsafe { as_any_object(self) };
        unsafe { item.setTarget(Some(target)) };
        menu.addItem(&item);
    }

    fn show_status_menu(&self) {
        self.rebuild_menu();
        let Some(status_item) = self.ivars().status_item.get() else {
            return;
        };
        let Some(menu) = status_item.menu(self.mtm()) else {
            return;
        };
        #[allow(deprecated)]
        status_item.popUpStatusItemMenu(&menu);
    }

    fn show_menu_at_mouse(&self) {
        let menu = NSMenu::initWithTitle(
            NSMenu::alloc(self.mtm()),
            &NSString::from_str(t(self.settings().language, "app_name")),
        );
        self.populate_menu(&menu);
        let mouse_location = NSEvent::mouseLocation();
        let popup_location = adjusted_popup_location(
            mouse_location,
            menu.numberOfItems().max(0) as usize,
            self.mtm(),
        );
        // A false return can simply mean the user dismissed the menu by clicking outside.
        // Do not fall back to the status bar menu, or it will reopen after dismissal.
        let _ = menu.popUpMenuPositioningItem_atLocation_inView(None, popup_location, None);
    }

    fn show_preferences_panel(&self) {
        let settings = self.settings();
        let lang = settings.language;
        let controls = build_preferences_controls(settings, lang, self.mtm());
        let rect = NSRect::new(
            NSPoint::new(0.0, 0.0),
            NSSize::new(PREFERENCES_PANEL_WIDTH, PREFERENCES_PANEL_HEIGHT),
        );
        let window = unsafe {
            NSWindow::initWithContentRect_styleMask_backing_defer(
                NSWindow::alloc(self.mtm()),
                rect,
                NSWindowStyleMask::Titled | NSWindowStyleMask::FullSizeContentView,
                NSBackingStoreType::Buffered,
                false,
            )
        };
        window.setTitle(&NSString::from_str(t(lang, "preferences_title")));
        window.setTitleVisibility(NSWindowTitleVisibility::Hidden);
        window.setTitlebarAppearsTransparent(true);
        window.setMovableByWindowBackground(true);
        window.setLevel(NSNormalWindowLevel);
        window.setHasShadow(true);
        window.setOpaque(false);
        window.setBackgroundColor(Some(&NSColor::clearColor()));

        let root = NSVisualEffectView::initWithFrame(NSVisualEffectView::alloc(self.mtm()), rect);
        root.setMaterial(NSVisualEffectMaterial::HUDWindow);
        root.setBlendingMode(NSVisualEffectBlendingMode::BehindWindow);
        root.setState(NSVisualEffectState::Active);
        root.setWantsLayer(true);
        if let Some(layer) = root.layer() {
            layer.setCornerRadius(22.0);
            layer.setMasksToBounds(true);
        }

        let title = NSTextField::labelWithString(
            &NSString::from_str(t(lang, "preferences_title")),
            self.mtm(),
        );
        title.setFrame(NSRect::new(
            NSPoint::new(32.0, PREFERENCES_PANEL_HEIGHT - 70.0),
            NSSize::new(PREFERENCES_PANEL_WIDTH - 64.0, 30.0),
        ));
        title.setFont(Some(&NSFont::systemFontOfSize_weight(24.0, 0.35)));
        title.setTextColor(Some(&NSColor::labelColor()));
        title.setUsesSingleLineMode(true);
        title.setLineBreakMode(NSLineBreakMode::ByClipping);
        root.addSubview(&title);

        let help = NSTextField::labelWithString(
            &NSString::from_str(t(lang, "preferences_help")),
            self.mtm(),
        );
        help.setFrame(NSRect::new(
            NSPoint::new(32.0, PREFERENCES_PANEL_HEIGHT - 140.0),
            NSSize::new(PREFERENCES_PANEL_WIDTH - 64.0, 54.0),
        ));
        help.setFont(Some(&NSFont::systemFontOfSize(15.0)));
        help.setTextColor(Some(&NSColor::secondaryLabelColor()));
        help.setUsesSingleLineMode(false);
        help.setLineBreakMode(NSLineBreakMode::ByWordWrapping);
        root.addSubview(&help);

        controls.view.setFrame(NSRect::new(
            NSPoint::new(32.0, 88.0),
            NSSize::new(PREFERENCES_PANEL_WIDTH - 64.0, 310.0),
        ));
        root.addSubview(&controls.view);

        let cancel = preferences_button(
            t(lang, "cancel"),
            NSPoint::new(PREFERENCES_PANEL_WIDTH - 292.0, 30.0),
            sel!(cancelPreferences:),
            self,
            self.mtm(),
        );
        let save = preferences_button(
            t(lang, "save"),
            NSPoint::new(PREFERENCES_PANEL_WIDTH - 152.0, 30.0),
            sel!(savePreferences:),
            self,
            self.mtm(),
        );
        save.setKeyEquivalent(&NSString::from_str("\r"));
        root.addSubview(&cancel);
        root.addSubview(&save);

        window.setContentView(Some(&root));
        window.center();

        let app = NSApplication::sharedApplication(self.mtm());
        app.activate();
        window.makeKeyAndOrderFront(None);

        *self.ivars().preferences_controls.borrow_mut() = Some(controls);
        *self.ivars().preferences_window.borrow_mut() = Some(window);
    }

    fn close_preferences_panel(&self) {
        if let Some(window) = self.ivars().preferences_window.borrow_mut().take() {
            window.orderOut(None);
        }
        *self.ivars().preferences_controls.borrow_mut() = None;
    }

    fn store(&self) -> Option<&Store> {
        self.ivars().store.get()
    }

    fn settings(&self) -> AppSettings {
        self.store()
            .and_then(|store| store.load_settings().ok())
            .unwrap_or_default()
    }

    fn persist_settings(&self, settings: AppSettings) {
        let settings = storage::normalize_settings(settings);
        if let Some(store) = self.store() {
            match store.save_settings(&settings) {
                Ok(()) => self.clear_error(),
                Err(err) => self.set_error(err),
            }
        }
        self.rebuild_menu();
    }

    fn update_settings(&self, update: impl FnOnce(&mut AppSettings)) {
        if let Some(store) = self.store() {
            let mut settings = store.load_settings().unwrap_or_default();
            update(&mut settings);
            self.persist_settings(settings);
            return;
        }
        self.rebuild_menu();
    }

    fn set_error(&self, err: String) {
        *self.ivars().last_error.borrow_mut() = Some(err);
    }

    fn clear_error(&self) {
        *self.ivars().last_error.borrow_mut() = None;
    }

    fn close_status_menu(&self) {
        if let Some(status_item) = self.ivars().status_item.get()
            && let Some(menu) = status_item.menu(self.mtm())
        {
            menu.cancelTrackingWithoutAnimation();
        }
    }

    fn open_search_panel(&self) {
        let (window, field, _) = self.ensure_search_panel();
        field.setStringValue(&NSString::from_str(""));
        self.refresh_search_candidates();
        position_search_window(&window, self.mtm());

        let app = NSApplication::sharedApplication(self.mtm());
        app.activate();
        window.makeKeyAndOrderFront(None);
        let _: bool = unsafe { msg_send![&*window, makeFirstResponder: &*field] };
    }

    fn close_search_panel(&self) {
        if let Some(window) = self.ivars().search_window.get() {
            window.orderOut(None);
        }
    }

    fn ensure_search_panel(&self) -> (Retained<NSWindow>, Retained<NSTextField>, Retained<NSView>) {
        let window = self
            .ivars()
            .search_window
            .get_or_init(|| self.build_search_window())
            .clone();
        let field = self
            .ivars()
            .search_field
            .get()
            .expect("search field is initialized with the search window")
            .clone();
        let results_view = self
            .ivars()
            .search_results_view
            .get()
            .expect("search results view is initialized with the search window")
            .clone();
        (window, field, results_view)
    }

    fn build_search_window(&self) -> Retained<NSWindow> {
        let rect = NSRect::new(
            NSPoint::new(0.0, 0.0),
            NSSize::new(SEARCH_PANEL_WIDTH, SEARCH_PANEL_HEIGHT),
        );
        let style = NSWindowStyleMask::Titled | NSWindowStyleMask::FullSizeContentView;
        let window = unsafe {
            NSWindow::initWithContentRect_styleMask_backing_defer(
                NSWindow::alloc(self.mtm()),
                rect,
                style,
                NSBackingStoreType::Buffered,
                false,
            )
        };
        window.setTitle(&NSString::from_str(t(
            self.settings().language,
            "search_history",
        )));
        window.setTitleVisibility(NSWindowTitleVisibility::Hidden);
        window.setTitlebarAppearsTransparent(true);
        window.setMovableByWindowBackground(true);
        window.setLevel(NSNormalWindowLevel);
        window.setHasShadow(true);
        window.setOpaque(false);
        window.setBackgroundColor(Some(&NSColor::clearColor()));
        window.setDelegate(Some(ProtocolObject::from_ref(self)));

        let root = NSVisualEffectView::initWithFrame(NSVisualEffectView::alloc(self.mtm()), rect);
        root.setMaterial(NSVisualEffectMaterial::HUDWindow);
        root.setBlendingMode(NSVisualEffectBlendingMode::BehindWindow);
        root.setState(NSVisualEffectState::Active);
        root.setWantsLayer(true);
        if let Some(layer) = root.layer() {
            layer.setCornerRadius(18.0);
            layer.setMasksToBounds(true);
        }

        let title = NSTextField::labelWithString(
            &NSString::from_str(t(self.settings().language, "search_history")),
            self.mtm(),
        );
        title.setFrame(NSRect::new(
            NSPoint::new(SEARCH_PANEL_PADDING, SEARCH_PANEL_HEIGHT - 44.0),
            NSSize::new(SEARCH_PANEL_WIDTH - SEARCH_PANEL_PADDING * 2.0, 26.0),
        ));
        title.setFont(Some(&NSFont::systemFontOfSize_weight(17.0, 0.45)));
        title.setTextColor(Some(&NSColor::labelColor()));
        root.addSubview(&title);

        let search_box = NSView::initWithFrame(
            NSView::alloc(self.mtm()),
            NSRect::new(
                NSPoint::new(SEARCH_PANEL_PADDING, SEARCH_PANEL_HEIGHT - 96.0),
                NSSize::new(SEARCH_PANEL_WIDTH - SEARCH_PANEL_PADDING * 2.0, 44.0),
            ),
        );
        search_box.setWantsLayer(true);
        if let Some(layer) = search_box.layer() {
            layer.setCornerRadius(12.0);
            layer.setMasksToBounds(true);
            layer.setBackgroundColor(Some(&NSColor::controlBackgroundColor().CGColor()));
        }

        let icon = NSTextField::labelWithString(&NSString::from_str("🔍"), self.mtm());
        icon.setFrame(NSRect::new(
            NSPoint::new(14.0, 11.0),
            NSSize::new(22.0, 22.0),
        ));
        search_box.addSubview(&icon);

        let field = NSTextField::initWithFrame(
            NSTextField::alloc(self.mtm()),
            NSRect::new(
                NSPoint::new(42.0, 9.0),
                NSSize::new(SEARCH_PANEL_WIDTH - SEARCH_PANEL_PADDING * 2.0 - 58.0, 26.0),
            ),
        );
        field.setPlaceholderString(Some(&NSString::from_str(t(
            self.settings().language,
            "search_placeholder",
        ))));
        field.setBezeled(false);
        field.setBordered(false);
        field.setDrawsBackground(false);
        field.setFocusRingType(NSFocusRingType::None);
        field.setEditable(true);
        field.setSelectable(true);
        field.setFont(Some(&NSFont::systemFontOfSize(18.0)));
        field.setTextColor(Some(&NSColor::labelColor()));
        unsafe {
            field.setDelegate(Some(ProtocolObject::from_ref(self)));
        }
        search_box.addSubview(&field);
        root.addSubview(&search_box);

        let scroll_frame = NSRect::new(
            NSPoint::new(SEARCH_PANEL_PADDING, SEARCH_PANEL_PADDING),
            NSSize::new(
                SEARCH_PANEL_WIDTH - SEARCH_PANEL_PADDING * 2.0,
                SEARCH_PANEL_HEIGHT - 124.0,
            ),
        );
        let scroll = NSScrollView::initWithFrame(NSScrollView::alloc(self.mtm()), scroll_frame);
        scroll.setBorderType(NSBorderType::NoBorder);
        scroll.setDrawsBackground(false);
        scroll.setHasVerticalScroller(true);
        scroll.setHasHorizontalScroller(false);
        scroll.setAutohidesScrollers(true);

        let results_view = NSView::initWithFrame(
            NSView::alloc(self.mtm()),
            NSRect::new(NSPoint::new(0.0, 0.0), scroll_frame.size),
        );
        scroll.setDocumentView(Some(&results_view));
        root.addSubview(&scroll);

        window.setContentView(Some(&root));
        let _ = self.ivars().search_field.set(field);
        let _ = self.ivars().search_results_view.set(results_view);
        window
    }

    fn refresh_search_candidates(&self) {
        let lang = self.settings().language;
        let Some(store) = self.store() else {
            return;
        };
        let query = self
            .ivars()
            .search_field
            .get()
            .map(|field| nsstring_to_string(&field.stringValue()))
            .unwrap_or_default();
        let entries = sorted_history(store.load_history().unwrap_or_default());
        let ranked = ranked_history_matches(entries, &query);
        *self.ivars().search_entries.borrow_mut() = ranked.clone();
        if let Some(view) = self.ivars().search_results_view.get() {
            self.render_search_results_view(view, &ranked, &query, lang);
        }
    }

    fn render_search_results_view(
        &self,
        view: &NSView,
        entries: &[HistoryEntry],
        query: &str,
        lang: Language,
    ) {
        view.setSubviews(&NSArray::new());
        let width = SEARCH_PANEL_WIDTH - SEARCH_PANEL_PADDING * 2.0;
        if query.trim().is_empty() {
            self.add_search_empty_state(view, t(lang, "search_placeholder"), width);
            return;
        }
        if entries.is_empty() {
            self.add_search_empty_state(view, t(lang, "search_no_results"), width);
            return;
        }

        let settings = self.settings();
        let max_candidates = settings.max_history_items.min(SEARCH_MAX_CANDIDATES);
        let rendered = entries.iter().take(max_candidates).collect::<Vec<_>>();
        let height = (rendered.len() as f64 * SEARCH_ROW_HEIGHT).max(SEARCH_PANEL_HEIGHT - 124.0);
        view.setFrame(NSRect::new(
            NSPoint::new(0.0, 0.0),
            NSSize::new(width, height),
        ));

        for (idx, entry) in rendered.iter().enumerate() {
            let prefix = format!("{:>2}. ", idx + 1);
            let preview_units = preview_units_for_prefix(width as usize, &prefix);
            let (content, truncated) = preview_with_truncation(&entry.content, preview_units);
            let title = format!("{prefix}{content}");
            let y = height - (idx as f64 + 1.0) * SEARCH_ROW_HEIGHT;

            let label = NSTextField::labelWithString(&NSString::from_str(&title), self.mtm());
            label.setFrame(NSRect::new(
                NSPoint::new(6.0, y + 11.0),
                NSSize::new(width - 12.0, 20.0),
            ));
            label.setFont(Some(&NSFont::systemFontOfSize(14.0)));
            label.setTextColor(Some(&NSColor::labelColor()));
            label.setUsesSingleLineMode(true);
            label.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
            view.addSubview(&label);

            let button = unsafe {
                NSButton::buttonWithTitle_target_action(
                    &NSString::from_str(""),
                    Some(as_any_object(self)),
                    Some(sel!(searchPanelRowClicked:)),
                    self.mtm(),
                )
            };
            button.setFrame(NSRect::new(
                NSPoint::new(0.0, y + 3.0),
                NSSize::new(width, SEARCH_ROW_HEIGHT - 6.0),
            ));
            button.setButtonType(NSButtonType::MomentaryChange);
            button.setBordered(false);
            button.setTransparent(true);
            button.setTag(entry.id as isize);
            button.setToolTip(Some(&NSString::from_str(if truncated {
                entry.content.as_str()
            } else {
                &title
            })));
            view.addSubview(&button);
        }
        if height > SEARCH_PANEL_HEIGHT - 124.0 {
            view.scrollPoint(NSPoint::new(0.0, height - (SEARCH_PANEL_HEIGHT - 124.0)));
        } else {
            view.scrollPoint(NSPoint::new(0.0, 0.0));
        }
    }

    fn add_search_empty_state(&self, view: &NSView, message: &str, width: f64) {
        view.setFrame(NSRect::new(
            NSPoint::new(0.0, 0.0),
            NSSize::new(width, SEARCH_PANEL_HEIGHT - 124.0),
        ));
        let label = NSTextField::labelWithString(&NSString::from_str(message), self.mtm());
        label.setFrame(NSRect::new(
            NSPoint::new(0.0, SEARCH_PANEL_HEIGHT - 164.0),
            NSSize::new(width, 26.0),
        ));
        label.setAlignment(NSTextAlignment::Center);
        label.setFont(Some(&NSFont::systemFontOfSize(14.0)));
        label.setTextColor(Some(&NSColor::secondaryLabelColor()));
        view.addSubview(&label);
    }

    /// 回车时激活当前候选列表中的第一项，等价于点击它。
    fn activate_first_search_candidate(&self) {
        let id = self.ivars().search_entries.borrow().first().map(|e| e.id);
        if let Some(id) = id {
            self.activate_search_entry(id);
        }
    }

    /// 复制并粘贴指定历史条目，然后关闭搜索浮层并刷新菜单内容。
    fn activate_search_entry(&self, id: u64) {
        self.close_search_panel();
        let app = NSApplication::sharedApplication(self.mtm());
        unsafe {
            let _: () = msg_send![&*app, deactivate];
        }
        if let Some(store) = self.store() {
            match copy_history_entry(store, id, true) {
                Ok(()) => self.clear_error(),
                Err(err) => self.report_paste_error(err),
            }
        }
        self.rebuild_menu();
    }

    /// 处理来自 `paste_frontmost` 等接口的错误：
    /// 若是辅助功能权限相关失败，弹原生 NSAlert 引导用户去系统设置；
    /// 否则照旧把错误写入菜单状态栏。
    fn report_paste_error(&self, err: String) {
        if err.contains("Accessibility permission") {
            self.set_error(err);
            self.show_accessibility_alert();
        } else {
            self.set_error(err);
        }
    }

    fn show_accessibility_alert(&self) {
        // 已经授权就直接清状态，不打扰用户。
        if clipboard::is_accessibility_trusted() {
            self.clear_error();
            return;
        }
        let lang = self.settings().language;
        let alert = NSAlert::new(self.mtm());
        alert.setAlertStyle(NSAlertStyle::Warning);
        alert.setMessageText(&NSString::from_str(t(lang, "permission_required_title")));
        alert.setInformativeText(&NSString::from_str(t(lang, "permission_required_body")));
        alert.addButtonWithTitle(&NSString::from_str(t(lang, "open_settings")));
        alert.addButtonWithTitle(&NSString::from_str(t(lang, "cancel")));

        let app = NSApplication::sharedApplication(self.mtm());
        app.activate();

        if alert.runModal() == NSAlertFirstButtonReturn {
            let _ = Command::new("open")
                .arg(
                    "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility",
                )
                .status();
        }
    }
}

pub fn run() -> Result<(), String> {
    let mtm = MainThreadMarker::new()
        .ok_or_else(|| "the menu bar GUI must run on the main thread".to_string())?;
    let app = NSApplication::sharedApplication(mtm);
    let delegate = MenuDelegate::new(mtm);
    app.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
    app.run();
    Ok(())
}

unsafe extern "C" fn hotkey_handler(
    _next_handler: EventHandlerCallRef,
    _event: EventRef,
    user_data: *mut c_void,
) -> OSStatus {
    if !user_data.is_null() {
        let delegate = unsafe { &*(user_data as *const MenuDelegate) };
        delegate.show_menu_at_mouse();
    }
    NO_ERR
}

unsafe fn as_any_object<T>(value: &T) -> &AnyObject {
    unsafe { &*(value as *const T as *const AnyObject) }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CaptureStatus {
    Changed,
    Unchanged,
    Ignored,
}

struct PreferencesControls {
    view: Retained<NSView>,
    history_limit_field: Retained<NSTextField>,
    visible_count_field: Retained<NSTextField>,
    rich_history_limit_field: Retained<NSTextField>,
    rich_visible_count_field: Retained<NSTextField>,
    menu_width_field: Retained<NSTextField>,
    language_popup: Retained<NSPopUpButton>,
    rich_popup: Retained<NSPopUpButton>,
}

fn capture_text(store: &Store, text: String) -> Result<CaptureStatus, String> {
    let text = normalize_clipboard_text(text);
    if text.is_empty() || text.len() > CAPTURE_MAX_BYTES || sensitive::looks_sensitive(&text) {
        return Ok(CaptureStatus::Ignored);
    }

    // 历史永不裁剪：设置中的上限只影响展示数量，存储保留全部条目。
    let mut entries = store.load_history()?;
    let inserted = storage::upsert_history(&mut entries, text);
    store.save_history(&entries)?;

    if inserted {
        Ok(CaptureStatus::Changed)
    } else {
        Ok(CaptureStatus::Unchanged)
    }
}

fn capture_rich(store: &Store, entry: RichHistoryEntry) -> Result<CaptureStatus, String> {
    // 历史永不裁剪：设置中的上限只影响展示数量，存储保留全部条目。
    let mut entries = store.load_rich_history()?;
    let inserted = storage::upsert_rich_history(&mut entries, entry);
    store.save_rich_history(&entries)?;

    if inserted {
        Ok(CaptureStatus::Changed)
    } else {
        Ok(CaptureStatus::Unchanged)
    }
}

fn copy_history_entry(store: &Store, id: u64, paste: bool) -> Result<(), String> {
    let mut entries = store.load_history()?;
    let entry_index = entries
        .iter()
        .position(|entry| entry.id == id)
        .ok_or_else(|| format!("history item `{id}` was not found"))?;
    let content = entries[entry_index].content.clone();
    clipboard::write_text(&content)?;
    if paste {
        clipboard::paste_frontmost()?;
    }
    let mut entry = entries.remove(entry_index);
    entry.use_count += 1;
    entry.updated_at = storage::now_millis();
    entries.insert(0, entry);
    store.save_history(&entries)?;
    Ok(())
}

fn toggle_history_pin(store: &Store, id: u64) -> Result<(), String> {
    let mut entries = store.load_history()?;
    let entry = entries
        .iter_mut()
        .find(|entry| entry.id == id)
        .ok_or_else(|| format!("history item `{id}` was not found"))?;
    entry.pinned = !entry.pinned;
    entry.updated_at = storage::now_millis();
    store.save_history(&entries)
}

fn copy_rich_history_entry(store: &Store, id: u64, paste: bool) -> Result<(), String> {
    let mut entries = store.load_rich_history()?;
    let entry_index = entries
        .iter()
        .position(|entry| entry.id == id)
        .ok_or_else(|| format!("rich history item `{id}` was not found"))?;
    clipboard::write_rich_clipboard(&entries[entry_index])?;
    if paste {
        clipboard::paste_frontmost()?;
    }
    entries[entry_index].use_count += 1;
    entries[entry_index].updated_at = storage::now_millis();
    store.save_rich_history(&entries)?;
    Ok(())
}

fn normalize_clipboard_text(text: String) -> String {
    text.trim_matches('\0').to_string()
}

fn history_item_title(index: usize, entry: &HistoryEntry, menu_width: usize) -> (String, bool) {
    let pin = if entry.pinned { "* " } else { "" };
    let prefix = format!("{:>2}. {pin}", index + 1);
    let preview_units = preview_units_for_prefix(menu_width, &prefix);
    let (content, truncated) = preview_with_truncation(&entry.content, preview_units);
    (format!("{prefix}{content}"), truncated)
}

fn sorted_history(mut entries: Vec<HistoryEntry>) -> Vec<HistoryEntry> {
    entries.sort_by(|left, right| {
        right
            .pinned
            .cmp(&left.pinned)
            .then_with(|| right.updated_at.cmp(&left.updated_at))
            .then_with(|| right.id.cmp(&left.id))
    });
    entries
}

fn ranked_history_matches(entries: Vec<HistoryEntry>, query: &str) -> Vec<HistoryEntry> {
    let query = normalize_search_text(query);
    if query.is_empty() {
        return Vec::new();
    }

    let mut scored = entries
        .into_iter()
        .filter_map(|entry| history_match_score(&query, &entry.content).map(|score| (entry, score)))
        .collect::<Vec<_>>();
    scored.sort_by(|(left, left_score), (right, right_score)| {
        right_score
            .cmp(left_score)
            .then_with(|| right.use_count.cmp(&left.use_count))
            .then_with(|| right.updated_at.cmp(&left.updated_at))
            .then_with(|| right.id.cmp(&left.id))
    });
    scored.into_iter().map(|(entry, _score)| entry).collect()
}

fn history_match_score(query: &str, content: &str) -> Option<i64> {
    let content = normalize_search_text(content);
    if content.is_empty() {
        return None;
    }
    if content == query {
        return Some(1_000_000);
    }
    if content.starts_with(query) {
        return Some(900_000 - content.len().saturating_sub(query.len()) as i64);
    }
    if let Some(index) = content.find(query) {
        return Some(800_000 - index as i64 * 20);
    }

    let query_words = query.split_whitespace().collect::<Vec<_>>();
    if !query_words.is_empty() && query_words.iter().all(|word| content.contains(word)) {
        let first_index = query_words
            .iter()
            .filter_map(|word| content.find(word))
            .min()
            .unwrap_or(0);
        return Some(700_000 + query_words.len() as i64 * 1_000 - first_index as i64 * 10);
    }

    fuzzy_subsequence_score(query, &content).map(|score| 500_000 + score)
}

fn fuzzy_subsequence_score(query: &str, content: &str) -> Option<i64> {
    let mut last_match: Option<usize> = None;
    let mut search_start = 0usize;
    let mut score = 0i64;
    for ch in query.chars() {
        let rest = content.get(search_start..)?;
        let found = rest.find(ch)?;
        let absolute = search_start + found;
        if let Some(prev) = last_match {
            let gap = absolute.saturating_sub(prev + 1);
            score -= gap as i64 * 8;
            if gap == 0 {
                score += 25;
            }
        } else {
            score -= absolute as i64 * 12;
        }
        score += 100;
        last_match = Some(absolute);
        search_start = absolute + ch.len_utf8();
    }
    Some(score)
}

fn normalize_search_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut previous_space = false;
    for ch in text.to_lowercase().chars() {
        let mapped = if ch.is_control() || ch.is_whitespace() {
            ' '
        } else {
            ch
        };
        if mapped == ' ' {
            if previous_space {
                continue;
            }
            previous_space = true;
        } else {
            previous_space = false;
        }
        out.push(mapped);
    }
    out.trim().to_string()
}

fn sorted_rich_history(mut entries: Vec<RichHistoryEntry>) -> Vec<RichHistoryEntry> {
    entries.sort_by(|left, right| {
        right
            .pinned
            .cmp(&left.pinned)
            .then_with(|| right.updated_at.cmp(&left.updated_at))
            .then_with(|| right.id.cmp(&left.id))
    });
    entries
}

fn build_preferences_controls(
    settings: AppSettings,
    lang: Language,
    mtm: MainThreadMarker,
) -> PreferencesControls {
    // 自上而下布局：加大左右留白，避免英文长标签被截断。
    const ROW_HEIGHT: f64 = 40.0;
    const PANEL_WIDTH: f64 = PREFERENCES_PANEL_WIDTH - 64.0;
    const TOTAL_ROWS: f64 = 7.0; // 语言、文本上限、文本展示、图片上限、图片展示、菜单宽度、富文本开关
    const PANEL_HEIGHT: f64 = TOTAL_ROWS * ROW_HEIGHT + 16.0;
    const LABEL_X: f64 = 24.0;
    const LABEL_WIDTH: f64 = 270.0;
    const CONTROL_X: f64 = 330.0;
    const CONTROL_WIDTH: f64 = 260.0;
    const NUMBER_FIELD_WIDTH: f64 = 148.0;
    const NUMBER_FIELD_HEIGHT: f64 = 20.0;
    const NUMBER_FIELD_Y_OFFSET: f64 = 2.0;
    const NUMBER_BACKGROUND_HEIGHT: f64 = 32.0;
    const NUMBER_BACKGROUND_Y_OFFSET: f64 = -4.0;

    let view = NSView::initWithFrame(
        NSView::alloc(mtm),
        NSRect::new(
            NSPoint::new(0.0, 0.0),
            NSSize::new(PANEL_WIDTH, PANEL_HEIGHT),
        ),
    );
    view.setWantsLayer(true);
    if let Some(layer) = view.layer() {
        layer.setCornerRadius(14.0);
        layer.setMasksToBounds(true);
        layer.setBackgroundColor(Some(&NSColor::colorWithWhite_alpha(1.0, 0.10).CGColor()));
    }

    let row_y = |row: usize| PANEL_HEIGHT - 24.0 - row as f64 * ROW_HEIGHT;

    let language_label =
        NSTextField::labelWithString(&NSString::from_str(t(lang, "language")), mtm);
    language_label.setFrame(NSRect::new(
        NSPoint::new(LABEL_X, row_y(0)),
        NSSize::new(LABEL_WIDTH, 24.0),
    ));
    let language_popup = NSPopUpButton::initWithFrame_pullsDown(
        NSPopUpButton::alloc(mtm),
        NSRect::new(
            NSPoint::new(CONTROL_X, row_y(0) - 4.0),
            NSSize::new(CONTROL_WIDTH, 28.0),
        ),
        false,
    );
    language_popup.addItemWithTitle(&NSString::from_str("English"));
    language_popup.addItemWithTitle(&NSString::from_str("中文"));
    language_popup.selectItemAtIndex(match settings.language {
        Language::English => 0,
        Language::Chinese => 1,
    });

    let history_limit_label =
        NSTextField::labelWithString(&NSString::from_str(t(lang, "history_limit")), mtm);
    history_limit_label.setFrame(NSRect::new(
        NSPoint::new(LABEL_X, row_y(1)),
        NSSize::new(LABEL_WIDTH, 24.0),
    ));
    let history_limit_field = NSTextField::initWithFrame(
        NSTextField::alloc(mtm),
        NSRect::new(
            NSPoint::new(CONTROL_X, row_y(1) + NUMBER_FIELD_Y_OFFSET),
            NSSize::new(NUMBER_FIELD_WIDTH, NUMBER_FIELD_HEIGHT),
        ),
    );
    style_preferences_number_field(&history_limit_field);
    history_limit_field
        .setStringValue(&NSString::from_str(&settings.max_history_items.to_string()));
    let history_limit_background = preferences_number_background(
        NSPoint::new(CONTROL_X, row_y(1) + NUMBER_BACKGROUND_Y_OFFSET),
        NUMBER_FIELD_WIDTH,
        NUMBER_BACKGROUND_HEIGHT,
        mtm,
    );

    let visible_count_label =
        NSTextField::labelWithString(&NSString::from_str(t(lang, "visible_count")), mtm);
    visible_count_label.setFrame(NSRect::new(
        NSPoint::new(LABEL_X, row_y(2)),
        NSSize::new(LABEL_WIDTH, 24.0),
    ));
    let visible_count_field = NSTextField::initWithFrame(
        NSTextField::alloc(mtm),
        NSRect::new(
            NSPoint::new(CONTROL_X, row_y(2) + NUMBER_FIELD_Y_OFFSET),
            NSSize::new(NUMBER_FIELD_WIDTH, NUMBER_FIELD_HEIGHT),
        ),
    );
    style_preferences_number_field(&visible_count_field);
    visible_count_field.setStringValue(&NSString::from_str(
        &settings.visible_history_items.to_string(),
    ));
    let visible_count_background = preferences_number_background(
        NSPoint::new(CONTROL_X, row_y(2) + NUMBER_BACKGROUND_Y_OFFSET),
        NUMBER_FIELD_WIDTH,
        NUMBER_BACKGROUND_HEIGHT,
        mtm,
    );

    let rich_history_limit_label =
        NSTextField::labelWithString(&NSString::from_str(t(lang, "rich_history_limit")), mtm);
    rich_history_limit_label.setFrame(NSRect::new(
        NSPoint::new(LABEL_X, row_y(3)),
        NSSize::new(LABEL_WIDTH, 24.0),
    ));
    let rich_history_limit_field = NSTextField::initWithFrame(
        NSTextField::alloc(mtm),
        NSRect::new(
            NSPoint::new(CONTROL_X, row_y(3) + NUMBER_FIELD_Y_OFFSET),
            NSSize::new(NUMBER_FIELD_WIDTH, NUMBER_FIELD_HEIGHT),
        ),
    );
    style_preferences_number_field(&rich_history_limit_field);
    rich_history_limit_field.setStringValue(&NSString::from_str(
        &settings.max_rich_history_items.to_string(),
    ));
    let rich_history_limit_background = preferences_number_background(
        NSPoint::new(CONTROL_X, row_y(3) + NUMBER_BACKGROUND_Y_OFFSET),
        NUMBER_FIELD_WIDTH,
        NUMBER_BACKGROUND_HEIGHT,
        mtm,
    );

    let rich_visible_count_label =
        NSTextField::labelWithString(&NSString::from_str(t(lang, "rich_visible_count")), mtm);
    rich_visible_count_label.setFrame(NSRect::new(
        NSPoint::new(LABEL_X, row_y(4)),
        NSSize::new(LABEL_WIDTH, 24.0),
    ));
    let rich_visible_count_field = NSTextField::initWithFrame(
        NSTextField::alloc(mtm),
        NSRect::new(
            NSPoint::new(CONTROL_X, row_y(4) + NUMBER_FIELD_Y_OFFSET),
            NSSize::new(NUMBER_FIELD_WIDTH, NUMBER_FIELD_HEIGHT),
        ),
    );
    style_preferences_number_field(&rich_visible_count_field);
    rich_visible_count_field.setStringValue(&NSString::from_str(
        &settings.visible_rich_history_items.to_string(),
    ));
    let rich_visible_count_background = preferences_number_background(
        NSPoint::new(CONTROL_X, row_y(4) + NUMBER_BACKGROUND_Y_OFFSET),
        NUMBER_FIELD_WIDTH,
        NUMBER_BACKGROUND_HEIGHT,
        mtm,
    );

    let menu_width_label =
        NSTextField::labelWithString(&NSString::from_str(t(lang, "menu_width")), mtm);
    menu_width_label.setFrame(NSRect::new(
        NSPoint::new(LABEL_X, row_y(5)),
        NSSize::new(LABEL_WIDTH, 24.0),
    ));
    let menu_width_field = NSTextField::initWithFrame(
        NSTextField::alloc(mtm),
        NSRect::new(
            NSPoint::new(CONTROL_X, row_y(5) + NUMBER_FIELD_Y_OFFSET),
            NSSize::new(NUMBER_FIELD_WIDTH, NUMBER_FIELD_HEIGHT),
        ),
    );
    style_preferences_number_field(&menu_width_field);
    menu_width_field.setStringValue(&NSString::from_str(&settings.menu_width.to_string()));
    let menu_width_background = preferences_number_background(
        NSPoint::new(CONTROL_X, row_y(5) + NUMBER_BACKGROUND_Y_OFFSET),
        NUMBER_FIELD_WIDTH,
        NUMBER_BACKGROUND_HEIGHT,
        mtm,
    );

    let rich_label =
        NSTextField::labelWithString(&NSString::from_str(t(lang, "rich_capture_setting")), mtm);
    rich_label.setFrame(NSRect::new(
        NSPoint::new(LABEL_X, row_y(6)),
        NSSize::new(LABEL_WIDTH, 24.0),
    ));
    let rich_popup = NSPopUpButton::initWithFrame_pullsDown(
        NSPopUpButton::alloc(mtm),
        NSRect::new(
            NSPoint::new(CONTROL_X, row_y(6) - 4.0),
            NSSize::new(CONTROL_WIDTH, 28.0),
        ),
        false,
    );
    rich_popup.addItemWithTitle(&NSString::from_str(t(lang, "enabled")));
    rich_popup.addItemWithTitle(&NSString::from_str(t(lang, "disabled")));
    rich_popup.selectItemAtIndex(if settings.capture_rich_clipboard {
        0
    } else {
        1
    });

    view.addSubview(&language_label);
    view.addSubview(&language_popup);
    view.addSubview(&history_limit_label);
    view.addSubview(&history_limit_background);
    view.addSubview(&history_limit_field);
    view.addSubview(&visible_count_label);
    view.addSubview(&visible_count_background);
    view.addSubview(&visible_count_field);
    view.addSubview(&rich_history_limit_label);
    view.addSubview(&rich_history_limit_background);
    view.addSubview(&rich_history_limit_field);
    view.addSubview(&rich_visible_count_label);
    view.addSubview(&rich_visible_count_background);
    view.addSubview(&rich_visible_count_field);
    view.addSubview(&menu_width_label);
    view.addSubview(&menu_width_background);
    view.addSubview(&menu_width_field);
    view.addSubview(&rich_label);
    view.addSubview(&rich_popup);

    PreferencesControls {
        view,
        history_limit_field,
        visible_count_field,
        rich_history_limit_field,
        rich_visible_count_field,
        menu_width_field,
        language_popup,
        rich_popup,
    }
}

fn read_preferences_controls(controls: &PreferencesControls) -> AppSettings {
    let language = match controls.language_popup.indexOfSelectedItem() {
        1 => Language::Chinese,
        _ => Language::English,
    };
    let max_history_items = parse_positive_usize(
        &nsstring_to_string(&controls.history_limit_field.stringValue()),
        AppSettings::default().max_history_items,
    );
    let visible_history_items = parse_positive_usize(
        &nsstring_to_string(&controls.visible_count_field.stringValue()),
        AppSettings::default().visible_history_items,
    );
    let max_rich_history_items = parse_positive_usize(
        &nsstring_to_string(&controls.rich_history_limit_field.stringValue()),
        AppSettings::default().max_rich_history_items,
    );
    let visible_rich_history_items = parse_positive_usize(
        &nsstring_to_string(&controls.rich_visible_count_field.stringValue()),
        AppSettings::default().visible_rich_history_items,
    );
    let menu_width = parse_positive_usize(
        &nsstring_to_string(&controls.menu_width_field.stringValue()),
        AppSettings::default().menu_width,
    );
    storage::normalize_settings(AppSettings {
        language,
        capture_rich_clipboard: controls.rich_popup.indexOfSelectedItem() == 0,
        max_history_items,
        visible_history_items,
        max_rich_history_items,
        visible_rich_history_items,
        menu_width,
    })
}

fn style_preferences_number_field(field: &NSTextField) {
    field.setBezeled(false);
    field.setBordered(false);
    field.setDrawsBackground(false);
    field.setFocusRingType(NSFocusRingType::None);
    field.setAlignment(NSTextAlignment::Center);
    field.setControlSize(NSControlSize::Regular);
    field.setUsesSingleLineMode(true);
    field.setLineBreakMode(NSLineBreakMode::ByClipping);
    field.setFont(Some(&NSFont::systemFontOfSize(15.0)));
}

fn preferences_number_background(
    origin: NSPoint,
    width: f64,
    height: f64,
    mtm: MainThreadMarker,
) -> Retained<NSView> {
    let view = NSView::initWithFrame(
        NSView::alloc(mtm),
        NSRect::new(origin, NSSize::new(width, height)),
    );
    view.setWantsLayer(true);
    if let Some(layer) = view.layer() {
        layer.setCornerRadius(6.0);
        layer.setMasksToBounds(true);
        layer.setBackgroundColor(Some(&NSColor::colorWithWhite_alpha(1.0, 0.12).CGColor()));
    }
    view
}

fn preferences_button(
    title: &str,
    origin: NSPoint,
    action: objc2::runtime::Sel,
    target: &MenuDelegate,
    mtm: MainThreadMarker,
) -> Retained<NSButton> {
    let button = unsafe {
        NSButton::buttonWithTitle_target_action(
            &NSString::from_str(title),
            Some(as_any_object(target)),
            Some(action),
            mtm,
        )
    };
    button.setFrame(NSRect::new(origin, NSSize::new(120.0, 32.0)));
    button.setBezelStyle(NSBezelStyle::Push);
    button.setButtonType(NSButtonType::MomentaryPushIn);
    button.setFont(Some(&NSFont::systemFontOfSize_weight(14.0, 0.25)));
    button
}

fn parse_positive_usize(value: &str, fallback: usize) -> usize {
    value
        .trim()
        .parse::<usize>()
        .ok()
        .filter(|value| *value > 0)
        .unwrap_or(fallback)
}

fn set_menu_item_tooltip(item: &NSMenuItem, tooltip: &str) {
    item.setToolTip(Some(&NSString::from_str(tooltip)));
}

fn title_units_for_width(menu_width: usize) -> usize {
    menu_width
        .saturating_sub(MENU_WIDTH_TEXT_PADDING)
        .checked_div(MENU_WIDTH_UNIT_PIXELS)
        .unwrap_or(0)
        .max(MIN_PREVIEW_UNITS)
}

fn preview_units_for_prefix(menu_width: usize, prefix: &str) -> usize {
    title_units_for_width(menu_width)
        .saturating_sub(display_units_for_text(prefix))
        .max(MIN_PREVIEW_UNITS)
}

fn nsstring_to_string(value: &NSString) -> String {
    let ptr = value.UTF8String();
    if ptr.is_null() {
        String::new()
    } else {
        unsafe { std::ffi::CStr::from_ptr(ptr) }
            .to_string_lossy()
            .into_owned()
    }
}

fn adjusted_popup_location(
    location: NSPoint,
    menu_item_count: usize,
    mtm: MainThreadMarker,
) -> NSPoint {
    let Some(visible_frame) = visible_frame_for_point(location, mtm) else {
        return location;
    };
    let estimated_height =
        menu_item_count as f64 * MENU_ITEM_HEIGHT_ESTIMATE + MENU_VERTICAL_PADDING_ESTIMATE;
    adjusted_popup_location_for_frame(location, visible_frame, estimated_height)
}

fn adjusted_popup_location_for_frame(
    mut location: NSPoint,
    visible_frame: NSRect,
    menu_height: f64,
) -> NSPoint {
    let bottom = visible_frame.min().y + MENU_SCREEN_MARGIN;
    let top = visible_frame.max().y - MENU_SCREEN_MARGIN;
    let available_below = location.y - bottom;

    if available_below < menu_height {
        location.y += menu_height - available_below;
    }

    if location.y > top {
        location.y = top;
    }
    if location.y < bottom {
        location.y = bottom;
    }

    location
}

fn preview_image_data(entry: &RichHistoryEntry) -> Option<Vec<u8>> {
    const PNG_TYPES: &[&str] = &["public.png", "Apple PNG pasteboard type"];
    const TIFF_TYPES: &[&str] = &["public.tiff", "NeXTTIFFPboardType"];
    for flavor in &entry.flavors {
        if PNG_TYPES.contains(&flavor.type_name.as_str()) {
            return Some(flavor.data.clone());
        }
    }
    for flavor in &entry.flavors {
        if TIFF_TYPES.contains(&flavor.type_name.as_str()) {
            return Some(flavor.data.clone());
        }
    }
    None
}

/// 加载内嵌的 logo PNG，并按状态栏推荐的 18pt 高度生成模板图。
/// 模板图（template image）会在 macOS 菜单栏中按当前主题自动渲染成
/// 黑色（浅色菜单栏）或白色（深色菜单栏）的单色 logo。
fn load_status_bar_image() -> Option<Retained<NSImage>> {
    const STATUS_BAR_ICON_BYTES: &[u8] = include_bytes!("../icons/menubar.png");
    let ns_data = unsafe {
        NSData::dataWithBytes_length(
            STATUS_BAR_ICON_BYTES.as_ptr().cast::<c_void>(),
            STATUS_BAR_ICON_BYTES.len() as _,
        )
    };
    let image = NSImage::initWithData(NSImage::alloc(), &ns_data)?;
    image.setSize(NSSize::new(18.0, 18.0));
    image.setTemplate(true);
    Some(image)
}

fn position_preview_window(window: &NSWindow, mtm: MainThreadMarker) {
    let mouse = NSEvent::mouseLocation();
    let frame = window.frame();
    let size = frame.size;
    let mut origin = NSPoint::new(mouse.x + 16.0, mouse.y - size.height / 2.0);
    if let Some(visible) = visible_frame_for_point(mouse, mtm) {
        if origin.x + size.width > visible.max().x {
            origin.x = mouse.x - 16.0 - size.width;
        }
        if origin.x < visible.min().x {
            origin.x = visible.min().x + MENU_SCREEN_MARGIN;
        }
        if origin.y < visible.min().y {
            origin.y = visible.min().y + MENU_SCREEN_MARGIN;
        }
        if origin.y + size.height > visible.max().y {
            origin.y = visible.max().y - size.height - MENU_SCREEN_MARGIN;
        }
    }
    window.setFrameOrigin(origin);
}

fn position_search_window(window: &NSWindow, mtm: MainThreadMarker) {
    let mouse = NSEvent::mouseLocation();
    let frame = window.frame();
    let size = frame.size;
    let visible = visible_frame_for_point(mouse, mtm)
        .or_else(|| NSScreen::mainScreen(mtm).map(|screen| screen.visibleFrame()));
    if let Some(visible) = visible {
        let origin = NSPoint::new(
            visible.min().x + (visible.size.width - size.width) / 2.0,
            visible.min().y + (visible.size.height - size.height) * 0.62,
        );
        window.setFrameOrigin(origin);
    } else {
        window.center();
    }
}

fn visible_frame_for_point(point: NSPoint, mtm: MainThreadMarker) -> Option<NSRect> {
    let screens = NSScreen::screens(mtm);
    for idx in 0..screens.count() {
        let screen = screens.objectAtIndex(idx);
        let frame = screen.frame();
        if point.x >= frame.min().x
            && point.x <= frame.max().x
            && point.y >= frame.min().y
            && point.y <= frame.max().y
        {
            return Some(screen.visibleFrame());
        }
    }
    NSScreen::mainScreen(mtm).map(|screen| screen.visibleFrame())
}

fn t(language: Language, key: &str) -> &'static str {
    match language {
        Language::English => match key {
            "app_name" => "clipy-rs",
            "hotkey" => "Global hotkey: Cmd+Shift+V",
            "error" => "Error",
            "capture_now" => "Capture Current Clipboard",
            "refresh" => "Refresh Menu",
            "show_menu" => "Show Menu",
            "history" => "History",
            "search_history" => "Search history",
            "search_placeholder" => "Type to search…",
            "search_no_results" => "No matches",
            "load_history_failed" => "Failed to load history",
            "storage_unavailable" => "Storage unavailable",
            "no_history" => "No history yet",
            "preferences" => "Preferences...",
            "preferences_title" => "Preferences",
            "preferences_help" => {
                "Adjust how many items are shown in the menu, language, and clipboard capture. History is never deleted."
            }
            "language" => "Language",
            "history_limit" => "Max text items shown",
            "visible_count" => "Visible recent items",
            "rich_history_limit" => "Max image/file items shown",
            "rich_visible_count" => "Visible image/file items",
            "menu_width" => "Menu width",
            "rich_capture_setting" => "Images and files",
            "enabled" => "Enabled",
            "disabled" => "Disabled",
            "save" => "Save",
            "cancel" => "Cancel",
            "rich_history" => "Images and Files",
            "no_rich_history" => "No images or files yet",
            "kind_image" => "Image",
            "kind_file" => "File",
            "favorites" => "Favorites",
            "no_favorites" => "No favorites yet",
            "add_favorite" => "Add Favorite",
            "remove_favorite" => "Remove Favorite",
            "no_favorite_candidates" => "No history items to favorite",
            "notes" => "Notes",
            "settings" => "Settings",
            "rich_enabled" => "[x] Capture images and files",
            "rich_disabled" => "[ ] Capture images and files",
            "clear_history" => "Clear History",
            "quit" => "Quit",
            "permission_required_title" => "Accessibility Permission Required",
            "permission_required_body" => {
                "Clipy RS needs Accessibility permission to auto-paste. Please enable it in System Settings → Privacy & Security → Accessibility, then try again."
            }
            "open_settings" => "Open Settings",
            _ => "",
        },
        Language::Chinese => match key {
            "app_name" => "clipy-rs",
            "hotkey" => "全局快捷键: Cmd+Shift+V",
            "error" => "错误",
            "capture_now" => "捕获当前剪贴板",
            "refresh" => "刷新菜单",
            "show_menu" => "显示菜单",
            "history" => "文本历史",
            "search_history" => "搜索历史",
            "search_placeholder" => "输入关键词搜索…",
            "search_no_results" => "无匹配结果",
            "load_history_failed" => "加载历史失败",
            "storage_unavailable" => "存储不可用",
            "no_history" => "暂无历史",
            "preferences" => "偏好设置...",
            "preferences_title" => "偏好设置",
            "preferences_help" => {
                "在这里调整菜单中展示的条数、语言以及图片/文件剪贴板捕获。历史记录不会被删除。"
            }
            "language" => "语言",
            "history_limit" => "文本最多展示数",
            "visible_count" => "顶部直接显示",
            "rich_history_limit" => "图片/文件最多展示数",
            "rich_visible_count" => "图片/文件直显数量",
            "menu_width" => "菜单宽度",
            "rich_capture_setting" => "图片/文件剪贴板",
            "enabled" => "开启",
            "disabled" => "关闭",
            "save" => "保存",
            "cancel" => "取消",
            "rich_history" => "图片和文件",
            "no_rich_history" => "暂无图片或文件",
            "kind_image" => "图片",
            "kind_file" => "文件",
            "favorites" => "收藏",
            "no_favorites" => "暂无收藏",
            "add_favorite" => "添加收藏",
            "remove_favorite" => "取消收藏",
            "no_favorite_candidates" => "暂无可收藏历史",
            "notes" => "备忘录",
            "settings" => "设置",
            "rich_enabled" => "[x] 捕获图片和文件",
            "rich_disabled" => "[ ] 捕获图片和文件",
            "clear_history" => "清空历史",
            "quit" => "退出",
            "permission_required_title" => "需要辅助功能权限",
            "permission_required_body" => {
                "Clipy RS 需要辅助功能权限才能自动粘贴。请在 系统设置 → 隐私与安全性 → 辅助功能 中启用 Clipy RS 后重试。"
            }
            "open_settings" => "打开系统设置",
            _ => "",
        },
    }
}

fn preview_with_truncation(text: &str, max_units: usize) -> (String, bool) {
    let max_units = max_units.max(ELLIPSIS_UNITS);
    let mut out = String::with_capacity(max_units.min(text.len()));
    let mut used_units = 0;
    let mut previous_space = false;
    for ch in text.chars() {
        let mapped = if ch.is_control() || ch.is_whitespace() {
            ' '
        } else {
            ch
        };
        if mapped == ' ' {
            if previous_space {
                continue;
            }
            previous_space = true;
        } else {
            previous_space = false;
        }
        let mapped_units = display_units(mapped);
        if used_units + mapped_units > max_units {
            append_ellipsis_within_budget(&mut out, &mut used_units, max_units);
            return (out, true);
        }
        out.push(mapped);
        used_units += mapped_units;
    }
    (out, false)
}

fn append_ellipsis_within_budget(out: &mut String, used_units: &mut usize, max_units: usize) {
    while *used_units + ELLIPSIS_UNITS > max_units {
        let Some(ch) = out.pop() else {
            break;
        };
        *used_units = (*used_units).saturating_sub(display_units(ch));
    }
    out.push_str(ELLIPSIS);
    *used_units += ELLIPSIS_UNITS;
}

fn display_units_for_text(text: &str) -> usize {
    text.chars().map(display_units).sum()
}

fn display_units(ch: char) -> usize {
    if is_wide_char(ch) { 2 } else { 1 }
}

fn is_wide_char(ch: char) -> bool {
    matches!(
        ch as u32,
        0x1100..=0x11FF
            | 0x2E80..=0xA4CF
            | 0xAC00..=0xD7AF
            | 0xF900..=0xFAFF
            | 0xFE10..=0xFE6F
            | 0xFF00..=0xFFEF
            | 0x1F300..=0x1FAFF
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use objc2_foundation::NSSize;

    fn frame(x: f64, y: f64, width: f64, height: f64) -> NSRect {
        NSRect::new(NSPoint::new(x, y), NSSize::new(width, height))
    }

    #[test]
    fn popup_location_moves_up_near_bottom() {
        let visible = frame(0.0, 0.0, 1440.0, 900.0);
        let location = NSPoint::new(500.0, 20.0);
        let adjusted = adjusted_popup_location_for_frame(location, visible, 220.0);

        assert!(adjusted.y > location.y);
        assert!(adjusted.y - MENU_SCREEN_MARGIN >= 220.0);
    }

    #[test]
    fn popup_location_keeps_middle_position() {
        let visible = frame(0.0, 0.0, 1440.0, 900.0);
        let location = NSPoint::new(500.0, 500.0);
        let adjusted = adjusted_popup_location_for_frame(location, visible, 220.0);

        assert_eq!(adjusted, location);
    }

    #[test]
    fn popup_location_clamps_to_visible_top() {
        let visible = frame(0.0, 0.0, 1440.0, 260.0);
        let location = NSPoint::new(500.0, 20.0);
        let adjusted = adjusted_popup_location_for_frame(location, visible, 400.0);

        assert_eq!(adjusted.y, visible.max().y - MENU_SCREEN_MARGIN);
    }

    #[test]
    fn preview_chars_follow_menu_width() {
        assert!(title_units_for_width(180) < title_units_for_width(360));
        assert!(preview_units_for_prefix(180, "10. ") < preview_units_for_prefix(360, "10. "));
        assert_eq!(title_units_for_width(0), MIN_PREVIEW_UNITS);
    }

    #[test]
    fn history_title_respects_width_budget() {
        let entry = HistoryEntry {
            id: 1,
            content: "abcdefghijklmnopqrstuvwxyz".to_string(),
            created_at: 1,
            updated_at: 1,
            use_count: 0,
            pinned: false,
        };

        let (narrow, narrow_truncated) = history_item_title(0, &entry, 180);
        let (wide, wide_truncated) = history_item_title(0, &entry, 360);

        assert!(narrow.chars().count() < wide.chars().count());
        assert!(narrow_truncated);
        assert!(!wide_truncated);
    }

    #[test]
    fn preview_truncation_keeps_ellipsis_within_budget() {
        let (preview, truncated) = preview_with_truncation("abcdef", 5);

        assert!(truncated);
        assert_eq!(display_units_for_text(&preview), 5);
        assert!(preview.ends_with(ELLIPSIS));
    }

    #[test]
    fn wide_characters_consume_more_budget() {
        let (preview, truncated) = preview_with_truncation("苹果电脑abc", 7);

        assert!(truncated);
        assert!(display_units_for_text(&preview) <= 7);
        assert!(preview.ends_with(ELLIPSIS));
    }
}
