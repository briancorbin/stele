use anyhow::{anyhow, Context, Result};
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::path::Path;

/// Load locales from `dir`. Two layouts are supported and may be mixed:
///   - `dir/<locale>.json` — the whole locale in one file.
///   - `dir/<locale>/**/*.json` — a locale split across files and folders, where
///     each folder name and file stem becomes a namespace segment. e.g.
///     `dir/en/walker/today.json` contributes to `copy.walker.today.*`.
///
/// Everything contributing to the same locale is deep-merged into one tree.
pub fn load_locales(dir: &Path) -> Result<BTreeMap<String, Value>> {
    let mut map: BTreeMap<String, Value> = BTreeMap::new();

    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .with_context(|| format!("reading locales dir {}", dir.display()))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    entries.sort_by_key(|e| e.path());

    for entry in entries {
        let path = entry.path();
        if path.is_dir() {
            // Folder-per-locale: the dir name is the locale; files/subfolders nest.
            let locale = name_of(&path);
            let mut tree = Value::Object(Map::new());
            load_dir_into(&path, &mut Vec::new(), &mut tree)?;
            merge_locale(&mut map, locale, tree)?;
        } else if path.extension().and_then(|e| e.to_str()) == Some("json") {
            // Single-file locale: the file stem is the locale.
            let locale = path
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default();
            let value = parse_json(&path)?;
            merge_locale(&mut map, locale, value)?;
        }
    }
    Ok(map)
}

fn name_of(path: &Path) -> String {
    path.file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default()
}

fn parse_json(path: &Path) -> Result<Value> {
    let text = std::fs::read_to_string(path)?;
    serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display()))
}

/// Recursively read every `*.json` under `dir`, nesting each file's content at a
/// path built from the folder names + file stem (relative to the locale root).
fn load_dir_into(dir: &Path, segments: &mut Vec<String>, tree: &mut Value) -> Result<()> {
    let mut entries: Vec<_> =
        std::fs::read_dir(dir)?.collect::<std::result::Result<Vec<_>, _>>()?;
    entries.sort_by_key(|e| e.path());

    for entry in entries {
        let path = entry.path();
        if path.is_dir() {
            segments.push(name_of(&path));
            load_dir_into(&path, segments, tree)?;
            segments.pop();
        } else if path.extension().and_then(|e| e.to_str()) == Some("json") {
            let stem = path
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default();
            let content = parse_json(&path)?;
            segments.push(stem);
            insert_at(tree, segments, content)
                .with_context(|| format!("merging {}", path.display()))?;
            segments.pop();
        }
    }
    Ok(())
}

/// Navigate/create object nodes along `segments`, then deep-merge `content` at
/// the final segment.
fn insert_at(tree: &mut Value, segments: &[String], content: Value) -> Result<()> {
    let mut cur = tree;
    for (i, seg) in segments.iter().enumerate() {
        let obj = cur
            .as_object_mut()
            .ok_or_else(|| anyhow!("'{seg}' is used as both a value and a namespace"))?;
        if i == segments.len() - 1 {
            if obj.contains_key(seg) {
                deep_merge(obj.get_mut(seg).unwrap(), content)?;
            } else {
                obj.insert(seg.clone(), content);
            }
            return Ok(());
        }
        cur = obj
            .entry(seg.clone())
            .or_insert_with(|| Value::Object(Map::new()));
    }
    Ok(())
}

fn merge_locale(map: &mut BTreeMap<String, Value>, locale: String, value: Value) -> Result<()> {
    match map.get_mut(&locale) {
        Some(existing) => deep_merge(existing, value),
        None => {
            map.insert(locale, value);
            Ok(())
        }
    }
}

/// Recursively merge `from` into `into`. Objects combine key-by-key; any leaf
/// collision (a key defined twice, or object-vs-value mismatch) is an error so
/// split files can never silently clobber each other.
fn deep_merge(into: &mut Value, from: Value) -> Result<()> {
    match (into, from) {
        (Value::Object(a), Value::Object(b)) => {
            for (k, v) in b {
                if a.contains_key(&k) {
                    deep_merge(a.get_mut(&k).unwrap(), v)?;
                } else {
                    a.insert(k, v);
                }
            }
            Ok(())
        }
        _ => Err(anyhow!(
            "conflicting copy keys across locale files (a key is defined twice, \
             or as both a namespace and a value)"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn insert_nests_by_path_segments() {
        let mut tree = Value::Object(Map::new());
        insert_at(
            &mut tree,
            &["walker".into(), "today".into()],
            json!({ "greeting": "Hi" }),
        )
        .unwrap();
        // file `en/walker/today.json` → copy.walker.today.greeting
        assert_eq!(tree["walker"]["today"]["greeting"], json!("Hi"));
    }

    #[test]
    fn deep_merge_combines_sibling_namespaces() {
        let mut a = json!({ "walker": { "today": { "greeting": "Hi" } } });
        deep_merge(
            &mut a,
            json!({ "walker": { "schedule": { "title": "Schedule" } } }),
        )
        .unwrap();
        assert_eq!(a["walker"]["today"]["greeting"], json!("Hi"));
        assert_eq!(a["walker"]["schedule"]["title"], json!("Schedule"));
    }

    #[test]
    fn deep_merge_rejects_collisions() {
        let mut a = json!({ "x": "one" });
        assert!(deep_merge(&mut a, json!({ "x": "two" })).is_err());
    }
}
