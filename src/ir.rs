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
    /// For `Select` messages: the name of the param that drives the branch
    /// (e.g. `gender`). `None` for plain/plural.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selector: Option<String>,
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
    /// A closed set of string values (a `$select` selector, e.g. gender) — emitted
    /// as a literal union so call sites get autocomplete and typo-checking.
    Enum(Vec<String>),
}

#[derive(Serialize, Deserialize, Debug, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Plain,
    Plural,
    Select,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(untagged)]
pub enum MessageValue {
    Plain(String),
    /// Plural categories → text, and `$select` cases → text, share this shape.
    Branches(BTreeMap<String, String>),
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
            if let MessageValue::Branches(forms) = value {
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

/// Collect the dotted paths of every leaf (string, `$plural`, or `$select`).
fn collect_paths(node: &Value, prefix: &mut Vec<String>, out: &mut Vec<String>) {
    let Some(obj) = node.as_object() else {
        return;
    };
    for (k, v) in obj {
        prefix.push(k.clone());
        let is_leaf = v.is_string()
            || v.as_object()
                .is_some_and(|o| o.contains_key("$plural") || o.contains_key("$select"));
        if is_leaf {
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
            let values = collect_values(path, locales, canonical, Shape::Plain);
            out.push(Message {
                path: path.clone(),
                params,
                kind: Kind::Plain,
                values,
                selector: None,
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
            let values = collect_values(path, locales, canonical, Shape::Plural);
            out.push(Message {
                path: path.clone(),
                params,
                kind: Kind::Plural,
                values,
                selector: None,
            });
        } else if let Some(select) = val.as_object().and_then(|o| o.get("$select")) {
            // tagged select leaf: { "$select": { "param": "gender", "cases": {…} } }
            let dotted = path.join(".");
            let sobj = select.as_object().ok_or_else(|| {
                anyhow!("'{dotted}' $select must be an object with 'param' and 'cases'")
            })?;
            let param = sobj
                .get("param")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow!("'{dotted}' $select needs a string 'param'"))?;
            let cases = sobj
                .get("cases")
                .and_then(|v| v.as_object())
                .ok_or_else(|| anyhow!("'{dotted}' $select needs a 'cases' object"))?;
            if !cases.contains_key("other") {
                return Err(anyhow!(
                    "'{dotted}' $select is missing the required 'other' case"
                ));
            }
            // The selector is a closed enum of the case keys; placeholders from any
            // case become params too (unioned, same as plural).
            let case_keys: Vec<String> = cases.keys().cloned().collect();
            let mut params = vec![Param {
                name: param.to_string(),
                ty: ParamType::Enum(case_keys),
            }];
            for form in cases.values() {
                let s = form
                    .as_str()
                    .ok_or_else(|| anyhow!("'{dotted}' $select case must be a string"))?;
                for p in params_from(s) {
                    if p.name != param && !params.iter().any(|x| x.name == p.name) {
                        params.push(p);
                    }
                }
            }
            let values = collect_values(path, locales, canonical, Shape::Select);
            out.push(Message {
                path: path.clone(),
                params,
                kind: Kind::Select,
                values,
                selector: Some(param.to_string()),
            });
        } else if val.is_object() {
            // namespace
            walk(val, path, locales, canonical, out)?;
        }
        path.pop();
    }
    Ok(())
}

/// The placeholder names referenced in a string, in order, with duplicates kept.
/// Single source of the `{{name}}` extraction, shared with `stele check`.
pub fn placeholder_names(s: &str) -> Vec<String> {
    PLACEHOLDER
        .captures_iter(s)
        .map(|c| c[1].to_string())
        .collect()
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

/// Which leaf shape we're collecting a locale's value for.
#[derive(Clone, Copy)]
enum Shape {
    Plain,
    Plural,
    Select,
}

fn to_message_value(val: &Value, shape: Shape) -> Option<MessageValue> {
    let branch_map = |obj: &serde_json::Map<String, Value>| {
        obj.iter()
            .filter_map(|(k, v)| Some((k.clone(), normalize(v.as_str()?))))
            .collect()
    };
    match shape {
        Shape::Plain => Some(MessageValue::Plain(normalize(val.as_str()?))),
        Shape::Plural => {
            let forms = val.as_object()?.get("$plural")?.as_object()?;
            Some(MessageValue::Branches(branch_map(forms)))
        }
        Shape::Select => {
            let cases = val
                .as_object()?
                .get("$select")?
                .as_object()?
                .get("cases")?
                .as_object()?;
            Some(MessageValue::Branches(branch_map(cases)))
        }
    }
}

/// Collect each locale's value for `path`. Missing translations fall back to the
/// canonical locale and emit a loud warning, so output is always complete.
fn collect_values(
    path: &[String],
    locales: &BTreeMap<String, Value>,
    canonical: &str,
    shape: Shape,
) -> BTreeMap<String, MessageValue> {
    let mut out = BTreeMap::new();
    for (loc, root) in locales {
        let value = lookup(root, path).and_then(|v| to_message_value(v, shape));
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
                    .and_then(|v| to_message_value(v, shape))
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
    fn select_builds_enum_param_and_selector() {
        let ir = build_ir(
            "en",
            &locale(json!({ "g": { "$select": { "param": "gender", "cases": {
                "female": "{{name}} la invitó",
                "male": "{{name}} lo invitó",
                "other": "{{name}} le invitó"
            } } } })),
        )
        .unwrap();
        let m = ir.messages.iter().find(|m| m.dotted() == "g").unwrap();
        assert_eq!(m.kind, Kind::Select);
        assert_eq!(m.selector.as_deref(), Some("gender"));
        // selector is an Enum of the case keys; `name` is a normal placeholder param
        let gender = m.params.iter().find(|p| p.name == "gender").unwrap();
        assert_eq!(
            gender.ty,
            ParamType::Enum(vec!["female".into(), "male".into(), "other".into()])
        );
        assert!(m.params.iter().any(|p| p.name == "name"));
    }

    #[test]
    fn select_without_other_is_rejected() {
        assert!(build_ir(
            "en",
            &locale(json!({ "g": { "$select": { "param": "gender", "cases": { "male": "x" } } } }))
        )
        .is_err());
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
