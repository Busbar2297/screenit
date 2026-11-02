//! Enforces the dependency layering from AGENTS.md section 2.

use serde_json::Value;
use std::process::Command;

/// (crate name without prefix, layer). A crate may depend only on equal-or-lower layers.
pub const LAYERS: &[(&str, u8)] = &[
    ("geom", 0),
    ("raster", 0),
    ("codecs", 0),
    ("i18n", 0),
    ("doc", 1),
    ("ops", 2),
    ("algo", 2),
    ("vector", 2),
    ("compose", 3),
    ("format", 3),
    ("capture", 4),
    ("record", 4),
    ("library", 4),
    ("llm", 5),
    ("platform", 5),
    ("engine", 5),
    ("ui-egui", 6),
    ("automation", 6),
    ("testkit", 7),
];

/// Same-layer edges that are allowed (a depends on b).
const SIDEWAYS_OK: &[(&str, &str)] = &[
    ("vector", "doc"),
    ("raster", "geom"),
    ("codecs", "raster"),
    ("format", "compose"),
    ("platform", "engine"),
    ("platform", "llm"),
];

fn layer_of(name: &str) -> Option<u8> {
    let short = name.strip_prefix("screenit-")?;
    LAYERS.iter().find(|(n, _)| *n == short).map(|(_, l)| *l)
}

pub fn run() -> Result<(), String> {
    let out = Command::new("cargo")
        .args(["metadata", "--format-version", "1", "--no-deps"])
        .output()
        .map_err(|e| format!("cargo metadata: {e}"))?;
    let meta: Value = serde_json::from_slice(&out.stdout).map_err(|e| e.to_string())?;
    let pkgs = meta
        .get("packages")
        .and_then(Value::as_array)
        .ok_or("no packages")?;
    let mut errors = Vec::new();
    for p in pkgs {
        let name = p.get("name").and_then(Value::as_str).unwrap_or("");
        let Some(layer) = layer_of(name) else {
            continue;
        };
        let short = name.strip_prefix("screenit-").unwrap_or(name);
        let deps = p
            .get("dependencies")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        for d in deps {
            if d.get("kind").and_then(Value::as_str) == Some("dev") {
                continue;
            }
            let dn = d.get("name").and_then(Value::as_str).unwrap_or("");
            let Some(dl) = layer_of(dn) else { continue };
            let dshort = dn.strip_prefix("screenit-").unwrap_or(dn);
            let allowed = dl < layer
                || (dl == layer && SIDEWAYS_OK.contains(&(short, dshort)))
                || short == "testkit";
            if !allowed {
                errors.push(format!("{name} (L{layer}) must not depend on {dn} (L{dl})"));
            }
            if short == "engine" && dshort == "platform" {
                errors.push("engine must not depend on platform (inject via traits)".into());
            }
        }
    }
    if errors.is_empty() {
        println!("layers: ok");
        Ok(())
    } else {
        Err(errors.join("\n"))
    }
}
