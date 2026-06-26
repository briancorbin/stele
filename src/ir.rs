use anyhow::{anyhow, Result};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

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
    let placeholder = Regex::new(r"\{(\w+)\}").unwrap();
    let mut messages = Vec::new();
    let mut path = Vec::new();
    walk(canon, &mut path, locales, canonical, &placeholder, &mut messages)?;

    let mut plural_rules = BTreeMap::new();
    for loc in locales.keys() {
        plural_rules.insert(loc.clone(), crate::plural::build_plural_table(loc)?);
    }

    Ok(Ir {
        canonical: canonical.to_string(),
        locales: locales.keys().cloned().collect(),
        messages,
        plural_rules,
    })
}

fn walk(
    node: &Value,
    path: &mut Vec<String>,
    locales: &BTreeMap<String, Value>,
    canonical: &str,
    ph: &Regex,
    out: &mut Vec<Message>,
) -> Result<()> {
    let obj = node
        .as_object()
        .ok_or_else(|| anyhow!("expected object at '{}'", path.join(".")))?;
    for (key, val) in obj {
        path.push(key.clone());
        if let Some(s) = val.as_str() {
            // plain string leaf
            let params = params_from(s, ph);
            let values = collect_values(path, locales, canonical, false);
            out.push(Message {
                path: path.clone(),
                params,
                kind: Kind::Plain,
                values,
            });
        } else if let Some(forms) = val.as_object().and_then(|o| o.get("$plural")) {
            // tagged plural leaf: { "$plural": { "one": ..., "other": ... } }
            let other = forms
                .as_object()
                .and_then(|m| m.get("other"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let mut params = vec![Param {
                name: "count".into(),
                ty: ParamType::Number,
            }];
            for p in params_from(other, ph) {
                if p.name != "count" {
                    params.push(p);
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
            walk(val, path, locales, canonical, ph, out)?;
        }
        path.pop();
    }
    Ok(())
}

fn params_from(s: &str, ph: &Regex) -> Vec<Param> {
    let mut seen: Vec<Param> = Vec::new();
    for cap in ph.captures_iter(s) {
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
            .filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_string())))
            .collect();
        Some(MessageValue::Plural(forms))
    } else {
        Some(MessageValue::Plain(val.as_str()?.to_string()))
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
