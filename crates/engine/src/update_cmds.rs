//! Update commands: look for a newer GitHub release, download it, install it.
//!
//! The network work runs on a worker thread so a command returns at once; the shared
//! `Updater` holds the state that `app.update.status` reports.

use crate::args::Args;
use crate::{CmdResult, CommandError, CommandSpec, Session, always};
use screenit_update::{UpdateState, Updater};
use serde_json::{Value, json};
use std::sync::Arc;

/// Seconds between automatic checks.
pub const CHECK_EVERY_SECS: u64 = 24 * 60 * 60;

fn updater(s: &Session) -> Result<Arc<Updater>, CommandError> {
    s.services
        .updater
        .clone()
        .ok_or_else(|| CommandError::Disabled("updates are not available in this build".into()))
}

fn have_updater(s: &Session) -> Option<&'static str> {
    s.services
        .updater
        .is_none()
        .then_some("updates are not available in this build")
}

fn can_download(s: &Session) -> Option<&'static str> {
    match s.services.updater.as_ref().map(|u| u.state()) {
        Some(UpdateState::Available {
            notify_only: false, ..
        }) => None,
        Some(_) => Some("no verified update is waiting to be downloaded"),
        None => have_updater(s),
    }
}

fn can_install(s: &Session) -> Option<&'static str> {
    match s.services.updater.as_ref().map(|u| u.state()) {
        Some(UpdateState::Ready { .. }) => None,
        Some(_) => Some("no downloaded update is ready to install"),
        None => have_updater(s),
    }
}

fn status_value(s: &Session) -> Value {
    let u = &s.settings.updates;
    let now = u64::try_from(s.services.clock.unix_secs()).unwrap_or(0);
    let (current, state) = match &s.services.updater {
        Some(up) => (up.current(), serde_json::to_value(up.state()).ok()),
        None => (env!("CARGO_PKG_VERSION").to_string(), None),
    };
    json!({
        "current": current,
        "available": s.services.updater.is_some(),
        "update": state.unwrap_or(json!({"state": "idle"})),
        "auto_check": u.auto_check,
        "auto_download": u.auto_download,
        "skipped_version": u.skipped_version,
        "last_check_unix": u.last_check_unix,
        "due": s.services.updater.is_some()
            && u.auto_check
            && now.saturating_sub(u.last_check_unix) >= CHECK_EVERY_SECS,
    })
}

fn status(s: &mut Session, p: &Value) -> CmdResult {
    Args::new(p, &[])?;
    Ok(status_value(s))
}

fn check(s: &mut Session, p: &Value) -> CmdResult {
    Args::new(p, &[])?;
    let up = updater(s)?;
    let skipped = s.settings.updates.skipped_version.clone();
    let auto_download = s.settings.updates.auto_download;
    s.settings.updates.last_check_unix = u64::try_from(s.services.clock.unix_secs()).unwrap_or(0);
    s.persist_settings()?;
    up.reset();
    let worker = up.clone();
    std::thread::spawn(move || {
        let state = worker.check(Some(&skipped).filter(|v| !v.is_empty()).map(String::as_str));
        if auto_download
            && matches!(
                state,
                UpdateState::Available {
                    notify_only: false,
                    ..
                }
            )
        {
            worker.download();
        }
    });
    Ok(status_value(s))
}

fn download(s: &mut Session, p: &Value) -> CmdResult {
    Args::new(p, &[])?;
    let up = updater(s)?;
    std::thread::spawn(move || {
        up.download();
    });
    Ok(status_value(s))
}

fn install(s: &mut Session, p: &Value) -> CmdResult {
    Args::new(p, &[])?;
    updater(s)?.install().map_err(CommandError::Failed)?;
    Ok(json!({"installing": true, "quit": true}))
}

fn skip(s: &mut Session, p: &Value) -> CmdResult {
    let a = Args::new(p, &["version"])?;
    let v = a.str("version")?.trim();
    if screenit_update::Version::parse(v).is_none() {
        return Err(CommandError::Params("version must look like 0.2.12".into()));
    }
    s.settings.updates.skipped_version = v.trim_start_matches(['v', 'V']).to_string();
    s.persist_settings()?;
    if let Some(up) = &s.services.updater {
        up.reset();
    }
    Ok(status_value(s))
}

