//! Translator interchange: export the catalog to a translator-friendly format
//! (CSV or XLIFF 1.2) and import the completed file back into the JSON catalog.
//! The JSON stays the source of truth — the exchange file is just transport.
//!
//! Import is driven by the **canonical** catalog's structure: only the *strings*
//! come from the translation file, so a translator can never reshape a `$plural`
//! or rename a `$select` param. Plural categories on export use the *target*
//! locale's CLDR rules (Polish gets `few`/`many` slots English never had).

use crate::plural::build_plural_table;
use anyhow::{bail, Result};
use regex::Regex;
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::sync::LazyLock;

/// One translatable entry. `id` is the dotted key, with a `#category` / `#case`
/// suffix for an individual plural form or select case (e.g. `inbox.unread#one`).
pub struct Unit {
    pub id: String,
    pub source: String,
    pub target: String,
}

const SEP: char = '#';
const CATEGORIES: &[&str] = &["zero", "one", "two", "few", "many", "other"];

// --- Export ----------------------------------------------------------------

/// Build the translation units for `target_locale`: every leaf of the canonical
/// catalog, paired with the target's existing translation (or empty). Plural
/// leaves expand to the target locale's required categories.
pub fn export_units(
    canonical: &Value,
    target: Option<&Value>,
    target_locale: &str,
) -> Result<Vec<Unit>> {
    let cats = target_plural_categories(target_locale)?;
    let mut units = Vec::new();
    walk_export(canonical, target, &cats, &mut Vec::new(), &mut units);
    Ok(units)
}

/// The plural categories `locale`'s integer CLDR rules actually produce, ordered.
fn target_plural_categories(locale: &str) -> Result<Vec<String>> {
    let table = build_plural_table(locale)?;
    let present: std::collections::BTreeSet<&String> =
        table.small.iter().chain(table.modulo.iter()).collect();
    Ok(CATEGORIES
        .iter()
        .filter(|c| present.contains(&c.to_string()))
        .map(|c| c.to_string())
        .collect())
}

