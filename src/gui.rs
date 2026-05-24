use std::cell::{Cell, OnceCell, RefCell};
use std::ffi::c_void;
use std::process::Command;
use std::ptr::null_mut;

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{DefinedClass, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSAlert, NSAlertFirstButtonReturn, NSAlertStyle, NSApplication, NSApplicationActivationPolicy,
    NSApplicationDelegate, NSEvent, NSMenu, NSMenuItem, NSPopUpButton, NSScreen, NSStatusBar,
    NSStatusItem, NSTextField, NSVariableStatusItemLength, NSView,
};
use objc2_foundation::{
    MainThreadMarker, NSNotification, NSObject, NSObjectProtocol, NSPoint, NSRect, NSSize,
    NSString, NSTimer,
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
const MENU_ITEM_HEIGHT_ESTIMATE: f64 = 22.0;
const MENU_VERTICAL_PADDING_ESTIMATE: f64 = 16.0;
const MENU_SCREEN_MARGIN: f64 = 8.0;

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
}

define_class!(
    #[unsafe(super = NSObject)]
    #[thread_kind = MainThreadOnly]
    #[ivars = MenuDelegateIvars]
    struct MenuDelegate;

    unsafe impl NSObjectProtocol for MenuDelegate {}

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
                    Err(err) => self.set_error(err),
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
                    Err(err) => self.set_error(err),
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
    }
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
            button.setTitle(&NSString::from_str("Clip"));
        }

        let menu =
            NSMenu::initWithTitle(NSMenu::alloc(self.mtm()), &NSString::from_str("clipy-rs"));
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

        if let Ok(change_count) = clipboard::change_count() {
            if !force_status && self.ivars().last_pasteboard_change.get() == change_count {
                return;
            }
            self.ivars().last_pasteboard_change.set(change_count);
        }

        let settings = self.settings();
        if settings.capture_rich_clipboard {
            match clipboard::read_rich_clipboard(CAPTURE_MAX_RICH_BYTES) {
                Ok(Some(entry)) => {
                    match capture_rich(store, entry, settings.max_history_items) {
                        Ok(CaptureStatus::Changed) => {
                            self.clear_error();
                            self.rebuild_menu();
                        }
                        Ok(CaptureStatus::Unchanged | CaptureStatus::Ignored) => {}
                        Err(err) => self.set_error(err),
                    }
                    return;
                }
                Ok(None) => {}
                Err(err) if force_status => self.set_error(err),
                Err(_) => {}
            }
        }

        match clipboard::read_text() {
            Ok(text) => match capture_text(store, text, settings.max_history_items) {
                Ok(CaptureStatus::Changed) => {
                    self.clear_error();
                    self.rebuild_menu();
                }
                Ok(CaptureStatus::Unchanged | CaptureStatus::Ignored) => {}
                Err(err) => self.set_error(err),
            },
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
        let entries = sorted_history(entries);
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

        if let Some(store) = self.store() {
            match sorted_rich_history(store.load_rich_history().unwrap_or_default()) {
                entries if entries.is_empty() => {
                    self.add_disabled_item(&submenu, t(lang, "no_rich_history"));
                }
                entries => {
                    for entry in entries.iter().take(settings.max_history_items) {
                        let kind = match entry.kind {
                            storage::RichClipboardKind::Image => t(lang, "kind_image"),
                            storage::RichClipboardKind::File => t(lang, "kind_file"),
                        };
                        let prefix = format!("{kind}: ");
                        let preview_units = preview_units_for_prefix(settings.menu_width, &prefix);
                        let (label, truncated) =
                            preview_with_truncation(&entry.label, preview_units);
                        let title = format!("{prefix}{label}");
                        self.add_action_item_with_tooltip(
                            &submenu,
                            &title,
                            sel!(copyRichHistoryItem:),
                            entry.id as isize,
                            truncated.then_some(entry.label.as_str()),
                        );
                    }
                }
            }
        } else {
            self.add_disabled_item(&submenu, t(lang, "storage_unavailable"));
        }

        submenu_item.setSubmenu(Some(&submenu));
        menu.addItem(&submenu_item);
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
        let alert = NSAlert::new(self.mtm());
        alert.setAlertStyle(NSAlertStyle::Informational);
        alert.setMessageText(&NSString::from_str(t(lang, "preferences_title")));
        alert.setInformativeText(&NSString::from_str(t(lang, "preferences_help")));
        alert.setAccessoryView(Some(&controls.view));
        alert.addButtonWithTitle(&NSString::from_str(t(lang, "save")));
        alert.addButtonWithTitle(&NSString::from_str(t(lang, "cancel")));
        alert.layout();

        let app = NSApplication::sharedApplication(self.mtm());
        app.activate();

        if alert.runModal() == NSAlertFirstButtonReturn {
            self.persist_settings(read_preferences_controls(&controls));
        }
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
                Ok(()) => match prune_store_history(store, settings.max_history_items) {
                    Ok(()) => self.clear_error(),
                    Err(err) => self.set_error(err),
                },
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
    menu_width_field: Retained<NSTextField>,
    language_popup: Retained<NSPopUpButton>,
    rich_popup: Retained<NSPopUpButton>,
}

