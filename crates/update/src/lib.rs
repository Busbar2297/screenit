//! Self-update from GitHub Releases.
//!
//! `Updater` asks GitHub for the latest release, downloads the installer, checks it against the
//! release's `SHA256SUMS.txt`, and hands it to the host to install. All network and process work
//! goes through the `UpdateHost` trait (WinHTTP in `screenit-platform`, a fake in tests), so this
//! crate is pure. `check` and `download` block; callers run them on a worker thread.
#![cfg_attr(
    not(test),
    deny(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::unimplemented,
        clippy::todo,
        clippy::unreachable
    )
)]

pub mod fake;
mod release;
mod version;

pub use release::{
    ASSET_PREFIX, Asset, Release, SUMS_NAME, parse_release, parse_sha256sums, sha256_file,
    sha256_hex,
};
pub use version::Version;

use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

/// The "latest release" endpoint of the project repository.
pub const LATEST_URL: &str = "https://api.github.com/repos/Busbar2297/screenit/releases/latest";
/// Largest installer the updater will download.
pub const MAX_MSI_BYTES: u64 = 200 * 1024 * 1024;
const MAX_JSON_BYTES: u64 = 1024 * 1024;
const MAX_SUMS_BYTES: u64 = 64 * 1024;

/// Network and process access, supplied by the app.
pub trait UpdateHost: Send + Sync {
    /// GET a small text body over HTTPS; fail when it is longer than `max_bytes`.
    fn get_text(&self, url: &str, max_bytes: u64) -> Result<String, String>;
    /// GET `url` into `dest` (written whole or not at all). `progress(done, total)`; total is 0
    /// when unknown.
    fn download(
        &self,
        url: &str,
        dest: &Path,
        max_bytes: u64,
        progress: &dyn Fn(u64, u64),
    ) -> Result<(), String>;
    /// Start the installer so that it replaces this app after it exits.
    fn launch_installer(&self, msi: &Path) -> Result<(), String>;
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum UpdateState {
    Idle,
    Checking,
    UpToDate,
    /// A newer release exists. `notify_only` when it cannot be verified (no checksum file).
    Available {
        version: String,
        page_url: String,
        notify_only: bool,
    },
    Downloading {
        version: String,
        percent: u8,
    },
    Ready {
        version: String,
        path: String,
    },
    Failed {
        message: String,
    },
}

struct Pending {
    version: Version,
    name: String,
    url: String,
    sha256: String,
}

struct Inner {
    state: UpdateState,
    pending: Option<Pending>,
}

pub struct Updater {
    host: Arc<dyn UpdateHost>,
    current: Version,
    dir: PathBuf,
    inner: Mutex<Inner>,
}

fn lock(m: &Mutex<Inner>) -> MutexGuard<'_, Inner> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Updater {
    /// `current` is this build's version text; `dir` receives downloaded installers.
    pub fn new(host: Arc<dyn UpdateHost>, current: &str, dir: PathBuf) -> Option<Updater> {
        Some(Updater {
            host,
            current: Version::parse(current)?,
            dir,
            inner: Mutex::new(Inner {
                state: UpdateState::Idle,
                pending: None,
            }),
        })
    }

    pub fn current(&self) -> String {
        self.current.to_string()
    }

    pub fn state(&self) -> UpdateState {
        lock(&self.inner).state.clone()
    }

    fn set(&self, state: UpdateState) -> UpdateState {
        lock(&self.inner).state = state.clone();
        state
    }

    fn fail(&self, message: impl std::fmt::Display) -> UpdateState {
        self.set(UpdateState::Failed {
            message: message.to_string(),
        })
    }

    /// Look for a newer release. `skipped` is a version the user chose to ignore.
    pub fn check(&self, skipped: Option<&str>) -> UpdateState {
        {
            let mut g = lock(&self.inner);
            if matches!(
                g.state,
                UpdateState::Checking | UpdateState::Downloading { .. } | UpdateState::Ready { .. }
            ) {
                return g.state.clone();
            }
            g.state = UpdateState::Checking;
            g.pending = None;
        }
        let release = match self
            .host
            .get_text(LATEST_URL, MAX_JSON_BYTES)
            .and_then(|b| parse_release(&b))
        {
            Ok(r) => r,
            Err(e) => return self.fail(e),
        };
        if release.version <= self.current
            || skipped.and_then(Version::parse) == Some(release.version)
        {
            return self.set(UpdateState::UpToDate);
        }
        let sha = self.checksum_of(&release);
        let notify_only = sha.is_none();
        if let (Some(msi), Some(sha256)) = (release.msi(), sha) {
            lock(&self.inner).pending = Some(Pending {
                version: release.version,
                name: msi.name.clone(),
                url: msi.url.clone(),
                sha256,
            });
        }
        let notify_only = notify_only || lock(&self.inner).pending.is_none();
        self.set(UpdateState::Available {
            version: release.version.to_string(),
            page_url: release.page_url,
            notify_only,
        })
    }

    fn checksum_of(&self, release: &Release) -> Option<String> {
        let msi = release.msi()?;
        let sums = release.asset(SUMS_NAME)?;
        let body = self.host.get_text(&sums.url, MAX_SUMS_BYTES).ok()?;
        parse_sha256sums(&body, &msi.name)
    }