fn walk_export(
    canon: &Value,
    target: Option<&Value>,
    cats: &[String],
    path: &mut Vec<String>,
    out: &mut Vec<Unit>,
) {
    let Some(obj) = canon.as_object() else {
        return;
    };
    for (key, cval) in obj {
        path.push(key.clone());
        let tnode = target.and_then(|t| t.as_object()).and_then(|o| o.get(key));
        let dotted = path.join(".");
        if let Some(src) = cval.as_str() {
            let tgt = tnode.and_then(|v| v.as_str()).unwrap_or("");
            out.push(Unit {
                id: dotted,
                source: src.to_string(),
                target: tgt.to_string(),
            });
        } else if let Some(forms) = plural_forms(cval) {
            let tforms = tnode.and_then(plural_forms);
            for cat in cats {
                // source falls back to canonical `other` for a category English
                // doesn't have but the target needs.
                let src = forms
                    .get(cat)
                    .or_else(|| forms.get("other"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let tgt = tforms
                    .and_then(|f| f.get(cat))
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                out.push(Unit {
                    id: format!("{dotted}{SEP}{cat}"),
                    source: src.to_string(),
                    target: tgt.to_string(),
                });
            }
        } else if let Some(cases) = select_cases(cval) {
            let tcases = tnode.and_then(select_cases);
            for (case, sval) in cases {
                let src = sval.as_str().unwrap_or("");
                let tgt = tcases
                    .and_then(|c| c.get(case))
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                out.push(Unit {
                    id: format!("{dotted}{SEP}{case}"),
                    source: src.to_string(),
                    target: tgt.to_string(),
                });
            }
        } else if cval.is_object() {
            walk_export(cval, tnode, cats, path, out);
        }
        path.pop();
    }
}

fn plural_forms(v: &Value) -> Option<&Map<String, Value>> {
    v.as_object()?.get("$plural")?.as_object()
}

fn select_cases(v: &Value) -> Option<&Map<String, Value>> {
    v.as_object()?
        .get("$select")?
        .as_object()?
        .get("cases")?
        .as_object()
}

// --- Import ----------------------------------------------------------------

/// Rebuild the target locale as `(path, leaf-value)` pairs, using the canonical
/// catalog for structure and the units for the translated strings. Entries with
/// no (non-empty) translation are omitted, so the result is a partial overlay.
pub fn build_paths(canonical: &Value, units: &[Unit]) -> Vec<(Vec<String>, Value)> {
    let map: HashMap<&str, &str> = units
        .iter()
        .filter(|u| !u.target.is_empty())
        .map(|u| (u.id.as_str(), u.target.as_str()))
        .collect();
    let mut out = Vec::new();
    walk_build(canonical, &map, &mut Vec::new(), &mut out);
    out
}

fn walk_build(
    canon: &Value,
    map: &HashMap<&str, &str>,
    path: &mut Vec<String>,
    out: &mut Vec<(Vec<String>, Value)>,
) {
    let Some(obj) = canon.as_object() else {
        return;
    };
    for (key, cval) in obj {
        path.push(key.clone());
        let dotted = path.join(".");
        if cval.is_string() {
            if let Some(t) = map.get(dotted.as_str()) {
                out.push((path.clone(), Value::String(t.to_string())));
            }
        } else if plural_forms(cval).is_some() {
            let mut forms = Map::new();
            for cat in CATEGORIES {
                if let Some(t) = map.get(format!("{dotted}{SEP}{cat}").as_str()) {
                    forms.insert(cat.to_string(), Value::String(t.to_string()));
                }
            }
            if !forms.is_empty() {
                out.push((path.clone(), json!({ "$plural": forms })));
            }
        } else if let Some(cases_obj) = select_cases(cval) {
            let param = cval["$select"]
                .get("param")
                .and_then(|p| p.as_str())
                .unwrap_or("value");
            let mut cases = Map::new();
            for case in cases_obj.keys() {
                if let Some(t) = map.get(format!("{dotted}{SEP}{case}").as_str()) {
                    cases.insert(case.clone(), Value::String(t.to_string()));
                }
            }
            if !cases.is_empty() {
                out.push((
                    path.clone(),
                    json!({ "$select": { "param": param, "cases": cases } }),
                ));
            }
        } else if cval.is_object() {
            walk_build(cval, map, path, out);
        }
        path.pop();
    }
}

/// Set `path` to `value` in a JSON tree, creating intermediate objects. Replaces
/// the whole leaf (so a re-imported `$plural` doesn't union with an old one).
pub fn set_path(root: &mut Value, path: &[String], value: Value) {
    if !root.is_object() {
        *root = Value::Object(Map::new());
    }
    let obj = root.as_object_mut().unwrap();
    if path.len() == 1 {
        obj.insert(path[0].clone(), value);
        return;
    }
    let child = obj
        .entry(path[0].clone())
        .or_insert_with(|| Value::Object(Map::new()));
    set_path(child, &path[1..], value);
}

// --- CSV -------------------------------------------------------------------

pub fn to_csv(canonical_locale: &str, target_locale: &str, units: &[Unit]) -> Result<String> {
    let mut wtr = csv::Writer::from_writer(Vec::new());
    wtr.write_record(["id", canonical_locale, target_locale])?;
    for u in units {
        wtr.write_record([&u.id, &u.source, &u.target])?;
    }
    Ok(String::from_utf8(wtr.into_inner()?)?)
}

/// Parse a CSV back. Returns the target locale (column-3 header) and the units.
pub fn from_csv(text: &str) -> Result<(String, Vec<Unit>)> {
    let mut rdr = csv::Reader::from_reader(text.as_bytes());
    let target = rdr.headers()?.get(2).unwrap_or("target").to_string();
    let mut units = Vec::new();
    for rec in rdr.records() {
        let rec = rec?;
        let id = rec.get(0).unwrap_or("").to_string();
        if id.is_empty() {
            continue;
        }
        units.push(Unit {
            id,
            source: rec.get(1).unwrap_or("").to_string(),
            target: rec.get(2).unwrap_or("").to_string(),
        });
    }
    Ok((target, units))
}

// --- XLIFF 1.2 -------------------------------------------------------------

pub fn to_xliff(canonical_locale: &str, target_locale: &str, units: &[Unit]) -> String {
    let mut s = String::new();
    s.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<xliff version=\"1.2\">\n");
    s.push_str(&format!(
        "  <file original=\"stele\" source-language=\"{canonical_locale}\" target-language=\"{target_locale}\" datatype=\"plaintext\">\n    <body>\n"
    ));
    for u in units {
        s.push_str(&format!(
            "      <trans-unit id=\"{}\">\n        <source>{}</source>\n        <target>{}</target>\n      </trans-unit>\n",
            xml_attr(&u.id),
            xml_text(&u.source),
            xml_text(&u.target),
        ));
    }
    s.push_str("    </body>\n  </file>\n</xliff>\n");
    s
}

static TRANS_UNIT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?s)<trans-unit\b[^>]*\bid="([^"]*)"[^>]*>(.*?)</trans-unit>"#).unwrap()
});
static SOURCE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?s)<source\b[^>]*>(.*?)</source>").unwrap());
static TARGET: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?s)<target\b[^>]*>(.*?)</target>").unwrap());
static TARGET_LANG: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"target-language="([^"]*)""#).unwrap());

/// Parse XLIFF 1.2 (as emitted here, or a TMS round-trip with literal-text
/// placeholders). Returns the target language and the units.
pub fn from_xliff(text: &str) -> Result<(String, Vec<Unit>)> {
    let target_lang = TARGET_LANG
        .captures(text)
        .map(|c| c[1].to_string())
        .unwrap_or_default();
    let mut units = Vec::new();
    for cap in TRANS_UNIT.captures_iter(text) {
        let id = xml_unescape(&cap[1]);
        let inner = &cap[2];
        let source = SOURCE
            .captures(inner)
            .map(|c| xml_unescape(&c[1]))
            .unwrap_or_default();
        let target = TARGET
            .captures(inner)
            .map(|c| xml_unescape(&c[1]))
            .unwrap_or_default();
        if id.is_empty() {
            continue;
        }
        units.push(Unit { id, source, target });
    }
    Ok((target_lang, units))
}

fn xml_text(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn xml_attr(s: &str) -> String {
    xml_text(s).replace('"', "&quot;")
}

fn xml_unescape(s: &str) -> String {
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

/// Validate that a format string is one we support.
pub fn parse_format(s: &str) -> Result<&'static str> {
    match s.to_lowercase().as_str() {
        "csv" => Ok("csv"),
        "xliff" | "xlf" => Ok("xliff"),
        other => bail!("unknown format '{other}' (use: csv | xliff)"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn catalog() -> Value {
        json!({
            "home": { "hi": "Hi {{name}}" },
            "n": { "$plural": { "one": "{{count}} dog", "other": "{{count}} dogs" } },
            "g": { "$select": { "param": "gender",
                "cases": { "female": "her", "male": "his", "other": "their" } } }
        })
    }

    #[test]
    fn export_expands_plurals_to_target_categories() {
        // Polish needs one/few/many/other — more than English's one/other.
        let units = export_units(&catalog(), None, "pl").unwrap();
        let ids: Vec<_> = units.iter().map(|u| u.id.as_str()).collect();
        assert!(ids.contains(&"n#few"));
        assert!(ids.contains(&"n#many"));
        // select cases come through
        assert!(ids.contains(&"g#female"));
        assert!(ids.contains(&"home.hi"));
    }

    #[test]
    fn csv_round_trips() {
        let units = export_units(&catalog(), None, "es").unwrap();
        let csv = to_csv("en", "es", &units).unwrap();
        let (loc, back) = from_csv(&csv).unwrap();
        assert_eq!(loc, "es");
        assert_eq!(back.len(), units.len());
        assert_eq!(back[0].id, units[0].id);
    }

    #[test]
    fn xliff_round_trips_with_escaping() {
        let units = vec![Unit {
            id: "a#one".into(),
            source: "x < y & \"z\"".into(),
            target: "ä < ö".into(),
        }];
        let xliff = to_xliff("en", "es", &units);
        let (loc, back) = from_xliff(&xliff).unwrap();
        assert_eq!(loc, "es");
        assert_eq!(back[0].id, "a#one");
        assert_eq!(back[0].source, "x < y & \"z\"");
        assert_eq!(back[0].target, "ä < ö");
    }

    #[test]
    fn import_rebuilds_structure_from_canonical() {
        let units = vec![
            Unit {
                id: "home.hi".into(),
                source: "Hi".into(),
                target: "Hola {{name}}".into(),
            },
            Unit {
                id: "n#one".into(),
                source: "".into(),
                target: "{{count}} perro".into(),
            },
            Unit {
                id: "n#other".into(),
                source: "".into(),
                target: "{{count}} perros".into(),
            },
            Unit {
                id: "g#female".into(),
                source: "".into(),
                target: "suya".into(),
            },
            Unit {
                id: "g#other".into(),
                source: "".into(),
                target: "suyo".into(),
            },
        ];
        let paths = build_paths(&catalog(), &units);
        let mut root = Value::Object(Map::new());
        for (p, v) in paths {
            set_path(&mut root, &p, v);
        }
        // plural reconstructed under $plural
        assert_eq!(root["n"]["$plural"]["one"], json!("{{count}} perro"));
        // select reconstructed with the canonical param preserved
        assert_eq!(root["g"]["$select"]["param"], json!("gender"));
        assert_eq!(root["g"]["$select"]["cases"]["female"], json!("suya"));
        assert_eq!(root["home"]["hi"], json!("Hola {{name}}"));
    }

    #[test]
    fn import_omits_untranslated() {
        let units = vec![Unit {
            id: "home.hi".into(),
            source: "Hi".into(),
            target: "".into(),
        }];
        assert!(build_paths(&catalog(), &units).is_empty());
    }
}
