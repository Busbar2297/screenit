//! Build/test harness: `cargo xtask <layers|parity|scorecard|ci|bless|panic-hunt|perf>`.

mod layers;
mod parity;

use std::process::{Command, ExitCode};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cmd = args.first().map(String::as_str).unwrap_or("help");
    let rest = args.get(1..).unwrap_or(&[]);
    let result = match cmd {
        "layers" => layers::run(),
        "parity" => parity::run(false),
        "scorecard" => parity::run(true),
        "ci" => ci(rest),
        "bless" => run_cargo(&["test", "--workspace"], &[("SCREENIT_BLESS", "1")]),
        "panic-hunt" => run_cargo(
            &[
                "test",
                "-p",
                "screenit-engine",
                "--test",
                "panic_hunt",
                "--",
                "--include-ignored",
            ],
            &[],
        ),
        "package" => package().map(|_| ()),
        "msi" => msi(),
        "i18n" => i18n_check(),
        "perf" => run_cargo(
            &[
                "test",
                "-p",
                "screenit-engine",
                "--release",
                "--test",
                "perf",
                "--",
                "--include-ignored",
            ],
            &[],
        ),
        _ => {
            eprintln!(
                "usage: cargo xtask <layers|parity|scorecard|ci [--e2e]|bless|panic-hunt|perf|package|msi>"
            );
            Ok(())
        }
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("xtask {cmd} failed: {e}");
            ExitCode::FAILURE
        }
    }
}

pub fn run_cargo(args: &[&str], env: &[(&str, &str)]) -> Result<(), String> {
    let mut c = Command::new("cargo");
    c.args(args);
    for (k, v) in env {
        c.env(k, v);
    }
    let status = c.status().map_err(|e| format!("spawn cargo: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("cargo {} exited with {status}", args.join(" ")))
    }
}

fn ci(rest: &[String]) -> Result<(), String> {
    let e2e = rest.iter().any(|a| a == "--e2e");
    run_cargo(&["fmt", "--all", "--check"], &[])?;
    run_cargo(
        &[
            "clippy",
            "--workspace",
            "--all-targets",
            "--",
            "-D",
            "warnings",
        ],
        &[],
    )?;
    run_cargo(&["test", "--workspace"], &[])?;
    layers::run()?;
    parity::run(false)?;
    parity::run(true)?;
    run_cargo(
        &[
            "test",
            "-p",
            "screenit-engine",
            "--test",
            "panic_hunt",
            "--",
            "--include-ignored",
        ],
        &[],
    )?;
    if e2e {
        run_cargo(
            &[
                "test",
                "--workspace",
                "--features",
                "e2e",
                "--",
                "--include-ignored",
            ],
            &[],
        )?;
    }
    Ok(())
}

/// Every `t("key")` / `tf("key", ..)` literal in the UI and app sources must exist in `en.toml`.
fn i18n_check() -> Result<(), String> {
    let en = std::fs::read_to_string("crates/i18n/locales/en.toml").map_err(|e| e.to_string())?;
    let keys: std::collections::HashSet<String> = en
        .lines()
        .filter_map(|l| {
            l.strip_prefix('"')?
                .split_once('"')
                .map(|(k, _)| k.to_string())
        })
        .collect();
    let mut missing = Vec::new();
    let mut stack = vec![
        std::path::PathBuf::from("crates/ui-egui/src"),
        std::path::PathBuf::from("apps/screenit/src"),
    ];
    while let Some(dir) = stack.pop() {
        for e in std::fs::read_dir(&dir)
            .map_err(|e| e.to_string())?
            .flatten()
        {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().is_some_and(|x| x == "rs") {
                let src = std::fs::read_to_string(&p).map_err(|e| e.to_string())?;
                for pat in ["t(\"", "tf(\"", "display(\""] {
                    let mut rest = src.as_str();
                    while let Some(i) = rest.find(pat) {
                        let before = rest[..i].chars().next_back();
                        let after = &rest[i + pat.len()..];
                        let ident_before = before.is_some_and(|c| c.is_alphanumeric() || c == '_');
                        if let Some(end) = after.find('"') {
                            let k = &after[..end];
                            if !ident_before && pat != "display(\"" && !keys.contains(k) {
                                missing.push(format!("{}: `{k}`", p.display()));
                            }
                        }
                        rest = after;
                    }
                }
            }
        }
    }
    let mut locales = 0;
    let mut gaps = Vec::new();
    for e in std::fs::read_dir("crates/i18n/locales")
        .map_err(|e| e.to_string())?
        .flatten()
    {
        let text = std::fs::read_to_string(e.path()).map_err(|e| e.to_string())?;
        locales += 1;
        let have: std::collections::HashSet<String> = text
            .lines()
            .filter_map(|l| {
                l.strip_prefix('"')?
                    .split_once('"')
                    .map(|(k, _)| k.to_string())
            })
            .collect();
        let lacking = keys.iter().filter(|k| !have.contains(*k)).count();
        if lacking > 0 {
            gaps.push(format!("{}: {lacking} keys missing", e.path().display()));
        }
    }
    println!("{locales} locales, {} keys in en.toml", keys.len());
    for g in &gaps {
        println!("  incomplete {g}");
    }
    if missing.is_empty() && gaps.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "{} unknown key(s) used in code, {} incomplete locale(s): {}",
            missing.len(),
            gaps.len(),
            missing.join("; ")
        ))
    }
}

