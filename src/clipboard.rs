use std::ffi::{CStr, c_void};
use std::io::Write;
#[cfg(not(target_os = "macos"))]
use std::process::{Command, Stdio};
use std::ptr::NonNull;

use crate::storage::{RichClipboardFlavor, RichClipboardKind, RichHistoryEntry};

use crate::storage::{RichClipboardFlavor, RichClipboardKind, RichHistoryEntry};

#[cfg(target_os = "macos")]
pub fn read_text() -> Result<String, String> {
    use objc2_app_kit::{NSPasteboard, NSPasteboardTypeString};

    let pasteboard = NSPasteboard::generalPasteboard();
    let text_type = unsafe { NSPasteboardTypeString };
    Ok(pasteboard
        .stringForType(text_type)
        .map(|value| nsstring_to_string(&value))
        .unwrap_or_default())
}

#[cfg(not(target_os = "macos"))]
pub fn read_text() -> Result<String, String> {
    let output = Command::new("pbpaste")
        .output()
        .map_err(|err| format!("failed to run pbpaste: {err}"))?;
    if !output.status.success() {
        return Err(format!(
            "pbpaste exited with status {}",
            output
                .status
                .code()
                .map_or_else(|| "signal".to_string(), |code| code.to_string())
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

#[cfg(target_os = "macos")]
pub fn change_count() -> Result<i64, String> {
    use objc2_app_kit::NSPasteboard;

    Ok(NSPasteboard::generalPasteboard().changeCount() as i64)
}

#[cfg(not(target_os = "macos"))]
pub fn change_count() -> Result<i64, String> {
    Err("rich clipboard support is only available on macOS".to_string())
}

#[cfg(target_os = "macos")]
pub fn read_rich_clipboard(max_bytes: usize) -> Result<Option<RichHistoryEntry>, String> {
    use objc2::rc::Retained;
    use objc2_app_kit::NSPasteboard;
    use objc2_foundation::NSString;

    let pasteboard = NSPasteboard::generalPasteboard();
    let Some(types) = pasteboard.types() else {
        return Ok(None);
    };

    let mut type_names = Vec::new();
    for idx in 0..types.count() {
        let ty = types.objectAtIndex(idx);
        type_names.push(nsstring_to_string(&ty));
    }

    let kind = if type_names.iter().any(|name| is_file_type(name)) {
        RichClipboardKind::File
    } else if type_names.iter().any(|name| is_image_type(name)) {
        RichClipboardKind::Image
    } else {
        return Ok(None);
    };

    let mut flavors = Vec::new();
    let mut total = 0usize;
    for idx in 0..types.count() {
        let ty: Retained<NSString> = types.objectAtIndex(idx);
        let type_name = nsstring_to_string(&ty);
        if !is_supported_rich_type(kind, &type_name) {
            continue;
        }
        let Some(data) = pasteboard.dataForType(&ty) else {
            continue;
        };
        let bytes = nsdata_to_vec(&data)?;
        total = total.saturating_add(bytes.len());
        if total > max_bytes {
            return Err(format!(
                "rich clipboard item is larger than {max_bytes} bytes"
            ));
        }
        flavors.push(RichClipboardFlavor {
            type_name,
            data: bytes,
        });
    }

    if flavors.is_empty() {
        return Ok(None);
    }

    let label = rich_label(kind, &pasteboard, &flavors);
    Ok(Some(RichHistoryEntry {
        id: 0,
        kind,
        label,
        flavors,
        created_at: 0,
        updated_at: 0,
        use_count: 0,
        pinned: false,
    }))
}

#[cfg(not(target_os = "macos"))]
pub fn read_rich_clipboard(_max_bytes: usize) -> Result<Option<RichHistoryEntry>, String> {
    Ok(None)
}

#[cfg(target_os = "macos")]
pub fn write_rich_clipboard(entry: &RichHistoryEntry) -> Result<(), String> {
    use objc2_app_kit::NSPasteboard;
    use objc2_foundation::{NSData, NSString};

    let pasteboard = NSPasteboard::generalPasteboard();
    pasteboard.clearContents();

    let type_strings: Vec<_> = entry
        .flavors
        .iter()
        .map(|flavor| NSString::from_str(&flavor.type_name))
        .collect();
    let type_array = nsstring_array(&type_strings);
    unsafe {
        pasteboard.declareTypes_owner(&type_array, None);
    }

    for (flavor, ty) in entry.flavors.iter().zip(type_strings.iter()) {
        let data = unsafe {
            NSData::dataWithBytes_length(
                flavor.data.as_ptr().cast::<c_void>(),
                flavor.data.len() as _,
            )
        };
        if !pasteboard.setData_forType(Some(&data), ty) {
            return Err(format!(
                "failed to restore pasteboard type {}",
                flavor.type_name
            ));
        }
    }

    Ok(())
}

#[cfg(not(target_os = "macos"))]
pub fn write_rich_clipboard(_entry: &RichHistoryEntry) -> Result<(), String> {
    Err("rich clipboard support is only available on macOS".to_string())
}

pub fn write_text(text: &str) -> Result<(), String> {
    let mut child = Command::new("pbcopy")
        .stdin(Stdio::piped())
        .spawn()
        .map_err(|err| format!("failed to run pbcopy: {err}"))?;
    child
        .stdin
        .as_mut()
        .ok_or_else(|| "failed to open pbcopy stdin".to_string())?
        .write_all(text.as_bytes())
        .map_err(|err| format!("failed to write clipboard text: {err}"))?;
    let status = child
        .wait()
        .map_err(|err| format!("failed to wait for pbcopy: {err}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!(
            "pbcopy exited with status {}",
            status
                .code()
                .map_or_else(|| "signal".to_string(), |code| code.to_string())
        ))
    }
}

#[cfg(target_os = "macos")]
pub fn paste_frontmost() -> Result<(), String> {
    const KEY_CODE_V: u16 = 0x09;
    const K_CG_EVENT_FLAG_MASK_COMMAND: u64 = 1 << 20;

    if unsafe { !AXIsProcessTrusted() } {
        return Err(
            "paste failed; grant Accessibility permission to Clipy RS or the launching terminal"
                .to_string(),
        );
    }

    post_key(KEY_CODE_V, K_CG_EVENT_FLAG_MASK_COMMAND)?;

    Ok(())
}

#[cfg(not(target_os = "macos"))]
pub fn paste_frontmost() -> Result<(), String> {
    Err("paste integration is only available on macOS".to_string())
}

#[cfg(target_os = "macos")]
pub fn move_cursor_left(count: usize) -> Result<(), String> {
    const KEY_CODE_LEFT_ARROW: u16 = 0x7B;

    if count == 0 {
        return Ok(());
    }

    if unsafe { !AXIsProcessTrusted() } {
        return Err(
            "cursor placement failed; grant Accessibility permission to Clipy RS or the launching terminal"
                .to_string(),
        );
    }

    for _ in 0..count {
        post_key(KEY_CODE_LEFT_ARROW, 0)?;
    }

    Ok(())
}

#[cfg(not(target_os = "macos"))]
pub fn move_cursor_left(_count: usize) -> Result<(), String> {
    Err("cursor placement is only available on macOS".to_string())
}

#[cfg(target_os = "macos")]
fn post_key(key_code: u16, flags: u64) -> Result<(), String> {
    const K_CG_HID_EVENT_TAP: u32 = 0;

    let key_down = unsafe { CGEventCreateKeyboardEvent(std::ptr::null_mut(), key_code, true) };
    let key_up = unsafe { CGEventCreateKeyboardEvent(std::ptr::null_mut(), key_code, false) };
    if key_down.is_null() || key_up.is_null() {
        unsafe {
            release_if_present(key_down);
            release_if_present(key_up);
        }
        return Err("failed to create keyboard event".to_string());
    }

    unsafe {
        CGEventSetFlags(key_down, flags);
        CGEventSetFlags(key_up, flags);
        CGEventPost(K_CG_HID_EVENT_TAP, key_down);
        CGEventPost(K_CG_HID_EVENT_TAP, key_up);
        CFRelease(key_down.cast_const());
        CFRelease(key_up.cast_const());
    }

    Ok(())
}

#[cfg(target_os = "macos")]
#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGEventCreateKeyboardEvent(
        source: *mut c_void,
        virtual_key: u16,
        key_down: bool,
    ) -> *mut c_void;
    fn CGEventSetFlags(event: *mut c_void, flags: u64);
    fn CGEventPost(tap: u32, event: *mut c_void);
}

#[cfg(target_os = "macos")]
#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFRelease(cf: *const c_void);
}

