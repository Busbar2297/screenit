//! Self-update plumbing: HTTPS downloads through WinHTTP (system proxy, Schannel TLS, no extra
//! crates) and the hand-over to the installer.
//!
//! Installing a per-user MSI over the running app fails because `screenit.exe` is in use, so
//! `launch_installer` starts a temporary copy of the exe with `--apply-update`. That copy waits for
//! this process to exit, runs `msiexec`, and starts the new version.

#![allow(unsafe_code)]

use screenit_update::UpdateHost;
use std::ffi::c_void;
use std::io::Write;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;
use windows_sys::Win32::Networking::WinHttp::{
    WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, WINHTTP_FLAG_SECURE, WINHTTP_QUERY_CONTENT_LENGTH,
    WINHTTP_QUERY_FLAG_NUMBER, WINHTTP_QUERY_STATUS_CODE, WinHttpCloseHandle, WinHttpConnect,
    WinHttpOpen, WinHttpOpenRequest, WinHttpQueryHeaders, WinHttpReadData, WinHttpReceiveResponse,
    WinHttpSendRequest, WinHttpSetTimeouts,
};

/// Argument that makes the app act as the installer helper.
pub const APPLY_ARG: &str = "--apply-update";
const WAIT_PID_ARG: &str = "--wait-pid";
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const DETACHED_PROCESS: u32 = 0x0000_0008;

struct Handle(*mut c_void);

impl Drop for Handle {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: the handle came from a WinHTTP open call and is closed exactly once.
            unsafe { WinHttpCloseHandle(self.0) };
        }
    }
}

/// A sent GET request whose response headers have been received. Field order is drop order:
/// request, then connection, then session.
struct Response {
    req: Handle,
    _conn: Handle,
    _session: Handle,
    content_length: u64,
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// `https://host/path` into `(host, path)`. Only plain HTTPS on port 443 is supported.
fn split_url(url: &str) -> Result<(&str, &str), String> {
    let rest = url
        .strip_prefix("https://")
        .ok_or("only https:// addresses are allowed")?;
    let (host, path) = rest.split_once('/').unwrap_or((rest, ""));
    if host.is_empty() || host.contains([':', '@', ' ']) || url.len() > 2000 {
        return Err("bad update address".into());
    }
    Ok((host, path))
}

fn query_number(req: *mut c_void, what: u32) -> Option<u32> {
    let mut v = 0u32;
    let mut len = 4u32;
    // SAFETY: `v` is a 4-byte buffer and `len` says so; the NUMBER flag makes WinHTTP write a u32.
    let ok = unsafe {
        WinHttpQueryHeaders(
            req,
            what | WINHTTP_QUERY_FLAG_NUMBER,
            std::ptr::null(),
            (&mut v as *mut u32).cast(),
            &mut len,
            std::ptr::null_mut(),
        )
    };
    (ok != 0).then_some(v)
}

fn get(url: &str) -> Result<Response, String> {
    let (host, path) = split_url(url)?;
    let agent = wide(&format!("screenIt/{}", env!("CARGO_PKG_VERSION")));
    // SAFETY: all string arguments are NUL-terminated and outlive each call; every handle is
    // wrapped in `Handle` right away so it is closed on every path.
    unsafe {
        let session = Handle(WinHttpOpen(
            agent.as_ptr(),
            WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
            std::ptr::null(),
            std::ptr::null(),
            0,
        ));
        if session.0.is_null() {
            return Err("cannot start the network session".into());
        }
        // resolve, connect, send, receive (ms)
        WinHttpSetTimeouts(session.0, 10_000, 10_000, 30_000, 30_000);
        let conn = Handle(WinHttpConnect(session.0, wide(host).as_ptr(), 443, 0));
        if conn.0.is_null() {
            return Err(format!("cannot reach {host}"));
        }
        let req = Handle(WinHttpOpenRequest(
            conn.0,
            wide("GET").as_ptr(),
            wide(&format!("/{path}")).as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null(),
            WINHTTP_FLAG_SECURE,
        ));
        if req.0.is_null() {
            return Err("cannot build the request".into());
        }
        let headers = wide("Accept: application/vnd.github+json, application/octet-stream\r\n");
        if WinHttpSendRequest(req.0, headers.as_ptr(), u32::MAX, std::ptr::null(), 0, 0, 0) == 0
            || WinHttpReceiveResponse(req.0, std::ptr::null_mut()) == 0
        {
            return Err(format!("no answer from {host}"));
        }
        let status = query_number(req.0, WINHTTP_QUERY_STATUS_CODE).unwrap_or(0);
        if status != 200 {
            return Err(format!("{host} answered HTTP {status}"));
        }
        let content_length = query_number(req.0, WINHTTP_QUERY_CONTENT_LENGTH).unwrap_or(0);
        Ok(Response {
            req,
            _conn: conn,
            _session: session,
            content_length: u64::from(content_length),
        })
    }
}

/// Read the body in chunks into `sink`, stopping with an error past `max_bytes`.
fn read_body(
    r: &Response,
    max_bytes: u64,
    mut sink: impl FnMut(&[u8]) -> Result<(), String>,
    progress: &dyn Fn(u64, u64),
) -> Result<(), String> {
    if r.content_length > max_bytes {
        return Err("the download is larger than allowed".into());
    }
    let mut buf = vec![0u8; 64 * 1024];
    let mut done = 0u64;
    loop {
        let mut n = 0u32;
        // SAFETY: `buf` has `buf.len()` writable bytes and `n` is a valid out-pointer.
        let ok =
            unsafe { WinHttpReadData(r.req.0, buf.as_mut_ptr().cast(), buf.len() as u32, &mut n) };
        if ok == 0 {
            return Err("the connection dropped during the download".into());
        }
        if n == 0 {
            return Ok(());
        }
        done += u64::from(n);
        if done > max_bytes {
            return Err("the download is larger than allowed".into());
        }
        sink(&buf[..n as usize])?;
        progress(done, r.content_length);
    }
}

pub struct WinUpdateHost;

impl UpdateHost for WinUpdateHost {
    fn get_text(&self, url: &str, max_bytes: u64) -> Result<String, String> {
        let r = get(url)?;
        let mut body = Vec::new();
        read_body(
            &r,
            max_bytes,
            |c| {
                body.extend_from_slice(c);
                Ok(())
            },
            &|_, _| {},
        )?;
        String::from_utf8(body).map_err(|_| "the answer is not text".to_string())
    }

