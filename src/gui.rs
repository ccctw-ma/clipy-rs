use std::cell::{Cell, OnceCell, RefCell};
use std::cmp::Reverse;
use std::ffi::c_void;
use std::process::Command;
use std::ptr::null_mut;

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{DefinedClass, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSApplicationDelegate, NSMenu, NSMenuItem,
    NSStatusBar, NSStatusItem, NSVariableStatusItemLength,
};
use objc2_foundation::{
    MainThreadMarker, NSNotification, NSObject, NSObjectProtocol, NSString, NSTimer,
};

use crate::clipboard;
use crate::sensitive;
use crate::storage::{self, HistoryEntry, SnippetEntry, Store};

const MENU_HISTORY_LIMIT: usize = 20;
const CAPTURE_MAX_ITEMS: usize = 100;
const CAPTURE_MAX_BYTES: usize = 256 * 1024;

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
    last_clipboard: RefCell<String>,
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

        #[unsafe(method(copySnippetItem:))]
        fn copy_snippet_item(&self, sender: &NSMenuItem) {
            let index = sender.tag();
            if index <= 0 {
                self.set_error("invalid snippet index".to_string());
                return;
            }

            if let Some(store) = self.store() {
                match copy_snippet_by_index(store, index as usize, true) {
                    Ok(()) => self.clear_error(),
                    Err(err) => self.set_error(err),
                }
            }
            self.rebuild_menu();
        }

        #[unsafe(method(clearHistory:))]
        fn clear_history(&self, _sender: &NSMenuItem) {
            if let Some(store) = self.store() {
                match store.save_history(&[]) {
                    Ok(()) => self.clear_error(),
                    Err(err) => self.set_error(err),
                }
            }
            self.rebuild_menu();
        }

        #[unsafe(method(openDataDirectory:))]
        fn open_data_directory(&self, _sender: &NSMenuItem) {
            if let Some(store) = self.store()
                && let Err(err) = Command::new("open").arg(store.root()).status()
            {
                self.set_error(format!("failed to open data directory: {err}"));
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
        self.capture_current_clipboard(false);
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

        match clipboard::read_text() {
            Ok(text) => {
                if !force_status && *self.ivars().last_clipboard.borrow() == text {
                    return;
                }
                *self.ivars().last_clipboard.borrow_mut() = text.clone();
                match capture_text(store, text) {
                    Ok(CaptureStatus::Changed) => {
                        self.clear_error();
                        self.rebuild_menu();
                    }
                    Ok(CaptureStatus::Unchanged | CaptureStatus::Ignored) => {}
                    Err(err) => self.set_error(err),
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

        self.add_disabled_item(&menu, "clipy-rs");
        self.add_disabled_item(&menu, "Global hotkey: Cmd+Shift+V");
        if let Some(err) = self.ivars().last_error.borrow().as_ref() {
            self.add_disabled_item(&menu, &format!("Error: {}", preview(err, 60)));
        }
        menu.addItem(&NSMenuItem::separatorItem(self.mtm()));

        self.add_action_item(&menu, "Capture Current Clipboard", sel!(captureNow:), 0);
        self.add_action_item(&menu, "Refresh Menu", sel!(refreshMenu:), 0);
        self.add_action_item(&menu, "Show Menu", sel!(showClipyMenu:), 0);
        menu.addItem(&NSMenuItem::separatorItem(self.mtm()));

        self.add_disabled_item(&menu, "History");
        if let Some(store) = self.store() {
            match store.load_history() {
                Ok(entries) => self.add_history_items(&menu, entries),
                Err(err) => {
                    self.add_disabled_item(&menu, &format!("Failed to load history: {err}"))
                }
            }
        } else {
            self.add_disabled_item(&menu, "Storage unavailable");
        }

        menu.addItem(&NSMenuItem::separatorItem(self.mtm()));
        self.add_snippets_menu(&menu);
        menu.addItem(&NSMenuItem::separatorItem(self.mtm()));

        self.add_action_item(&menu, "Clear History", sel!(clearHistory:), 0);
        self.add_action_item(&menu, "Open Data Directory", sel!(openDataDirectory:), 0);
        self.add_action_item(&menu, "Quit", sel!(quit:), 0);
    }

    fn add_history_items(&self, menu: &NSMenu, entries: Vec<HistoryEntry>) {
        let entries = sorted_history(entries);
        if entries.is_empty() {
            self.add_disabled_item(menu, "No history yet");
            return;
        }

        for (idx, entry) in entries.iter().take(MENU_HISTORY_LIMIT).enumerate() {
            let pin = if entry.pinned { "* " } else { "" };
            let title = format!("{:>2}. {}{}", idx + 1, pin, preview(&entry.content, 72));
            self.add_action_item(menu, &title, sel!(copyHistoryItem:), entry.id as isize);
        }
    }

    fn add_snippets_menu(&self, menu: &NSMenu) {
        let submenu_item = self.new_disabled_item("Snippets");
        let submenu =
            NSMenu::initWithTitle(NSMenu::alloc(self.mtm()), &NSString::from_str("Snippets"));

        if let Some(store) = self.store() {
            match sorted_snippets(store.load_snippets().unwrap_or_default()) {
                snippets if snippets.is_empty() => {
                    self.add_disabled_item(&submenu, "No snippets");
                }
                snippets => {
                    for (idx, snippet) in snippets.iter().take(MENU_HISTORY_LIMIT).enumerate() {
                        let title = format!(
                            "{} - {}",
                            preview(&snippet.name, 24),
                            preview(&snippet.content, 52)
                        );
                        self.add_action_item(
                            &submenu,
                            &title,
                            sel!(copySnippetItem:),
                            (idx + 1) as isize,
                        );
                    }
                }
            }
        } else {
            self.add_disabled_item(&submenu, "Storage unavailable");
        }

        submenu_item.setSubmenu(Some(&submenu));
        menu.addItem(&submenu_item);
    }

    fn add_disabled_item(&self, menu: &NSMenu, title: &str) {
        let item = self.new_disabled_item(title);
        menu.addItem(&item);
    }

    fn new_disabled_item(&self, title: &str) -> Retained<NSMenuItem> {
        let item = unsafe {
            NSMenuItem::initWithTitle_action_keyEquivalent(
                NSMenuItem::alloc(self.mtm()),
                &NSString::from_str(title),
                None,
                &NSString::from_str(""),
            )
        };
        item.setEnabled(false);
        item
    }

    fn add_action_item(&self, menu: &NSMenu, title: &str, action: objc2::runtime::Sel, tag: isize) {
        let item = unsafe {
            NSMenuItem::initWithTitle_action_keyEquivalent(
                NSMenuItem::alloc(self.mtm()),
                &NSString::from_str(title),
                Some(action),
                &NSString::from_str(""),
            )
        };
        item.setTag(tag);
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

    fn store(&self) -> Option<&Store> {
        self.ivars().store.get()
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
        delegate.show_status_menu();
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

fn capture_text(store: &Store, text: String) -> Result<CaptureStatus, String> {
    let text = normalize_clipboard_text(text);
    if text.is_empty() || text.len() > CAPTURE_MAX_BYTES || sensitive::looks_sensitive(&text) {
        return Ok(CaptureStatus::Ignored);
    }

    let mut entries = store.load_history()?;
    let inserted = storage::upsert_history(&mut entries, text);
    storage::prune_history(&mut entries, CAPTURE_MAX_ITEMS);
    store.save_history(&entries)?;

    if inserted {
        Ok(CaptureStatus::Changed)
    } else {
        Ok(CaptureStatus::Unchanged)
    }
}

fn copy_history_entry(store: &Store, id: u64, paste: bool) -> Result<(), String> {
    let mut entries = store.load_history()?;
    let entry = entries
        .iter_mut()
        .find(|entry| entry.id == id)
        .ok_or_else(|| format!("history item `{id}` was not found"))?;
    clipboard::write_text(&entry.content)?;
    entry.use_count += 1;
    entry.updated_at = storage::now_millis();
    store.save_history(&entries)?;
    if paste {
        clipboard::paste_frontmost()?;
    }
    Ok(())
}

fn copy_snippet_by_index(store: &Store, index: usize, paste: bool) -> Result<(), String> {
    let mut snippets = sorted_snippets(store.load_snippets()?);
    let snippet = snippets
        .get_mut(index.saturating_sub(1))
        .ok_or_else(|| format!("snippet index `{index}` was not found"))?;
    clipboard::write_text(&snippet.content)?;
    snippet.use_count += 1;
    snippet.updated_at = storage::now_millis();
    store.save_snippets(&snippets)?;
    if paste {
        clipboard::paste_frontmost()?;
    }
    Ok(())
}

fn normalize_clipboard_text(text: String) -> String {
    text.trim_matches('\0').to_string()
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

fn sorted_snippets(mut snippets: Vec<SnippetEntry>) -> Vec<SnippetEntry> {
    snippets.sort_by_key(|snippet| Reverse(snippet.updated_at));
    snippets
}

fn preview(text: &str, max_chars: usize) -> String {
    let mut out = String::with_capacity(max_chars.min(text.len()));
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
        out.push(mapped);
        if out.chars().count() >= max_chars {
            out.push_str("...");
            break;
        }
    }
    out
}