#[cfg(target_os = "macos")]
#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn AXIsProcessTrusted() -> bool;
}

#[cfg(target_os = "macos")]
unsafe fn release_if_present(value: *mut c_void) {
    if !value.is_null() {
        unsafe { CFRelease(value.cast_const()) };
    }
}

#[cfg(target_os = "macos")]
fn nsstring_to_string(value: &objc2_foundation::NSString) -> String {
    let ptr = value.UTF8String();
    if ptr.is_null() {
        String::new()
    } else {
        unsafe { CStr::from_ptr(ptr) }
            .to_string_lossy()
            .into_owned()
    }
}

#[cfg(target_os = "macos")]
fn nsdata_to_vec(data: &objc2_foundation::NSData) -> Result<Vec<u8>, String> {
    let len = data.length();
    let mut bytes = vec![0u8; len];
    if len == 0 {
        return Ok(bytes);
    }
    let ptr = NonNull::new(bytes.as_mut_ptr().cast::<c_void>())
        .ok_or_else(|| "failed to allocate clipboard data buffer".to_string())?;
    unsafe {
        data.getBytes_length(ptr, len as _);
    }
    Ok(bytes)
}

#[cfg(target_os = "macos")]
fn nsstring_array(
    values: &[objc2::rc::Retained<objc2_foundation::NSString>],
) -> objc2::rc::Retained<objc2_foundation::NSArray<objc2_foundation::NSString>> {
    use objc2::AnyThread;
    use objc2_foundation::NSArray;

    let mut objects: Vec<NonNull<objc2_foundation::NSString>> = values
        .iter()
        .map(|value| NonNull::from(value.as_ref()))
        .collect();
    unsafe {
        NSArray::initWithObjects_count(NSArray::alloc(), objects.as_mut_ptr(), objects.len() as _)
    }
}