/// Portable release: binaries + docs staged under `release/screenit-v<version>-windows-x64/` and
/// zipped next to it, ready to attach to a GitHub release. (An MSI installer is a documented gap.)
fn package() -> Result<std::path::PathBuf, String> {
    // Binaries embed source paths (panic locations); strip the builder's home and checkout paths.
    // rustc applies the last matching prefix, so the broad home-directory rule comes first.
    let home = std::env::var("USERPROFILE").unwrap_or_default();
    let cargo_home = std::env::var("CARGO_HOME").unwrap_or_else(|_| format!("{home}\\.cargo"));
    let rustup_home = std::env::var("RUSTUP_HOME").unwrap_or_else(|_| format!("{home}\\.rustup"));
    let checkout = std::env::current_dir()
        .map(|p| p.display().to_string())
        .unwrap_or_default();
    let flags = [
        (home.as_str(), "home"),
        (cargo_home.as_str(), "cargo"),
        (rustup_home.as_str(), "rustup"),
        (checkout.as_str(), "screenit"),
    ]
    .iter()
    .filter(|(from, _)| !from.is_empty())
    .map(|(from, to)| format!("--remap-path-prefix={from}={to}"))
    .collect::<Vec<_>>()
    .join("\u{1f}");
    run_cargo(
        &["build", "--release", "-p", "screenit", "-p", "screenit-cli"],
        &[("CARGO_ENCODED_RUSTFLAGS", flags.as_str())],
    )?;
    let name = format!("screenit-v{}-windows-x64", env!("CARGO_PKG_VERSION"));
    let root = std::path::Path::new("release");
    let stage = root.join(&name);
    let _ = std::fs::remove_dir_all(&stage);
    std::fs::create_dir_all(&stage).map_err(|e| e.to_string())?;
    for f in ["screenit.exe", "screenit-cli.exe"] {
        std::fs::copy(
            std::path::Path::new("target/release").join(f),
            stage.join(f),
        )
        .map_err(|e| format!("copy {f}: {e}"))?;
    }
    for f in [
        "README.md",
        "LICENSE-MIT",
        "LICENSE-APACHE",
        "ATTRIBUTION.md",
        "CHANGELOG.md",
    ] {
        if std::path::Path::new(f).exists() {
            std::fs::copy(f, stage.join(f)).map_err(|e| e.to_string())?;
        }
    }
    // Editable translations ship next to the exe (users can also override in %LOCALAPPDATA%).
    let loc = stage.join("locales");
    std::fs::create_dir_all(&loc).map_err(|e| e.to_string())?;
    let entries = std::fs::read_dir("crates/i18n/locales").map_err(|e| e.to_string())?;
    for e in entries.flatten() {
        let p = e.path();
        if p.extension().is_some_and(|x| x == "toml") {
            if let Some(n) = p.file_name() {
                std::fs::copy(&p, loc.join(n)).map_err(|e| e.to_string())?;
            }
        }
    }
    let zip = root.join(format!("{name}.zip"));
    let _ = std::fs::remove_file(&zip);
    let status = Command::new("powershell")
        .args([
            "-NoProfile",
            "-Command",
            &format!(
                "Compress-Archive -Path '{}' -DestinationPath '{}' -Force",
                stage.display(),
                zip.display()
            ),
        ])
        .status()
        .map_err(|e| e.to_string())?;
    if !status.success() || !zip.exists() {
        return Err("Compress-Archive failed".into());
    }
    println!("packaged {}", zip.display());
    Ok(stage)
}