    fn download(
        &self,
        url: &str,
        dest: &Path,
        max_bytes: u64,
        progress: &dyn Fn(u64, u64),
    ) -> Result<(), String> {
        let r = get(url)?;
        let part = dest.with_extension("part");
        let result = (|| {
            let mut f = std::fs::File::create(&part).map_err(|e| e.to_string())?;
            read_body(
                &r,
                max_bytes,
                |c| f.write_all(c).map_err(|e| e.to_string()),
                progress,
            )?;
            f.sync_all().map_err(|e| e.to_string())?;
            drop(f);
            std::fs::rename(&part, dest).map_err(|e| e.to_string())
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(&part);
        }
        result
    }

    fn launch_installer(&self, msi: &Path) -> Result<(), String> {
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        let helper =
            std::env::temp_dir().join(format!("screenit-update-{}.exe", std::process::id()));
        std::fs::copy(&exe, &helper).map_err(|e| format!("cannot prepare the updater: {e}"))?;
        Command::new(&helper)
            .arg(APPLY_ARG)
            .arg(msi)
            .arg(WAIT_PID_ARG)
            .arg(std::process::id().to_string())
            .creation_flags(DETACHED_PROCESS | CREATE_NO_WINDOW)
            .spawn()
            .map(|_| ())
            .map_err(|e| format!("cannot start the updater: {e}"))
    }
}

/// `--apply-update <msi> --wait-pid <pid>` arguments, when the process was started as the helper.
pub fn apply_request(args: &[String]) -> Option<(PathBuf, u32)> {
    let i = args.iter().position(|a| a == APPLY_ARG)?;
    let msi = PathBuf::from(args.get(i + 1)?);
    let pid = args
        .iter()
        .position(|a| a == WAIT_PID_ARG)
        .and_then(|j| args.get(j + 1))
        .and_then(|p| p.parse().ok())
        .unwrap_or(0);
    Some((msi, pid))
}

/// Wait until process `pid` has exited (or `max` passed).
fn wait_for_exit(pid: u32, max: Duration) {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{
        OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject,
    };
    if pid == 0 {
        return;
    }
    // SAFETY: plain FFI; a null handle (process already gone) is handled, and the handle is
    // closed once.
    unsafe {
        let h = OpenProcess(PROCESS_SYNCHRONIZE, 0, pid);
        if !h.is_null() {
            WaitForSingleObject(h, max.as_millis().min(u128::from(u32::MAX - 1)) as u32);
            CloseHandle(h);
        }
    }
}

fn desktop_link_exists() -> bool {
    ["USERPROFILE", "OneDrive"].iter().any(|v| {
        std::env::var_os(v)
            .map(|d| Path::new(&d).join("Desktop").join("screenIt.lnk").exists())
            .unwrap_or(false)
    })
}

/// The `msiexec` arguments for upgrading: keep the user's start-with-Windows and desktop choices.
pub fn msiexec_args(msi: &Path, startup: bool, desktop: bool, log: &Path) -> Vec<String> {
    vec![
        "/i".into(),
        msi.to_string_lossy().into_owned(),
        "/qn".into(),
        "/norestart".into(),
        format!("STARTUP={}", u8::from(startup)),
        format!("DESKTOP={}", u8::from(desktop)),
        "/l*v".into(),
        log.to_string_lossy().into_owned(),
    ]
}

/// Helper mode: wait for the app, run the installer, start the new version. Returns the exit
/// code for the helper process.
pub fn apply_update(msi: &Path, wait_pid: u32) -> i32 {
    wait_for_exit(wait_pid, Duration::from_secs(60));
    let startup = crate::autostart::is_enabled_as(crate::autostart::APP_VALUE);
    let log = msi.with_file_name("install.log");
    let status = Command::new("msiexec.exe")
        .args(msiexec_args(msi, startup, desktop_link_exists(), &log))
        .creation_flags(CREATE_NO_WINDOW)
        .status();
    let code = status.ok().and_then(|s| s.code()).unwrap_or(-1);
    if code == 0 || code == 3010 {
        let _ = std::fs::remove_file(msi);
    }
    if let Some(dir) = std::env::var_os("LOCALAPPDATA") {
        let exe = Path::new(&dir)
            .join("Programs")
            .join("screenIt")
            .join("screenit.exe");
        let _ = Command::new(exe).spawn();
    }
    code
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_are_split_and_checked() {
        assert_eq!(
            split_url("https://api.github.com/repos/a/b").unwrap(),
            ("api.github.com", "repos/a/b")
        );
        assert!(split_url("http://x/y").is_err());
        assert!(split_url("https:///y").is_err());
        assert!(split_url("https://user@x/y").is_err());
        assert!(split_url("https://x:99/y").is_err());
    }

    #[test]
    fn apply_arguments_are_read() {
        let a: Vec<String> = ["x", "--apply-update", "C:\\u\\a.msi", "--wait-pid", "42"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(apply_request(&a), Some((PathBuf::from("C:\\u\\a.msi"), 42)));
        assert_eq!(apply_request(&["x".to_string()]), None);
    }

    #[test]
    fn msiexec_keeps_the_users_choices() {
        let a = msiexec_args(Path::new("a.msi"), false, true, Path::new("l.log"));
        assert!(a.contains(&"STARTUP=0".to_string()) && a.contains(&"DESKTOP=1".to_string()));
        assert!(a.contains(&"/qn".to_string()));
    }

    #[test]
    #[ignore = "needs internet"]
    fn fetches_the_real_latest_release() {
        let t = WinUpdateHost
            .get_text(screenit_update::LATEST_URL, 1 << 20)
            .unwrap();
        assert!(screenit_update::parse_release(&t).is_ok());
    }
}