#[cfg(target_os = "macos")]
fn is_image_type(name: &str) -> bool {
    matches!(
        name,
        "public.png"
            | "public.tiff"
            | "com.adobe.pdf"
            | "Apple PNG pasteboard type"
            | "NeXT TIFF v4.0 pasteboard type"
    )
}

#[cfg(target_os = "macos")]
fn is_file_type(name: &str) -> bool {
    matches!(name, "public.file-url" | "NSFilenamesPboardType")
}

#[cfg(target_os = "macos")]
fn is_supported_rich_type(kind: RichClipboardKind, name: &str) -> bool {
    match kind {
        RichClipboardKind::Image => is_image_type(name),
        RichClipboardKind::File => is_file_type(name) || name == "public.utf8-plain-text",
    }
}

#[cfg(target_os = "macos")]
fn rich_label(
    kind: RichClipboardKind,
    pasteboard: &objc2_app_kit::NSPasteboard,
    flavors: &[RichClipboardFlavor],
) -> String {
    match kind {
        RichClipboardKind::Image => {
            let bytes: usize = flavors.iter().map(|flavor| flavor.data.len()).sum();
            format!("Image clipboard ({})", human_bytes(bytes))
        }
        RichClipboardKind::File => {
            let file_type = objc2_foundation::NSString::from_str("public.file-url");
            if let Some(value) = pasteboard.stringForType(&file_type) {
                format!("File clipboard: {}", nsstring_to_string(&value))
            } else {
                "File clipboard".to_string()
            }
        }
    }
}