    /// Download and verify the installer found by `check`.
    pub fn download(&self) -> UpdateState {
        let (version, name, url, sha256) = {
            let g = lock(&self.inner);
            match (&g.state, &g.pending) {
                (UpdateState::Available { .. }, Some(p)) => {
                    (p.version, p.name.clone(), p.url.clone(), p.sha256.clone())
                }
                _ => return g.state.clone(),
            }
        };
        let ver = version.to_string();
        self.set(UpdateState::Downloading {
            version: ver.clone(),
            percent: 0,
        });
        if let Err(e) = std::fs::create_dir_all(&self.dir) {
            return self.fail(format!("cannot create the update folder: {e}"));
        }
        let dest = self.dir.join(&name);
        let progress = |done: u64, total: u64| {
            let percent = if total == 0 {
                0
            } else {
                (done.saturating_mul(100) / total).min(100) as u8
            };
            lock(&self.inner).state = UpdateState::Downloading {
                version: ver.clone(),
                percent,
            };
        };
        if let Err(e) = self.host.download(&url, &dest, MAX_MSI_BYTES, &progress) {
            let _ = std::fs::remove_file(&dest);
            return self.fail(e);
        }
        match sha256_file(&dest) {
            Ok(got) if got == sha256 => self.set(UpdateState::Ready {
                version: ver,
                path: dest.to_string_lossy().into_owned(),
            }),
            Ok(_) => {
                let _ = std::fs::remove_file(&dest);
                self.fail("the download does not match its checksum and was deleted")
            }
            Err(e) => self.fail(format!("cannot read the download: {e}")),
        }
    }

    /// Start installing the downloaded update. The app must exit afterwards.
    pub fn install(&self) -> Result<(), String> {
        let path = match lock(&self.inner).state.clone() {
            UpdateState::Ready { path, .. } => PathBuf::from(path),
            _ => return Err("no update is ready to install".into()),
        };
        self.host.launch_installer(&path)
    }

    /// Forget a failure or a finished check so the next `check` starts afresh.
    pub fn reset(&self) {
        let mut g = lock(&self.inner);
        if matches!(
            g.state,
            UpdateState::Failed { .. } | UpdateState::UpToDate | UpdateState::Available { .. }
        ) {
            g.state = UpdateState::Idle;
            g.pending = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::FakeHost;

    fn dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("screenit-update-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    fn updater(host: &Arc<FakeHost>, tag: &str) -> Updater {
        Updater::new(host.clone(), "0.2.11", dir(tag)).unwrap()
    }

    #[test]
    fn newer_release_downloads_verifies_and_installs() {
        let h = Arc::new(FakeHost::with_release("0.2.12", Some(b"installer-bytes")));
        let u = updater(&h, "ok");
        assert!(matches!(
            u.check(None),
            UpdateState::Available {
                notify_only: false,
                ..
            }
        ));
        let UpdateState::Ready { version, path } = u.download() else {
            panic!("not ready: {:?}", u.state());
        };
        assert_eq!(version, "0.2.12");
        assert_eq!(std::fs::read(&path).unwrap(), b"installer-bytes");
        u.install().unwrap();
        assert_eq!(h.launched().len(), 1);
    }

    #[test]
    fn same_older_and_skipped_versions_are_up_to_date() {
        for (v, skip) in [
            ("0.2.11", None),
            ("0.2.3", None),
            ("0.2.12", Some("0.2.12")),
        ] {
            let h = Arc::new(FakeHost::with_release(v, Some(b"x")));
            assert_eq!(
                updater(&h, "same").check(skip),
                UpdateState::UpToDate,
                "{v}"
            );
        }
    }

    #[test]
    fn checksum_mismatch_deletes_the_file() {
        let h = Arc::new(FakeHost::with_release("0.2.12", Some(b"good")));
        h.corrupt_download(b"tampered");
        let u = updater(&h, "bad");
        u.check(None);
        assert!(matches!(u.download(), UpdateState::Failed { .. }));
        assert!(!u.dir.join("screenit-v0.2.12-windows-x64.msi").exists());
        assert!(u.install().is_err());
    }

    #[test]
    fn offline_fails_quietly_and_can_be_retried() {
        let h = Arc::new(FakeHost::with_release("0.2.12", Some(b"x")));
        h.set_offline(true);
        let u = updater(&h, "off");
        assert!(matches!(u.check(None), UpdateState::Failed { .. }));
        h.set_offline(false);
        assert!(matches!(u.check(None), UpdateState::Available { .. }));
    }

    #[test]
    fn release_without_checksums_is_notify_only() {
        let h = Arc::new(FakeHost::with_release("0.2.12", None));
        let u = updater(&h, "nosum");
        assert!(matches!(
            u.check(None),
            UpdateState::Available {
                notify_only: true,
                ..
            }
        ));
        assert!(matches!(u.download(), UpdateState::Available { .. }));
    }

    #[test]
    fn download_before_check_does_nothing() {
        let h = Arc::new(FakeHost::with_release("0.2.12", Some(b"x")));
        assert_eq!(updater(&h, "idle").download(), UpdateState::Idle);
    }
}
