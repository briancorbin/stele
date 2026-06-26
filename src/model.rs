use anyhow::{Context, Result};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::Path;

/// Load every `*.json` file in `dir` into a map keyed by file stem (the locale).
pub fn load_locales(dir: &Path) -> Result<BTreeMap<String, Value>> {
    let mut map = BTreeMap::new();
    let entries =
        std::fs::read_dir(dir).with_context(|| format!("reading locales dir {}", dir.display()))?;
    for entry in entries {
        let path = entry?.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let name = path
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();
        let text = std::fs::read_to_string(&path)?;
        let value: Value =
            serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
        map.insert(name, value);
    }
    Ok(map)
}