#[cfg(target_os = "macos")]
fn human_bytes(bytes: usize) -> String {
    if bytes < 1024 {
        format!("{bytes}B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1}K", bytes as f64 / 1024.0)
    } else {
        format!("{:.1}M", bytes as f64 / 1024.0 / 1024.0)
    }
}

#[cfg(target_os = "macos")]
fn nsstring_to_string(value: &objc2_foundation::NSString) -> String {
    let ptr = value.UTF8String();
    if ptr.is_null() {
        String::new()
    } else {
        unsafe { CStr::from_ptr(ptr) }
            .to_string_lossy()
            .into_owned()
    }
}

#[cfg(target_os = "macos")]
fn nsdata_to_vec(data: &objc2_foundation::NSData) -> Result<Vec<u8>, String> {
    let len = data.length();
    let mut bytes = vec![0u8; len];
    if len == 0 {
        return Ok(bytes);
    }
    let ptr = NonNull::new(bytes.as_mut_ptr().cast::<c_void>())
        .ok_or_else(|| "failed to allocate clipboard data buffer".to_string())?;
    unsafe {
        data.getBytes_length(ptr, len as _);
    }
    Ok(bytes)
}

#[cfg(target_os = "macos")]
fn nsstring_array(
    values: &[objc2::rc::Retained<objc2_foundation::NSString>],
) -> objc2::rc::Retained<objc2_foundation::NSArray<objc2_foundation::NSString>> {
    use objc2::AnyThread;
    use objc2_foundation::NSArray;

    let mut objects: Vec<NonNull<objc2_foundation::NSString>> = values
        .iter()
        .map(|value| NonNull::from(value.as_ref()))
        .collect();
    unsafe {
        NSArray::initWithObjects_count(NSArray::alloc(), objects.as_mut_ptr(), objects.len() as _)
    }
}

#[cfg(target_os = "macos")]
fn is_image_type(name: &str) -> bool {
    matches!(
        name,
        "public.png"
            | "public.tiff"
            | "com.adobe.pdf"
            | "Apple PNG pasteboard type"
            | "NeXT TIFF v4.0 pasteboard type"
    )
}

#[cfg(target_os = "macos")]
fn is_file_type(name: &str) -> bool {
    matches!(name, "public.file-url" | "NSFilenamesPboardType")
}

#[cfg(target_os = "macos")]
fn is_supported_rich_type(kind: RichClipboardKind, name: &str) -> bool {
    match kind {
        RichClipboardKind::Image => is_image_type(name),
        RichClipboardKind::File => is_file_type(name) || name == "public.utf8-plain-text",
    }
}

#[cfg(target_os = "macos")]
fn rich_label(
    kind: RichClipboardKind,
    pasteboard: &objc2_app_kit::NSPasteboard,
    flavors: &[RichClipboardFlavor],
) -> String {
    match kind {
        RichClipboardKind::Image => {
            let bytes: usize = flavors.iter().map(|flavor| flavor.data.len()).sum();
            format!("Image clipboard ({})", human_bytes(bytes))
        }
        RichClipboardKind::File => {
            let file_type = objc2_foundation::NSString::from_str("public.file-url");
            if let Some(value) = pasteboard.stringForType(&file_type) {
                format!("File clipboard: {}", nsstring_to_string(&value))
            } else {
                "File clipboard".to_string()
            }
        }
    }
}

#[cfg(target_os = "macos")]
fn human_bytes(bytes: usize) -> String {
    if bytes < 1024 {
        format!("{bytes}B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1}K", bytes as f64 / 1024.0)
    } else {
        format!("{:.1}M", bytes as f64 / 1024.0 / 1024.0)
    }
}