fn capture_text(store: &Store, text: String, max_items: usize) -> Result<CaptureStatus, String> {
    let text = normalize_clipboard_text(text);
    if text.is_empty() || text.len() > CAPTURE_MAX_BYTES || sensitive::looks_sensitive(&text) {
        return Ok(CaptureStatus::Ignored);
    }

    let mut entries = store.load_history()?;
    let inserted = storage::upsert_history(&mut entries, text);
    storage::prune_history(&mut entries, max_items);
    store.save_history(&entries)?;

    if inserted {
        Ok(CaptureStatus::Changed)
    } else {
        Ok(CaptureStatus::Unchanged)
    }
}

fn capture_rich(
    store: &Store,
    entry: RichHistoryEntry,
    max_items: usize,
) -> Result<CaptureStatus, String> {
    let mut entries = store.load_rich_history()?;
    let inserted = storage::upsert_rich_history(&mut entries, entry);
    storage::prune_rich_history(&mut entries, max_items);
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
    clipboard::write_text(&entries[entry_index].content)?;
    if paste {
        clipboard::paste_frontmost()?;
    }
    entries[entry_index].use_count += 1;
    entries[entry_index].updated_at = storage::now_millis();
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

fn adjusted_popup_location(
    location: NSPoint,
    menu_item_count: usize,
    mtm: MainThreadMarker,
) -> NSPoint {
    let Some(visible_frame) = visible_frame_for_point(location, mtm) else {
        return location;
    };
    let estimated_height = estimate_menu_height(menu_item_count);
    adjusted_popup_location_for_frame(location, visible_frame, estimated_height)
}

fn build_preferences_controls(
    settings: AppSettings,
    lang: Language,
    mtm: MainThreadMarker,
) -> PreferencesControls {
    let view = NSView::initWithFrame(
        NSView::alloc(mtm),
        NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(360.0, 216.0)),
    );

    let language_label =
        NSTextField::labelWithString(&NSString::from_str(t(lang, "language")), mtm);
    language_label.setFrame(NSRect::new(
        NSPoint::new(0.0, 176.0),
        NSSize::new(120.0, 24.0),
    ));
    let language_popup = NSPopUpButton::initWithFrame_pullsDown(
        NSPopUpButton::alloc(mtm),
        NSRect::new(NSPoint::new(150.0, 172.0), NSSize::new(180.0, 28.0)),
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
        NSPoint::new(0.0, 136.0),
        NSSize::new(140.0, 24.0),
    ));
    let history_limit_field = NSTextField::initWithFrame(
        NSTextField::alloc(mtm),
        NSRect::new(NSPoint::new(150.0, 132.0), NSSize::new(90.0, 24.0)),
    );
    history_limit_field
        .setStringValue(&NSString::from_str(&settings.max_history_items.to_string()));

    let visible_count_label =
        NSTextField::labelWithString(&NSString::from_str(t(lang, "visible_count")), mtm);
    visible_count_label.setFrame(NSRect::new(
        NSPoint::new(0.0, 96.0),
        NSSize::new(140.0, 24.0),
    ));
    let visible_count_field = NSTextField::initWithFrame(
        NSTextField::alloc(mtm),
        NSRect::new(NSPoint::new(150.0, 92.0), NSSize::new(90.0, 24.0)),
    );
    visible_count_field.setStringValue(&NSString::from_str(
        &settings.visible_history_items.to_string(),
    ));

    let menu_width_label =
        NSTextField::labelWithString(&NSString::from_str(t(lang, "menu_width")), mtm);
    menu_width_label.setFrame(NSRect::new(
        NSPoint::new(0.0, 56.0),
        NSSize::new(140.0, 24.0),
    ));
    let menu_width_field = NSTextField::initWithFrame(
        NSTextField::alloc(mtm),
        NSRect::new(NSPoint::new(150.0, 52.0), NSSize::new(90.0, 24.0)),
    );
    menu_width_field.setStringValue(&NSString::from_str(&settings.menu_width.to_string()));

    let rich_label =
        NSTextField::labelWithString(&NSString::from_str(t(lang, "rich_capture_setting")), mtm);
    rich_label.setFrame(NSRect::new(
        NSPoint::new(0.0, 16.0),
        NSSize::new(140.0, 24.0),
    ));
    let rich_popup = NSPopUpButton::initWithFrame_pullsDown(
        NSPopUpButton::alloc(mtm),
        NSRect::new(NSPoint::new(150.0, 12.0), NSSize::new(180.0, 28.0)),
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
    view.addSubview(&history_limit_field);
    view.addSubview(&visible_count_label);
    view.addSubview(&visible_count_field);
    view.addSubview(&menu_width_label);
    view.addSubview(&menu_width_field);
    view.addSubview(&rich_label);
    view.addSubview(&rich_popup);

    PreferencesControls {
        view,
        history_limit_field,
        visible_count_field,
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
    let menu_width = parse_positive_usize(
        &nsstring_to_string(&controls.menu_width_field.stringValue()),
        AppSettings::default().menu_width,
    );
    storage::normalize_settings(AppSettings {
        language,
        capture_rich_clipboard: controls.rich_popup.indexOfSelectedItem() == 0,
        max_history_items,
        visible_history_items,
        menu_width,
    })
}

fn prune_store_history(store: &Store, max_items: usize) -> Result<(), String> {
    let mut history = store.load_history()?;
    storage::prune_history(&mut history, max_items);
    store.save_history(&history)?;

    let mut rich_history = store.load_rich_history()?;
    storage::prune_rich_history(&mut rich_history, max_items);
    store.save_rich_history(&rich_history)
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

fn estimate_menu_height(item_count: usize) -> f64 {
    item_count as f64 * MENU_ITEM_HEIGHT_ESTIMATE + MENU_VERTICAL_PADDING_ESTIMATE
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
            "load_history_failed" => "Failed to load history",
            "storage_unavailable" => "Storage unavailable",
            "no_history" => "No history yet",
            "preferences" => "Preferences...",
            "preferences_title" => "Preferences",
            "preferences_help" => "Update the menu layout and clipboard capture behavior.",
            "language" => "Language",
            "history_limit" => "History limit",
            "visible_count" => "Visible recent items",
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
            "load_history_failed" => "加载历史失败",
            "storage_unavailable" => "存储不可用",
            "no_history" => "暂无历史",
            "preferences" => "偏好设置...",
            "preferences_title" => "偏好设置",
            "preferences_help" => "在这里调整历史显示数量、语言以及图片/文件剪贴板捕获。",
            "language" => "语言",
            "history_limit" => "历史条数上限",
            "visible_count" => "顶部直接显示",
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
