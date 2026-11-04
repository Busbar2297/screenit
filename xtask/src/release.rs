//! `cargo xtask release`: build the zip and MSI, tag, and publish a GitHub release with the
//! installer, the zip and `SHA256SUMS.txt` (the updater verifies downloads against that file).
//!
//! GitHub access uses the credential `git` already has (`git credential fill`) through
//! `curl.exe`, which ships with Windows. The token is written to a temp header file, never
//! printed and never put on a command line.

use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const REPO: &str = "Busbar2297/screenit";

fn git(args: &[&str]) -> Result<String, String> {
    let out = Command::new("git")
        .args(args)
        .output()
        .map_err(|e| format!("git: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        Err(format!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        ))
    }
}

/// The `## <version>` section of CHANGELOG.md, without its heading.
pub fn changelog_section(text: &str, version: &str) -> Option<String> {
    let mut lines = text.lines();
    let head = format!("## {version}");
    lines.find(|l| l.trim() == head)?;
    let body: Vec<&str> = lines.take_while(|l| !l.starts_with("## ")).collect();
    let body = body.join("\n").trim().to_string();
    (!body.is_empty()).then_some(body)
}

fn token() -> Result<String, String> {
    let mut child = Command::new("git")
        .args(["credential", "fill"])
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("git credential: {e}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        use std::io::Write;
        let _ = stdin.write_all(b"protocol=https\nhost=github.com\n\n");
    }
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .find_map(|l| l.strip_prefix("password=").map(str::to_string))
        .filter(|t| !t.is_empty())
        .ok_or_else(|| "git has no GitHub credential".to_string())
}

struct Api {
    header: PathBuf,
}

impl Api {
    fn new() -> Result<Api, String> {
        let header = Path::new("target").join("release-auth.txt");
        std::fs::write(&header, format!("Authorization: Bearer {}\n", token()?))
            .map_err(|e| e.to_string())?;
        Ok(Api { header })
    }

    /// Send a request and return (HTTP status, JSON body).
    fn call(
        &self,
        method: &str,
        url: &str,
        body: Option<&Path>,
        content_type: &str,
    ) -> Result<(u16, Value), String> {
        let out_path = Path::new("target").join("release-reply.json");
        let mut c = Command::new("curl.exe");
        c.args(["-sS", "-X", method, "-o"])
            .arg(&out_path)
            .args(["-w", "%{http_code}"])
            .arg("-H")
            .arg(format!("@{}", self.header.display()))
            .args(["-H", "Accept: application/vnd.github+json"])
            .args(["-H", "User-Agent: screenit-xtask"])
            .args(["-H", &format!("Content-Type: {content_type}")]);
        if let Some(b) = body {
            c.arg("--data-binary").arg(format!("@{}", b.display()));
        }
        let out = c.arg(url).output().map_err(|e| format!("curl: {e}"))?;
        if !out.status.success() {
            return Err(format!(
                "curl failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            ));
        }
        let code: u16 = String::from_utf8_lossy(&out.stdout)
            .trim()
            .parse()
            .map_err(|_| "no HTTP status from curl".to_string())?;
        let text = std::fs::read_to_string(&out_path).unwrap_or_default();
        let _ = std::fs::remove_file(&out_path);
        Ok((code, serde_json::from_str(&text).unwrap_or(Value::Null)))
    }
}

impl Drop for Api {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.header);
    }
}

pub fn run() -> Result<(), String> {
    let version = env!("CARGO_PKG_VERSION");
    let tag = format!("v{version}");
    let notes = std::fs::read_to_string("CHANGELOG.md")
        .ok()
        .and_then(|t| changelog_section(&t, version))
        .ok_or_else(|| format!("CHANGELOG.md has no `## {version}` section"))?;

    // Builds the zip, the MSI and release/SHA256SUMS.txt.
    super::msi()?;
    let files: Vec<PathBuf> = [
        format!("screenit-v{version}-windows-x64.msi"),
        format!("screenit-v{version}-windows-x64.zip"),
        "SHA256SUMS.txt".to_string(),
    ]
    .iter()
    .map(|n| Path::new("release").join(n))
    .collect();
    if let Some(missing) = files.iter().find(|f| !f.exists()) {
        return Err(format!("{} was not built", missing.display()));
    }

    if git(&["tag", "-l", &tag])?.is_empty() {
        git(&["tag", "-a", &tag, "-m", &format!("screenIt {version}")])?;
    }
    git(&["push", "origin", &tag])?;

    let api = Api::new()?;
    let base = format!("https://api.github.com/repos/{REPO}");
    let (code, existing) = api.call(
        "GET",
        &format!("{base}/releases/tags/{tag}"),
        None,
        "application/json",
    )?;
    let release = if code == 200 {
        existing
    } else {
        let body = Path::new("target").join("release-body.json");
        std::fs::write(
            &body,
            json!({"tag_name": tag, "name": tag, "body": notes, "target_commitish": "main"})
                .to_string(),
        )
        .map_err(|e| e.to_string())?;
        let (code, made) = api.call(
            "POST",
            &format!("{base}/releases"),
            Some(&body),
            "application/json",
        )?;
        let _ = std::fs::remove_file(&body);
        if code != 201 {
            return Err(format!("creating the release failed (HTTP {code}): {made}"));
        }
        made
    };
    let id = release
        .get("id")
        .and_then(Value::as_u64)
        .ok_or("release has no id")?;
    let have: Vec<String> = release
        .get("assets")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|x| x.get("name").and_then(Value::as_str).map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    for f in &files {
        let name = f.file_name().and_then(|n| n.to_str()).unwrap_or_default();
        if have.iter().any(|h| h == name) {
            println!("already uploaded: {name}");
            continue;
        }
        let url =
            format!("https://uploads.github.com/repos/{REPO}/releases/{id}/assets?name={name}");
        let (code, reply) = api.call("POST", &url, Some(f), "application/octet-stream")?;
        if code != 201 {
            return Err(format!("uploading {name} failed (HTTP {code}): {reply}"));
        }
        println!("uploaded {name}");
    }
    println!("https://github.com/{REPO}/releases/tag/{tag}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_one_changelog_section() {
        let t = "# Changelog\n\n## 0.2.12\n\n- New.\n- Newer.\n\n## 0.2.11\n\n- Old.\n";
        assert_eq!(
            changelog_section(t, "0.2.12").as_deref(),
            Some("- New.\n- Newer.")
        );
        assert_eq!(changelog_section(t, "0.2.11").as_deref(), Some("- Old."));
        assert_eq!(changelog_section(t, "9.9.9"), None);
        assert_eq!(changelog_section("## 1.0.0\n\n## 0.9.0\nx", "1.0.0"), None);
    }
}