pub fn specs() -> Vec<CommandSpec> {
    let c = |id, label, params, enabled, run| CommandSpec {
        id,
        label,
        menu: "Help",
        shortcut: "",
        params,
        enabled,
        run,
    };
    vec![
        c("app.update.status", "Update Status", "{}", always, status),
        c(
            "app.update.check",
            "Check for Updates",
            "{}",
            have_updater,
            check,
        ),
        c(
            "app.update.download",
            "Download Update",
            "{}",
            can_download,
            download,
        ),
        c(
            "app.update.install",
            "Restart to Update",
            "{}",
            can_install,
            install,
        ),
        c(
            "app.update.skip",
            "Skip This Version",
            r#"{"version":str}"#,
            have_updater,
            skip,
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Services;
    use screenit_update::fake::FakeHost;
    use std::time::{Duration, Instant};

    fn session(host: Arc<FakeHost>, tag: &str) -> Session {
        let dir = std::env::temp_dir().join(format!("scit-upd-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut s = Session::new(Services {
            updater: Updater::new(host, "0.2.11", dir.join("updates")).map(Arc::new),
            ..Services::default()
        });
        s.use_settings_file(dir.join("settings.json"));
        s
    }

    fn wait_until(s: &Session, done: impl Fn(&UpdateState) -> bool) -> UpdateState {
        let up = s.services.updater.clone().unwrap();
        let end = Instant::now() + Duration::from_secs(10);
        while Instant::now() < end {
            let st = up.state();
            if done(&st) {
                return st;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        up.state()
    }

    #[test]
    fn check_downloads_and_install_hands_over() {
        let host = Arc::new(FakeHost::with_release("0.2.12", Some(b"msi")));
        let mut s = session(host.clone(), "flow");
        assert!(s.dispatch("app.update.install", &json!({})).is_err());
        assert!(s.dispatch("app.update.download", &json!({})).is_err());
        s.dispatch("app.update.check", &json!({})).unwrap();
        let st = wait_until(&s, |st| matches!(st, UpdateState::Ready { .. }));
        assert!(matches!(st, UpdateState::Ready { .. }), "{st:?}");
        let r = s.dispatch("app.update.install", &json!({})).unwrap();
        assert_eq!(r["quit"], true);
        assert_eq!(host.launched().len(), 1);
        let status = s.dispatch("app.update.status", &json!({})).unwrap();
        assert_eq!(status["update"]["state"], "ready");
        assert_eq!(status["due"], false, "a check just ran");
    }

    #[test]
    fn auto_download_off_waits_for_the_download_command() {
        let host = Arc::new(FakeHost::with_release("0.2.12", Some(b"msi")));
        let mut s = session(host, "manual");
        s.set_setting("updates.auto_download", json!(false))
            .unwrap();
        s.dispatch("app.update.check", &json!({})).unwrap();
        let st = wait_until(&s, |st| matches!(st, UpdateState::Available { .. }));
        assert!(matches!(st, UpdateState::Available { .. }));
        s.dispatch("app.update.download", &json!({})).unwrap();
        let st = wait_until(&s, |st| matches!(st, UpdateState::Ready { .. }));
        assert!(matches!(st, UpdateState::Ready { .. }), "{st:?}");
    }

    #[test]
    fn skip_hides_that_version_and_rejects_junk() {
        let host = Arc::new(FakeHost::with_release("0.2.12", Some(b"msi")));
        let mut s = session(host, "skip");
        assert!(
            s.dispatch("app.update.skip", &json!({"version":"x"}))
                .is_err()
        );
        assert!(s.dispatch("app.update.skip", &json!({})).is_err());
        assert!(
            s.dispatch("app.update.skip", &json!({"version":"v0.2.12","x":1}))
                .is_err()
        );
        s.dispatch("app.update.skip", &json!({"version":"v0.2.12"}))
            .unwrap();
        assert_eq!(s.settings.updates.skipped_version, "0.2.12");
        s.dispatch("app.update.check", &json!({})).unwrap();
        let st = wait_until(&s, |st| *st == UpdateState::UpToDate);
        assert_eq!(st, UpdateState::UpToDate);
    }

    #[test]
    fn offline_check_reports_failed_and_no_updater_disables_commands() {
        let host = Arc::new(FakeHost::with_release("0.2.12", Some(b"msi")));
        host.set_offline(true);
        let mut s = session(host, "off");
        s.dispatch("app.update.check", &json!({})).unwrap();
        let st = wait_until(&s, |st| matches!(st, UpdateState::Failed { .. }));
        assert!(matches!(st, UpdateState::Failed { .. }));

        let mut bare = Session::new(Services::default());
        assert!(matches!(
            bare.dispatch("app.update.check", &json!({})),
            Err(CommandError::Disabled(_))
        ));
        let st = bare.dispatch("app.update.status", &json!({})).unwrap();
        assert_eq!(st["available"], false);
        assert_eq!(st["due"], false);
    }

    #[test]
    fn due_after_a_day_when_auto_check_is_on() {
        let host = Arc::new(FakeHost::with_release("0.2.11", None));
        let mut s = session(host, "due");
        assert_eq!(
            s.dispatch("app.update.status", &json!({})).unwrap()["due"],
            true
        );
        s.set_setting("updates.auto_check", json!(false)).unwrap();
        assert_eq!(
            s.dispatch("app.update.status", &json!({})).unwrap()["due"],
            false
        );
    }
}
