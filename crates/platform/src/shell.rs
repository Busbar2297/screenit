//! Shell integration: the user's real Pictures folder (follows OneDrive / folder redirection),
//! the native Save As dialog, and "show in folder".

use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use windows::Win32::Foundation::HWND;
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemFree,
};
use windows::Win32::UI::Shell::Common::COMDLG_FILTERSPEC;
use windows::Win32::UI::Shell::{
    FOLDERID_Pictures, FileSaveDialog, IFileSaveDialog, IShellItem, KF_FLAG_DEFAULT,
    SHCreateItemFromParsingName, SHGetKnownFolderPath, SIGDN_FILESYSPATH,
};
use windows::core::{HSTRING, PCWSTR};

/// The Pictures known folder (e.g. `C:\Users\me\OneDrive\Pictures` when redirected).
pub fn pictures_dir() -> Option<PathBuf> {
    // SAFETY: SHGetKnownFolderPath returns a CoTaskMem string that is copied, then freed.
    unsafe {
        let p = SHGetKnownFolderPath(&FOLDERID_Pictures, KF_FLAG_DEFAULT, None).ok()?;
        let s = p.to_string().ok();
        CoTaskMemFree(Some(p.0 as *const _));
        s.filter(|s| !s.is_empty()).map(PathBuf::from)
    }
}

/// Where screenIt saves captures and recordings: `<Pictures>\screenIt`.
pub fn output_dir() -> PathBuf {
    pictures_dir()
        .or_else(|| std::env::var_os("USERPROFILE").map(|p| PathBuf::from(p).join("Pictures")))
        .unwrap_or_else(|| PathBuf::from("."))
        .join("screenIt")
}

/// Native Save As dialog. `filter` is (description, pattern), e.g. ("MP4 video", "*.mp4").
/// Returns `None` when the user cancels or the dialog cannot be shown.
pub fn save_dialog(
    title: &str,
    dir: &Path,
    file_name: &str,
    filter: (&str, &str),
) -> Option<PathBuf> {
    let (title, name) = (HSTRING::from(title), HSTRING::from(file_name));
    let (fname, fspec) = (HSTRING::from(filter.0), HSTRING::from(filter.1));
    let ext = filter.1.trim_start_matches("*.");
    let ext = HSTRING::from(ext);
    let dir = HSTRING::from(dir.as_os_str());
    let owner = crate::window::main_hwnd_raw().map(|h| HWND(h as *mut _));
    // SAFETY: standard IFileSaveDialog usage; every string outlives the calls that borrow it and
    // the returned path string is copied before being freed.
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let dlg: IFileSaveDialog =
            CoCreateInstance(&FileSaveDialog, None, CLSCTX_INPROC_SERVER).ok()?;
        let _ = dlg.SetTitle(&title);
        let specs = [COMDLG_FILTERSPEC {
            pszName: PCWSTR(fname.as_ptr()),
            pszSpec: PCWSTR(fspec.as_ptr()),
        }];
        let _ = dlg.SetFileTypes(&specs);
        let _ = dlg.SetDefaultExtension(&ext);
        let _ = dlg.SetFileName(&name);
        if let Ok(folder) = SHCreateItemFromParsingName::<_, _, IShellItem>(&dir, None) {
            let _ = dlg.SetFolder(&folder);
        }
        dlg.Show(owner).ok()?; // cancelled -> error
        let item = dlg.GetResult().ok()?;
        let p = item.GetDisplayName(SIGDN_FILESYSPATH).ok()?;
        let s = p.to_string().ok();
        CoTaskMemFree(Some(p.0 as *const _));
        s.map(PathBuf::from)
    }
}

/// True for a plain `https://` address with no quotes or control characters.
pub fn https_url(url: &str) -> bool {
    let Some(rest) = url.strip_prefix("https://") else {
        return false;
    };
    !rest.is_empty()
        && rest.chars().all(|c| {
            c.is_ascii_alphanumeric()
                || matches!(
                    c,
                    '.' | '/' | '-' | '_' | '?' | '=' | '&' | '%' | '#' | ':' | '~' | '+'
                )
        })
}

/// Open `url` in the default browser. Anything that is not a plain https address is ignored.
pub fn open_url(url: &str) -> bool {
    if !https_url(url) {
        return false;
    }
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    std::process::Command::new("cmd.exe")
        .args(["/C", "start", "", url])
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .is_ok()
}

/// Open Explorer with `path` selected (or the folder itself if it is a directory).
pub fn reveal(path: &Path) {
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let mut cmd = std::process::Command::new("explorer.exe");
    if path.is_dir() {
        cmd.arg(path);
    } else {
        // Explorer wants `/select,"C:\path with spaces\file"` as a single raw argument.
        cmd.raw_arg(format!("/select,\"{}\"", path.display()));
    }
    let _ = cmd.creation_flags(CREATE_NO_WINDOW).spawn();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn https_url_accepts_the_repository_and_rejects_other_schemes() {
        assert!(https_url("https://github.com/Busbar2297/screenit"));
        assert!(!https_url("http://github.com/Busbar2297/screenit"));
        assert!(!https_url("https://evil.example/\" & calc"));
        assert!(!https_url("https://"));
        assert!(!open_url("notepad.exe"));
    }

    #[test]
    fn output_dir_is_inside_the_pictures_known_folder() {
        let out = output_dir();
        assert!(out.ends_with("screenIt"));
        if let Some(p) = pictures_dir() {
            assert_eq!(out.parent(), Some(p.as_path()));
        }
    }
}
