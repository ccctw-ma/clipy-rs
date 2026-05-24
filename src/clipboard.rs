use std::io::Write;
use std::process::{Command, Stdio};

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

pub fn paste_frontmost() -> Result<(), String> {
    let status = Command::new("osascript")
        .arg("-e")
        .arg(r#"tell application "System Events" to keystroke "v" using command down"#)
        .status()
        .map_err(|err| format!("failed to run osascript: {err}"))?;
    if status.success() {
        Ok(())
    } else {
        Err("paste failed; grant Accessibility permission to the terminal app".to_string())
    }
}