/// Component/feature XML for the files in `dir` (one level of sub-folders, e.g. `locales`).
fn wix_files(dir: &std::path::Path) -> Result<(String, String), String> {
    let id = |s: &str| {
        let clean: String = s
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
            .collect();
        format!("C_{clean}")
    };
    let mut files = String::new();
    let mut refs = String::new();
    let mut walk = |folder: &std::path::Path, rel: &str, out: &mut String| -> Result<(), String> {
        let mut names: Vec<_> = std::fs::read_dir(folder)
            .map_err(|e| e.to_string())?
            .filter_map(|e| e.ok())
            .filter(|e| e.path().is_file())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        for n in names {
            let key = format!("{rel}{n}");
            let cid = id(&key);
            let src = folder.join(&n);
            out.push_str(&format!(
                "<Component Id=\"{cid}\" Guid=\"*\"><File Id=\"F{cid}\" Source=\"{}\" KeyPath=\"yes\" /></Component>\n",
                src.display()
            ));
            refs.push_str(&format!("<ComponentRef Id=\"{cid}\" />\n"));
        }
        Ok(())
    };
    walk(dir, "", &mut files)?;
    let locales = dir.join("locales");
    if locales.is_dir() {
        let mut inner = String::new();
        walk(&locales, "locales/", &mut inner)?;
        files.push_str(&format!(
            "<Directory Id=\"LocalesDir\" Name=\"locales\">\n{inner}</Directory>\n"
        ));
    }
    Ok((files, refs))
}

/// Per-user MSI (WiX Toolset v3, downloaded on first use into `target/wix314`).
fn msi() -> Result<(), String> {
    let stage = package()?;
    let version = env!("CARGO_PKG_VERSION");
    let wix = std::path::PathBuf::from("target/wix314");
    if !wix.join("candle.exe").exists() {
        let ps = format!(
            "$ErrorActionPreference='Stop'; New-Item -ItemType Directory -Force '{0}' | Out-Null; \
             Invoke-WebRequest -UseBasicParsing https://github.com/wixtoolset/wix3/releases/download/wix3141rtm/wix314-binaries.zip -OutFile 'target/wix314.zip'; \
             Expand-Archive 'target/wix314.zip' '{0}' -Force",
            wix.display()
        );
        let ok = Command::new("powershell")
            .args(["-NoProfile", "-Command", &ps])
            .status()
            .map_err(|e| e.to_string())?
            .success();
        if !ok {
            return Err("could not download the WiX toolset".into());
        }
    }
    let template = std::fs::read_to_string("installer/screenit.wxs").map_err(|e| e.to_string())?;
    let (files, refs) = wix_files(&stage)?;
    let wxs = template
        .replace("<!--FILES-->", &files)
        .replace("<!--REFS-->", &refs);
    let obj_dir = std::path::Path::new("target/wix");
    std::fs::create_dir_all(obj_dir).map_err(|e| e.to_string())?;
    let wxs_path = obj_dir.join("screenit.wxs");
    std::fs::write(&wxs_path, wxs).map_err(|e| e.to_string())?;
    let obj = obj_dir.join("screenit.wixobj");
    let out = std::path::Path::new("release").join(format!("screenit-v{version}-windows-x64.msi"));
    let _ = std::fs::remove_file(&out);
    let ok = Command::new(wix.join("candle.exe"))
        .args([
            "-nologo",
            "-arch",
            "x64",
            &format!("-dVersion={version}"),
            "-out",
        ])
        .arg(&obj)
        .arg(&wxs_path)
        .status()
        .map_err(|e| format!("candle: {e}"))?
        .success();
    if !ok {
        return Err("candle failed".into());
    }
    let ok = Command::new(wix.join("light.exe"))
        .args(["-nologo", "-sval", "-out"])
        .arg(&out)
        .arg(&obj)
        .status()
        .map_err(|e| format!("light: {e}"))?
        .success();
    if !ok || !out.exists() {
        return Err("light failed".into());
    }
    println!("built {}", out.display());
    Ok(())
}
