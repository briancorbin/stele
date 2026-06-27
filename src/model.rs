use anyhow::{anyhow, Context, Result};
use serde::de::{self, Deserialize, Deserializer, MapAccess, SeqAccess, Visitor};
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::fmt;
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
    let mut de = serde_json::Deserializer::from_str(&text);
    let NoDupValue(value) =
        NoDupValue::deserialize(&mut de).with_context(|| format!("parsing {}", path.display()))?;
    de.end()
        .with_context(|| format!("parsing {}", path.display()))?;
    Ok(value)
}

/// A `serde_json::Value` that rejects duplicate object keys instead of silently
/// keeping the last one — so a translator pasting a key twice in one file is a
/// loud error, matching the cross-file collision guarantee. serde_json still does
/// all the parsing; this only intercepts how objects are assembled.
struct NoDupValue(Value);

impl<'de> Deserialize<'de> for NoDupValue {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        d.deserialize_any(NoDupVisitor)
    }
}

struct NoDupVisitor;

impl<'de> Visitor<'de> for NoDupVisitor {
    type Value = NoDupValue;

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("any valid JSON value")
    }

    fn visit_bool<E: de::Error>(self, v: bool) -> Result<Self::Value, E> {
        Ok(NoDupValue(Value::Bool(v)))
    }
    fn visit_i64<E: de::Error>(self, v: i64) -> Result<Self::Value, E> {
        Ok(NoDupValue(Value::Number(v.into())))
    }
    fn visit_u64<E: de::Error>(self, v: u64) -> Result<Self::Value, E> {
        Ok(NoDupValue(Value::Number(v.into())))
    }
    fn visit_f64<E: de::Error>(self, v: f64) -> Result<Self::Value, E> {
        Ok(NoDupValue(
            serde_json::Number::from_f64(v).map_or(Value::Null, Value::Number),
        ))
    }
    fn visit_str<E: de::Error>(self, v: &str) -> Result<Self::Value, E> {
        Ok(NoDupValue(Value::String(v.to_owned())))
    }
    fn visit_none<E: de::Error>(self) -> Result<Self::Value, E> {
        Ok(NoDupValue(Value::Null))
    }
    fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
        Ok(NoDupValue(Value::Null))
    }
    fn visit_some<D: Deserializer<'de>>(self, d: D) -> Result<Self::Value, D::Error> {
        NoDupValue::deserialize(d)
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
        let mut items = Vec::new();
        while let Some(NoDupValue(v)) = seq.next_element()? {
            items.push(v);
        }
        Ok(NoDupValue(Value::Array(items)))
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
        let mut obj = Map::new();
        while let Some(key) = map.next_key::<String>()? {
            let NoDupValue(value) = map.next_value()?;
            if obj.contains_key(&key) {
                return Err(de::Error::custom(format!("duplicate key '{key}'")));
            }
            obj.insert(key, value);
        }
        Ok(NoDupValue(Value::Object(obj)))
    }
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
    fn rejects_duplicate_keys_in_one_file() {
        assert!(serde_json::from_str::<NoDupValue>(r#"{"x":1,"x":2}"#).is_err());
        assert!(serde_json::from_str::<NoDupValue>(r#"{"a":{"b":1,"b":2}}"#).is_err());
        assert!(serde_json::from_str::<NoDupValue>(r#"{"a":1,"b":2}"#).is_ok());
    }

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
