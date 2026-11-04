//! A scripted `UpdateHost` for tests (no network, no installer).

use crate::{UpdateHost, release::sha256_hex};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Default)]
pub struct FakeHost {
    routes: Mutex<HashMap<String, Vec<u8>>>,
    /// Bytes written instead of the real installer, to test checksum failures.
    corrupt: Mutex<Option<Vec<u8>>>,
    launched: Mutex<Vec<PathBuf>>,
    offline: AtomicBool,
}

fn guard<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl FakeHost {
    /// A latest release `version` with an installer holding `msi` and, when `msi` is given, a
    /// matching `SHA256SUMS.txt`.
    pub fn with_release(version: &str, msi: Option<&[u8]>) -> FakeHost {
        let base = format!("https://github.com/Busbar2297/screenit/releases/download/v{version}");
        let name = format!("screenit-v{version}-windows-x64.msi");
        let mut assets = vec![format!(
            r#"{{"name":"{name}","browser_download_url":"{base}/{name}","size":10}}"#
        )];
        let h = FakeHost::default();
        if let Some(bytes) = msi {
            assets.push(format!(
                r#"{{"name":"SHA256SUMS.txt","browser_download_url":"{base}/SHA256SUMS.txt"}}"#
            ));
            guard(&h.routes).insert(format!("{base}/{name}"), bytes.to_vec());
            let sums = format!("{}  {name}\n", sha256_hex(bytes));
            guard(&h.routes).insert(format!("{base}/SHA256SUMS.txt"), sums.into_bytes());
        }
        let json = format!(
            r#"{{"tag_name":"v{version}","html_url":"https://github.com/Busbar2297/screenit/releases/tag/v{version}","assets":[{}]}}"#,
            assets.join(",")
        );
        guard(&h.routes).insert(crate::LATEST_URL.to_string(), json.into_bytes());
        h
    }

    pub fn set_offline(&self, off: bool) {
        self.offline.store(off, Ordering::SeqCst);
    }

    pub fn corrupt_download(&self, bytes: &[u8]) {
        *guard(&self.corrupt) = Some(bytes.to_vec());
    }

    pub fn launched(&self) -> Vec<PathBuf> {
        guard(&self.launched).clone()
    }

    fn body(&self, url: &str) -> Result<Vec<u8>, String> {
        if self.offline.load(Ordering::SeqCst) {
            return Err("offline".into());
        }
        guard(&self.routes)
            .get(url)
            .cloned()
            .ok_or_else(|| format!("404 {url}"))
    }
}

impl UpdateHost for FakeHost {
    fn get_text(&self, url: &str, max_bytes: u64) -> Result<String, String> {
        let b = self.body(url)?;
        if b.len() as u64 > max_bytes {
            return Err("too large".into());
        }
        String::from_utf8(b).map_err(|e| e.to_string())
    }

    fn download(
        &self,
        url: &str,
        dest: &Path,
        max_bytes: u64,
        progress: &dyn Fn(u64, u64),
    ) -> Result<(), String> {
        let mut b = self.body(url)?;
        if let Some(c) = guard(&self.corrupt).clone() {
            b = c;
        }
        if b.len() as u64 > max_bytes {
            return Err("too large".into());
        }
        progress(b.len() as u64, b.len() as u64);
        std::fs::write(dest, b).map_err(|e| e.to_string())
    }

    fn launch_installer(&self, msi: &Path) -> Result<(), String> {
        guard(&self.launched).push(msi.to_path_buf());
        Ok(())
    }
}
