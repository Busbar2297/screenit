//! The GitHub "latest release" JSON and the `SHA256SUMS.txt` file.

use crate::version::Version;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::Path;

/// Download links must live under this prefix.
pub const ASSET_PREFIX: &str = "https://github.com/Busbar2297/screenit/";
/// Name of the checksum file published next to the installer.
pub const SUMS_NAME: &str = "SHA256SUMS.txt";

const MAX_ASSETS: usize = 50;

#[derive(Clone, Debug, PartialEq)]
pub struct Asset {
    pub name: String,
    pub url: String,
    pub size: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Release {
    pub version: Version,
    pub page_url: String,
    pub assets: Vec<Asset>,
}

impl Release {
    pub fn asset(&self, name: &str) -> Option<&Asset> {
        self.assets.iter().find(|a| a.name == name)
    }

    /// The installer: the first `.msi` asset.
    pub fn msi(&self) -> Option<&Asset> {
        self.assets
            .iter()
            .find(|a| a.name.to_ascii_lowercase().ends_with(".msi"))
    }
}

fn text<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    v.get(key).and_then(Value::as_str)
}

fn plain_name(n: &str) -> bool {
    !n.is_empty()
        && n.len() <= 120
        && !n.contains(['/', '\\', ':'])
        && !n.contains("..")
        && !n.chars().any(char::is_control)
}

/// Parse the body of `releases/latest`. Drafts, pre-releases and links outside the project
/// repository are refused.
pub fn parse_release(body: &str) -> Result<Release, String> {
    let v: Value = serde_json::from_str(body).map_err(|e| format!("bad release JSON: {e}"))?;
    if v.get("draft").and_then(Value::as_bool).unwrap_or(false)
        || v.get("prerelease")
            .and_then(Value::as_bool)
            .unwrap_or(false)
    {
        return Err("the latest release is a draft or pre-release".into());
    }
    let tag = text(&v, "tag_name").ok_or("release has no tag")?;
    let version = Version::parse(tag).ok_or_else(|| format!("unusable release tag `{tag}`"))?;
    let page_url = text(&v, "html_url").unwrap_or_default().to_string();
    let mut assets = Vec::new();
    for a in v
        .get("assets")
        .and_then(Value::as_array)
        .map(|a| a.iter().take(MAX_ASSETS))
        .into_iter()
        .flatten()
    {
        let (Some(name), Some(url)) = (text(a, "name"), text(a, "browser_download_url")) else {
            continue;
        };
        if !plain_name(name) || !url.starts_with(ASSET_PREFIX) {
            continue;
        }
        assets.push(Asset {
            name: name.to_string(),
            url: url.to_string(),
            size: a.get("size").and_then(Value::as_u64).unwrap_or(0),
        });
    }
    Ok(Release {
        version,
        page_url,
        assets,
    })
}

/// Find `name` in a `sha256sum` style listing (`<hex>  <name>` or `<hex> *<name>`).
pub fn parse_sha256sums(text: &str, name: &str) -> Option<String> {
    for line in text.lines().take(1000) {
        let line = line.trim();
        let Some((hash, rest)) = line.split_once(char::is_whitespace) else {
            continue;
        };
        let file = rest.trim_start().trim_start_matches('*');
        if file == name && hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Some(hash.to_ascii_lowercase());
        }
    }
    None
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

pub fn sha256_file(path: &Path) -> std::io::Result<String> {
    let mut f = std::fs::File::open(path)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(hex(&h.finalize()))
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes.iter().fold(String::new(), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const JSON: &str = r#"{"tag_name":"v0.2.12","html_url":"https://github.com/Busbar2297/screenit/releases/tag/v0.2.12",
      "draft":false,"prerelease":false,"assets":[
      {"name":"screenit-v0.2.12-windows-x64.msi","browser_download_url":"https://github.com/Busbar2297/screenit/releases/download/v0.2.12/screenit-v0.2.12-windows-x64.msi","size":10},
      {"name":"evil.msi","browser_download_url":"https://evil.example/evil.msi","size":1},
      {"name":"..\\x.msi","browser_download_url":"https://github.com/Busbar2297/screenit/x","size":1},
      {"name":"SHA256SUMS.txt","browser_download_url":"https://github.com/Busbar2297/screenit/releases/download/v0.2.12/SHA256SUMS.txt"}]}"#;

    #[test]
    fn parses_a_release_and_drops_foreign_links() {
        let r = parse_release(JSON).unwrap();
        assert_eq!(r.version.to_string(), "0.2.12");
        assert_eq!(r.assets.len(), 2);
        assert!(r.msi().unwrap().name.ends_with(".msi"));
        assert!(r.asset(SUMS_NAME).is_some());
    }

    #[test]
    fn refuses_bad_json_drafts_and_missing_tags() {
        assert!(parse_release("not json").is_err());
        assert!(parse_release("{}").is_err());
        assert!(parse_release(r#"{"tag_name":"v1.0.0","draft":true}"#).is_err());
        assert!(parse_release(r#"{"tag_name":"v1.0.0","prerelease":true}"#).is_err());
        assert!(parse_release(r#"{"tag_name":"latest"}"#).is_err());
    }

    #[test]
    fn sums_lookup() {
        let h = "a".repeat(64);
        let t = format!("{h}  one.zip\n{} *two.msi\n", "B".repeat(64));
        assert_eq!(parse_sha256sums(&t, "one.zip"), Some(h));
        assert_eq!(parse_sha256sums(&t, "two.msi"), Some("b".repeat(64)));
        assert_eq!(parse_sha256sums(&t, "three"), None);
        assert_eq!(parse_sha256sums("zz  one.zip", "one.zip"), None);
        assert_eq!(parse_sha256sums("", "x"), None);
    }

    #[test]
    fn sha256_known_vector() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}
