use anyhow::{anyhow, Result};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, HashSet};
use std::sync::LazyLock;

/// Matches `{{ name }}` placeholders — double-brace, whitespace tolerant. Single
/// braces are left alone, so literal `{x}` in copy passes through untouched.
static PLACEHOLDER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\{\{\s*(\w+)\s*\}\}").unwrap());

/// Canonicalize placeholders to `{{name}}` (no inner spaces) at generate time, so
/// every emitter's runtime interpolation can match the same strict token and the
/// TS and Swift runtimes can never drift.
fn normalize(s: &str) -> String {
    PLACEHOLDER.replace_all(s, "{{${1}}}").into_owned()
}

/// The language-neutral intermediate representation. This is the contract every
/// emitter consumes; it serializes to/from JSON so emitters can live in any
/// language (the protoc-plugin model).
#[derive(Serialize, Deserialize, Debug)]
pub struct Ir {
    pub canonical: String,
    pub locales: Vec<String>,
    pub messages: Vec<Message>,
    /// Per-locale baked plural-category tables, resolved from CLDR via ICU4X.
    pub plural_rules: BTreeMap<String, PluralTable>,
}

/// A baked plural-category lookup for one locale. `small[n]` covers n in 0..100;
/// `modulo[n % 100]` covers n >= 100. `categories` is the set of categories the
/// locale actually uses (from CLDR), useful for validating `$plural` coverage.
#[derive(Serialize, Deserialize, Debug)]
pub struct PluralTable {
    pub categories: Vec<String>,
    pub small: Vec<String>,
    pub modulo: Vec<String>,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct Message {
    pub path: Vec<String>,
    pub params: Vec<Param>,
    pub kind: Kind,
    pub values: BTreeMap<String, MessageValue>,
}

impl Message {
    pub fn dotted(&self) -> String {
        self.path.join(".")
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Param {
    pub name: String,
    #[serde(rename = "type")]
    pub ty: ParamType,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum ParamType {
    String,
    Number,
}

#[derive(Serialize, Deserialize, Debug, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Plain,
    Plural,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(untagged)]
pub enum MessageValue {
    Plain(String),
    Plural(BTreeMap<String, String>),
}

pub fn build_ir(canonical: &str, locales: &BTreeMap<String, Value>) -> Result<Ir> {
    let canon = locales
        .get(canonical)
        .ok_or_else(|| anyhow!("canonical locale '{}' not found", canonical))?;
    let mut messages = Vec::new();
    let mut path = Vec::new();
    walk(canon, &mut path, locales, canonical, &mut messages)?;

    let mut plural_rules = BTreeMap::new();
    for loc in locales.keys() {
        plural_rules.insert(loc.clone(), crate::plural::build_plural_table(loc)?);
    }

    // Warn if a plural omits a category its locale's CLDR rules actually produce
    // (e.g. Polish without `few` → blank string at runtime for counts like 2).
    for m in &messages {
        if m.kind != Kind::Plural {
            continue;
        }
        for (loc, value) in &m.values {
            if let MessageValue::Plural(forms) = value {
                // Only the categories the locale's INTEGER rules actually produce
                // (from the baked tables) — not ICU's full set, which includes
                // compact-only categories like Spanish `many` that integers never hit.
                let table = &plural_rules[loc];
                let reachable: HashSet<&String> =
                    table.small.iter().chain(table.modulo.iter()).collect();
                for cat in reachable {
                    if !forms.contains_key(cat) {
                        eprintln!(
                            "warning: '{}' [{}] is missing the '{}' plural form this locale needs",
                            m.dotted(),
                            loc,
                            cat
                        );
                    }
                }
            }
        }
    }

    // Warn about keys present only in a non-canonical locale — they're silently
    // dropped (the catalog shape is defined by the canonical locale).
    let canonical_paths: HashSet<String> = messages.iter().map(|m| m.dotted()).collect();
    for (loc, root) in locales {
        if loc == canonical {
            continue;
        }
        let mut found = Vec::new();
        collect_paths(root, &mut Vec::new(), &mut found);
        for p in found {
            if !canonical_paths.contains(&p) {
                eprintln!(
                    "warning: key '{p}' exists in locale '{loc}' but not in canonical '{canonical}' — it will be ignored"
                );
            }
        }
    }

    Ok(Ir {
        canonical: canonical.to_string(),
        locales: locales.keys().cloned().collect(),
        messages,
        plural_rules,
    })
}

/// Collect the dotted paths of every leaf (string or `$plural`) under `node`.
fn collect_paths(node: &Value, prefix: &mut Vec<String>, out: &mut Vec<String>) {
    let Some(obj) = node.as_object() else {
        return;
    };
    for (k, v) in obj {
        prefix.push(k.clone());
        if v.is_string() || v.as_object().is_some_and(|o| o.contains_key("$plural")) {
            out.push(prefix.join("."));
        } else if v.is_object() {
            collect_paths(v, prefix, out);
        }
        prefix.pop();
    }
}

fn walk(
    node: &Value,
    path: &mut Vec<String>,
    locales: &BTreeMap<String, Value>,
    canonical: &str,
    out: &mut Vec<Message>,
) -> Result<()> {
    let obj = node
        .as_object()
        .ok_or_else(|| anyhow!("expected object at '{}'", path.join(".")))?;
    for (key, val) in obj {
        path.push(key.clone());
        if let Some(s) = val.as_str() {
            // plain string leaf
            let params = params_from(s);
            let values = collect_values(path, locales, canonical, false);
            out.push(Message {
                path: path.clone(),
                params,
                kind: Kind::Plain,
                values,
            });
        } else if let Some(plural) = val.as_object().and_then(|o| o.get("$plural")) {
            // tagged plural leaf: { "$plural": { "one": ..., "other": ... } }
            let dotted = path.join(".");
            let forms = plural.as_object().ok_or_else(|| {
                anyhow!("'{dotted}' $plural must be an object of category → string")
            })?;
            const CATEGORIES: &[&str] = &["zero", "one", "two", "few", "many", "other"];
            if !forms.contains_key("other") {
                return Err(anyhow!(
                    "'{dotted}' $plural is missing the required 'other' form"
                ));
            }
            // params: `count` plus every placeholder used in ANY form (not just `other`).
            let mut params = vec![Param {
                name: "count".into(),
                ty: ParamType::Number,
            }];
            for (cat, form) in forms {
                if !CATEGORIES.contains(&cat.as_str()) {
                    return Err(anyhow!(
                        "'{dotted}' $plural has unknown category '{cat}' (valid: zero one two few many other)"
                    ));
                }
                let s = form
                    .as_str()
                    .ok_or_else(|| anyhow!("'{dotted}' $plural form '{cat}' must be a string"))?;
                for p in params_from(s) {
                    if p.name != "count" && !params.iter().any(|x| x.name == p.name) {
                        params.push(p);
                    }
                }
            }
            let values = collect_values(path, locales, canonical, true);
            out.push(Message {
                path: path.clone(),
                params,
                kind: Kind::Plural,
                values,
            });
        } else if val.is_object() {
            // namespace
            walk(val, path, locales, canonical, out)?;
        }
        path.pop();
    }
    Ok(())
}

fn params_from(s: &str) -> Vec<Param> {
    let mut seen: Vec<Param> = Vec::new();
    for cap in PLACEHOLDER.captures_iter(s) {
        let name = cap[1].to_string();
        if seen.iter().any(|p| p.name == name) {
            continue;
        }
        let ty = if name == "count" {
            ParamType::Number
        } else {
            ParamType::String
        };
        seen.push(Param { name, ty });
    }
    seen
}

fn lookup<'a>(root: &'a Value, path: &[String]) -> Option<&'a Value> {
    let mut cur = root;
    for seg in path {
        cur = cur.as_object()?.get(seg)?;
    }
    Some(cur)
}

fn to_message_value(val: &Value, plural: bool) -> Option<MessageValue> {
    if plural {
        let m = val.as_object()?.get("$plural")?.as_object()?;
        let forms = m
            .iter()
            .filter_map(|(k, v)| Some((k.clone(), normalize(v.as_str()?))))
            .collect();
        Some(MessageValue::Plural(forms))
    } else {
        Some(MessageValue::Plain(normalize(val.as_str()?)))
    }
}

/// Collect each locale's value for `path`. Missing translations fall back to the
/// canonical locale and emit a loud warning, so output is always complete.
fn collect_values(
    path: &[String],
    locales: &BTreeMap<String, Value>,
    canonical: &str,
    plural: bool,
) -> BTreeMap<String, MessageValue> {
    let mut out = BTreeMap::new();
    for (loc, root) in locales {
        let value = lookup(root, path).and_then(|v| to_message_value(v, plural));
        let resolved = match value {
            Some(v) => v,
            None => {
                if loc != canonical {
                    eprintln!(
                        "warning: locale '{}' missing key '{}' — falling back to '{}'",
                        loc,
                        path.join("."),
                        canonical
                    );
                }
                lookup(&locales[canonical], path)
                    .and_then(|v| to_message_value(v, plural))
                    .unwrap_or(MessageValue::Plain(String::new()))
            }
        };
        out.insert(loc.clone(), resolved);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn locale(v: Value) -> BTreeMap<String, Value> {
        BTreeMap::from([("en".to_string(), v)])
    }

    #[test]
    fn extracts_double_brace_params_whitespace_tolerant() {
        let names: Vec<_> = params_from("Hi {{name}}, {{ count }} nearby")
            .into_iter()
            .map(|p| p.name)
            .collect();
        assert_eq!(names, vec!["name", "count"]);
    }

    #[test]
    fn normalize_canonicalizes_spacing_and_leaves_single_braces() {
        assert_eq!(normalize("Hi {{ name }}"), "Hi {{name}}");
        assert_eq!(normalize("{{count}} dogs"), "{{count}} dogs");
        // literal single braces are NOT placeholders — pass through untouched
        assert_eq!(normalize("set it to {x}"), "set it to {x}");
    }

    #[test]
    fn plural_params_union_across_all_forms() {
        let ir = build_ir(
            "en",
            &locale(json!({ "m": { "$plural": {
                "one": "{{name}} has {{count}}",
                "other": "{{count}} items"
            } } })),
        )
        .unwrap();
        let m = ir.messages.iter().find(|m| m.dotted() == "m").unwrap();
        let names: Vec<_> = m.params.iter().map(|p| p.name.as_str()).collect();
        // `name` appears only in the `one` form but must still be a param.
        assert!(names.contains(&"name") && names.contains(&"count"));
    }

    #[test]
    fn malformed_plural_is_rejected() {
        assert!(build_ir("en", &locale(json!({ "p": { "$plural": "nope" } }))).is_err());
        assert!(build_ir("en", &locale(json!({ "p": { "$plural": { "one": "x" } } }))).is_err());
        assert!(build_ir(
            "en",
            &locale(json!({ "p": { "$plural": { "banana": "x", "other": "y" } } }))
        )
        .is_err());
    }
}
