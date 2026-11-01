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
        "package" => package(),
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
                "usage: cargo xtask <layers|parity|scorecard|ci [--e2e]|bless|panic-hunt|perf|package>"
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

/// Portable release: binaries + docs staged under `release/screenit-v<version>-windows-x64/` and
/// zipped next to it, ready to attach to a GitHub release. (An MSI installer is a documented gap.)
fn package() -> Result<(), String> {
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
    Ok(())
}
