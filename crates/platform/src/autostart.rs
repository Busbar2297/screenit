//! Start with Windows: a value under `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`.
//! Works for the portable EXE too, because it records the running executable's path.

#![allow(unsafe_code)]

use windows_sys::Win32::Foundation::ERROR_SUCCESS;
use windows_sys::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_QUERY_VALUE, KEY_SET_VALUE, REG_OPTION_NON_VOLATILE, REG_SZ,
    RegCloseKey, RegCreateKeyExW, RegDeleteValueW, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW,
};

const RUN_KEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";
/// Value name used by the app.
pub const APP_VALUE: &str = "screenIt";
/// Argument that starts the app hidden in the tray.
pub const MINIMIZED_ARG: &str = "--minimized";

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// The command line stored in the Run key.
pub fn command_line(exe: &std::path::Path, minimized: bool) -> String {
    if minimized {
        format!("\"{}\" {MINIMIZED_ARG}", exe.display())
    } else {
        format!("\"{}\"", exe.display())
    }
}

/// Open the Run key; `write` asks for set access and creates it when missing.
fn open(write: bool) -> Option<HKEY> {
    let key = wide(RUN_KEY);
    let mut h: HKEY = std::ptr::null_mut();
    // SAFETY: `key` is NUL-terminated and outlives the call; `h` is a valid out-pointer.
    let rc = unsafe {
        if write {
            RegCreateKeyExW(
                HKEY_CURRENT_USER,
                key.as_ptr(),
                0,
                std::ptr::null(),
                REG_OPTION_NON_VOLATILE,
                KEY_SET_VALUE | KEY_QUERY_VALUE,
                std::ptr::null(),
                &mut h,
                std::ptr::null_mut(),
            )
        } else {
            RegOpenKeyExW(HKEY_CURRENT_USER, key.as_ptr(), 0, KEY_QUERY_VALUE, &mut h)
        }
    };
    (rc == ERROR_SUCCESS).then_some(h)
}

/// Create or update the Run value `name`.
pub fn enable_as(name: &str, command: &str) -> bool {
    let Some(h) = open(true) else {
        return false;
    };
    let (n, v) = (wide(name), wide(command));
    let Ok(bytes) = u32::try_from(v.len() * 2) else {
        // SAFETY: `h` was opened above.
        unsafe { RegCloseKey(h) };
        return false;
    };
    // SAFETY: `n`/`v` are NUL-terminated UTF-16 and `bytes` is exactly the byte length of `v`.
    let rc = unsafe { RegSetValueExW(h, n.as_ptr(), 0, REG_SZ, v.as_ptr().cast(), bytes) };
    // SAFETY: `h` is an open key.
    unsafe { RegCloseKey(h) };
    rc == ERROR_SUCCESS
}

/// Remove the Run value `name`; true when it is gone afterwards.
pub fn disable_as(name: &str) -> bool {
    let Some(h) = open(true) else {
        return true;
    };
    let n = wide(name);
    // SAFETY: `n` is NUL-terminated; `h` is an open key.
    let rc = unsafe { RegDeleteValueW(h, n.as_ptr()) };
    // SAFETY: `h` is an open key.
    unsafe { RegCloseKey(h) };
    rc == ERROR_SUCCESS || !is_enabled_as(name)
}

/// The stored command line for `name`, if any.
pub fn query_as(name: &str) -> Option<String> {
    let h = open(false)?;
    let n = wide(name);
    let mut size = 0u32;
    // SAFETY: size query only; null data pointer is allowed.
    let rc = unsafe {
        RegQueryValueExW(
            h,
            n.as_ptr(),
            std::ptr::null(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut size,
        )
    };
    if rc != ERROR_SUCCESS || size == 0 || size > 65_536 {
        // SAFETY: `h` is an open key.
        unsafe { RegCloseKey(h) };
        return None;
    }
    let mut buf = vec![0u16; (size as usize).div_ceil(2)];
    // SAFETY: `buf` holds at least `size` bytes.
    let rc = unsafe {
        RegQueryValueExW(
            h,
            n.as_ptr(),
            std::ptr::null(),
            std::ptr::null_mut(),
            buf.as_mut_ptr().cast(),
            &mut size,
        )
    };
    // SAFETY: `h` is an open key.
    unsafe { RegCloseKey(h) };
    if rc != ERROR_SUCCESS {
        return None;
    }
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    Some(String::from_utf16_lossy(&buf[..end]))
}

pub fn is_enabled_as(name: &str) -> bool {
    query_as(name).is_some()
}

/// Turn start-with-Windows on or off for the running executable.
pub fn set(enabled: bool, minimized: bool) -> bool {
    if !enabled {
        return disable_as(APP_VALUE);
    }
    match std::env::current_exe() {
        Ok(exe) => enable_as(APP_VALUE, &command_line(&exe, minimized)),
        Err(_) => false,
    }
}

/// Keep the stored command current (the exe may have moved, or the minimized flag changed).
pub fn refresh(enabled: bool, minimized: bool) {
    if !enabled {
        return;
    }
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let want = command_line(&exe, minimized);
    if query_as(APP_VALUE).as_deref() != Some(want.as_str()) {
        enable_as(APP_VALUE, &want);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_key_round_trip() {
        let name = format!("screenIt.test.{}", std::process::id());
        assert!(!is_enabled_as(&name));
        let cmd = command_line(
            std::path::Path::new("C:\\Apps\\screenIt\\screenit.exe"),
            true,
        );
        assert!(enable_as(&name, &cmd));
        assert_eq!(query_as(&name).as_deref(), Some(cmd.as_str()));
        assert!(cmd.ends_with("--minimized"));
        assert!(disable_as(&name));
        assert!(!is_enabled_as(&name));
        assert!(disable_as(&name), "disabling twice is fine");
    }

    #[test]
    fn command_line_quotes_paths_with_spaces() {
        let c = command_line(std::path::Path::new("C:\\Program Files\\x.exe"), false);
        assert_eq!(c, "\"C:\\Program Files\\x.exe\"");
    }
}
