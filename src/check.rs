//! `stele check` — validate a catalog across locales without generating code.
//! Catches the bugs that make a multi-locale catalog ship broken: missing or
//! drifted translations, placeholder mismatches that crash interpolation, and
//! plural forms a locale's CLDR rules require but the catalog omits. Produces
//! structured diagnostics so the CLI can render a report and gate CI.

use crate::ir::placeholder_names;
use crate::plural::build_plural_table;
use anyhow::Result;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, PartialEq, Clone, Copy)]
pub enum Severity {
    Error,
    Warning,
}

#[derive(Debug)]
pub struct Diagnostic {
    pub severity: Severity,
    pub locale: String,
    pub key: String,
    pub message: String,
}

impl Diagnostic {
    fn error(locale: &str, key: &str, message: impl Into<String>) -> Self {
        Diagnostic {
            severity: Severity::Error,
            locale: locale.to_string(),
            key: key.to_string(),
            message: message.into(),
        }
    }
    fn warning(locale: &str, key: &str, message: impl Into<String>) -> Self {
        Diagnostic {
            severity: Severity::Warning,
            locale: locale.to_string(),
            key: key.to_string(),
            message: message.into(),
        }
    }
}

/// The result of checking a catalog: the canonical key count plus every issue.
pub struct Report {
    pub key_count: usize,
    pub diagnostics: Vec<Diagnostic>,
}

impl Report {
    pub fn errors(&self) -> usize {
        self.count(Severity::Error)
    }
    pub fn warnings(&self) -> usize {
        self.count(Severity::Warning)
    }
    fn count(&self, sev: Severity) -> usize {
        self.diagnostics
            .iter()
            .filter(|d| d.severity == sev)
            .count()
    }
}

#[derive(PartialEq, Clone, Copy)]
enum Kind {
    Plain,
    Plural,
    Select,
}

/// A flattened catalog leaf with the placeholder names it references. For plurals
/// `branches` holds the categories provided; for selects it holds the case names
/// and `selector` the param driving them.
struct Leaf {
    kind: Kind,
    placeholders: BTreeSet<String>,
    branches: BTreeSet<String>,
    selector: Option<String>,
}

const CATEGORIES: &[&str] = &["zero", "one", "two", "few", "many", "other"];

/// Flatten one locale tree into `dotted-key → Leaf`, collecting any malformed
/// `$plural` shapes as `(key, message)` errors rather than failing outright — so
/// `check` reports every problem at once instead of stopping at the first.
fn flatten(root: &Value) -> (BTreeMap<String, Leaf>, Vec<(String, String)>) {
    let mut leaves = BTreeMap::new();
    let mut errors = Vec::new();
    let mut path = Vec::new();
    walk(root, &mut path, &mut leaves, &mut errors);
    (leaves, errors)
}

fn walk(
    node: &Value,
    path: &mut Vec<String>,
    leaves: &mut BTreeMap<String, Leaf>,
    errors: &mut Vec<(String, String)>,
) {
    let Some(obj) = node.as_object() else {
        return;
    };
    for (k, v) in obj {
        path.push(k.clone());
        let dotted = path.join(".");
        if let Some(s) = v.as_str() {
            leaves.insert(
                dotted,
                Leaf {
                    kind: Kind::Plain,
                    placeholders: placeholder_names(s).into_iter().collect(),
                    branches: BTreeSet::new(),
                    selector: None,
                },
            );
        } else if let Some(plural) = v.as_object().and_then(|o| o.get("$plural")) {
            match plural.as_object() {
                None => errors.push((
                    dotted.clone(),
                    "$plural must be an object of category → string".to_string(),
                )),
                Some(forms_obj) => {
                    let mut placeholders = BTreeSet::new();
                    let mut branches = BTreeSet::new();
                    for (cat, form) in forms_obj {
                        if !CATEGORIES.contains(&cat.as_str()) {
                            errors.push((
                                dotted.clone(),
                                format!("unknown plural category '{cat}' (valid: zero one two few many other)"),
                            ));
                            continue;
                        }
                        branches.insert(cat.clone());
                        match form.as_str() {
                            Some(fs) => placeholders.extend(placeholder_names(fs)),
                            None => errors.push((
                                dotted.clone(),
                                format!("plural form '{cat}' must be a string"),
                            )),
                        }
                    }
                    if !branches.contains("other") {
                        errors.push((
                            dotted.clone(),
                            "$plural is missing the required 'other' form".to_string(),
                        ));
                    }
                    leaves.insert(
                        dotted,
                        Leaf {
                            kind: Kind::Plural,
                            placeholders,
                            branches,
                            selector: None,
                        },
                    );
                }
            }
        } else if let Some(select) = v.as_object().and_then(|o| o.get("$select")) {
            let sobj = select.as_object();
            let param = sobj.and_then(|o| o.get("param")).and_then(|p| p.as_str());
            let cases = sobj
                .and_then(|o| o.get("cases"))
                .and_then(|c| c.as_object());
            match (param, cases) {
                (Some(param), Some(cases_obj)) => {
                    let mut placeholders = BTreeSet::new();
                    let mut branches = BTreeSet::new();
                    for (case, form) in cases_obj {
                        branches.insert(case.clone());
                        match form.as_str() {
                            Some(fs) => placeholders.extend(placeholder_names(fs)),
                            None => errors.push((
                                dotted.clone(),
                                format!("select case '{case}' must be a string"),
                            )),
                        }
                    }
                    if !branches.contains("other") {
                        errors.push((
                            dotted.clone(),
                            "$select is missing the required 'other' case".to_string(),
                        ));
                    }
                    leaves.insert(
                        dotted,
                        Leaf {
                            kind: Kind::Select,
                            placeholders,
                            branches,
                            selector: Some(param.to_string()),
                        },
                    );
                }
                _ => errors.push((
                    dotted.clone(),
                    "$select needs a string 'param' and a 'cases' object".to_string(),
                )),
            }
        } else if v.is_object() {
            walk(v, path, leaves, errors);
        }
        path.pop();
    }
}

// `count` is the implicit plural argument and is always available, so whether a
// given translation literally interpolates it is a translation choice, not drift.
fn non_count(set: &BTreeSet<String>) -> BTreeSet<&String> {
    set.iter().filter(|p| p.as_str() != "count").collect()
}

fn kind_label(kind: Kind) -> &'static str {
    match kind {
        Kind::Plain => "a plain string",
        Kind::Plural => "a $plural",
        Kind::Select => "a $select",
    }
}

/// Every plural in `leaves` must provide each category that `locale`'s integer
/// CLDR rules can actually produce (e.g. Polish needs `few`/`many`), or a count
/// hits a blank string at runtime.
fn check_plural_coverage(
    locale: &str,
    leaves: &BTreeMap<String, Leaf>,
    diags: &mut Vec<Diagnostic>,
) -> Result<()> {
    let table = build_plural_table(locale)?;
    let reachable: BTreeSet<&String> = table.small.iter().chain(table.modulo.iter()).collect();
    for (key, leaf) in leaves {
        if leaf.kind != Kind::Plural {
            continue;
        }
        for cat in &reachable {
            if !leaf.branches.contains(*cat) {
                diags.push(Diagnostic::error(
                    locale,
                    key,
                    format!("missing plural form '{cat}' that '{locale}' requires"),
                ));
            }
        }
    }
    Ok(())
}

/// Validate the whole catalog. The canonical locale defines the reference shape;
/// every other locale is compared against it.
pub fn check(canonical: &str, locales: &BTreeMap<String, Value>) -> Result<Report> {
    let mut diags = Vec::new();

    let canon_root = locales
        .get(canonical)
        .ok_or_else(|| anyhow::anyhow!("canonical locale '{canonical}' not found"))?;
    let (canon_leaves, canon_errors) = flatten(canon_root);
    for (key, msg) in canon_errors {
        diags.push(Diagnostic::error(canonical, &key, msg));
    }
    check_plural_coverage(canonical, &canon_leaves, &mut diags)?;

    for (loc, root) in locales {
        if loc == canonical {
            continue;
        }
        let (leaves, errors) = flatten(root);
        for (key, msg) in errors {
            diags.push(Diagnostic::error(loc, &key, msg));
        }

        for (key, canon) in &canon_leaves {
            match leaves.get(key) {
                None => diags.push(Diagnostic::error(loc, key, "missing translation")),
                Some(leaf) if leaf.kind != canon.kind => {
                    diags.push(Diagnostic::error(
                        loc,
                        key,
                        format!(
                            "kind mismatch: canonical is {}, here it's {}",
                            kind_label(canon.kind),
                            kind_label(leaf.kind)
                        ),
                    ));
                }
                Some(leaf) => {
                    let canon_ph = non_count(&canon.placeholders);
                    let leaf_ph = non_count(&leaf.placeholders);
                    // A placeholder the locale uses but canonical doesn't → the
                    // caller never passes it → `undefined` at runtime. Hard error.
                    for p in leaf_ph.difference(&canon_ph) {
                        diags.push(Diagnostic::error(
                            loc,
                            key,
                            format!("placeholder {{{{{p}}}}} is not in the canonical string — it will be undefined at runtime"),
                        ));
                    }
                    // A canonical placeholder the locale drops → not a crash, but
                    // usually a translation oversight. Warn.
                    for p in canon_ph.difference(&leaf_ph) {
                        diags.push(Diagnostic::warning(
                            loc,
                            key,
                            format!("placeholder {{{{{p}}}}} from the canonical string is not used here"),
                        ));
                    }
                    // For $select, the case set + selector are caller-facing and
                    // language-independent — every locale must match the canonical.
                    if canon.kind == Kind::Select {
                        if leaf.selector != canon.selector {
                            diags.push(Diagnostic::error(
                                loc,
                                key,
                                format!(
                                    "select param '{}' differs from canonical '{}'",
                                    leaf.selector.as_deref().unwrap_or("?"),
                                    canon.selector.as_deref().unwrap_or("?")
                                ),
                            ));
                        }
                        for c in canon.branches.difference(&leaf.branches) {
                            diags.push(Diagnostic::error(
                                loc,
                                key,
                                format!("missing select case '{c}' (the caller can pass it)"),
                            ));
                        }
                        for c in leaf.branches.difference(&canon.branches) {
                            diags.push(Diagnostic::warning(
                                loc,
                                key,
                                format!("select case '{c}' is not in the canonical locale — it's unreachable"),
                            ));
                        }
                    }
                }
            }
        }

        for key in leaves.keys() {
            if !canon_leaves.contains_key(key) {
                diags.push(Diagnostic::warning(
                    loc,
                    key,
                    "key is not in the canonical locale — it will be ignored",
                ));
            }
        }

        check_plural_coverage(loc, &leaves, &mut diags)?;
    }

    Ok(Report {
        key_count: canon_leaves.len(),
        diagnostics: diags,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn cat(pairs: &[(&str, Value)]) -> BTreeMap<String, Value> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect()
    }

    fn has_error(r: &Report, key: &str, needle: &str) -> bool {
        r.diagnostics
            .iter()
            .any(|d| d.severity == Severity::Error && d.key == key && d.message.contains(needle))
    }

    #[test]
    fn clean_catalog_has_no_errors() {
        let r = check(
            "en",
            &cat(&[
                ("en", json!({ "home": { "hi": "Hi {{name}}" } })),
                ("es", json!({ "home": { "hi": "Hola {{name}}" } })),
            ]),
        )
        .unwrap();
        assert_eq!(r.errors(), 0);
        assert_eq!(r.key_count, 1);
    }

    #[test]
    fn flags_missing_translation() {
        let r = check(
            "en",
            &cat(&[
                ("en", json!({ "a": "x", "b": "y" })),
                ("es", json!({ "a": "x" })),
            ]),
        )
        .unwrap();
        assert!(has_error(&r, "b", "missing translation"));
    }

    #[test]
    fn flags_placeholder_not_in_canonical_as_error() {
        let r = check(
            "en",
            &cat(&[
                ("en", json!({ "hi": "Hi {{name}}" })),
                ("es", json!({ "hi": "Hola {{nombre}}" })),
            ]),
        )
        .unwrap();
        assert!(has_error(&r, "hi", "nombre"));
    }

    #[test]
    fn dropped_placeholder_is_a_warning_not_error() {
        let r = check(
            "en",
            &cat(&[
                ("en", json!({ "hi": "Hi {{name}}" })),
                ("es", json!({ "hi": "Hola" })),
            ]),
        )
        .unwrap();
        assert_eq!(r.errors(), 0);
        assert_eq!(r.warnings(), 1);
    }

    #[test]
    fn extra_key_is_a_warning() {
        let r = check(
            "en",
            &cat(&[
                ("en", json!({ "a": "x" })),
                ("es", json!({ "a": "x", "ghost": "boo" })),
            ]),
        )
        .unwrap();
        assert_eq!(r.errors(), 0);
        assert!(r
            .diagnostics
            .iter()
            .any(|d| d.key == "ghost" && d.severity == Severity::Warning));
    }

    #[test]
    fn flags_missing_plural_category_for_locale() {
        // Polish integer rules need few/many; providing only one/other is incomplete.
        let r = check(
            "en",
            &cat(&[
                (
                    "en",
                    json!({ "n": { "$plural": { "one": "{{count}}", "other": "{{count}}" } } }),
                ),
                (
                    "pl",
                    json!({ "n": { "$plural": { "one": "{{count}}", "other": "{{count}}" } } }),
                ),
            ]),
        )
        .unwrap();
        assert!(has_error(&r, "n", "few") || has_error(&r, "n", "many"));
    }

    #[test]
    fn flags_missing_select_case() {
        let r = check(
            "en",
            &cat(&[
                (
                    "en",
                    json!({ "g": { "$select": { "param": "gender",
                    "cases": { "female": "f", "male": "m", "other": "o" } } } }),
                ),
                // es drops the "female" case the caller can still pass
                (
                    "es",
                    json!({ "g": { "$select": { "param": "gender",
                    "cases": { "male": "m", "other": "o" } } } }),
                ),
            ]),
        )
        .unwrap();
        assert!(has_error(&r, "g", "female"));
    }

    #[test]
    fn flags_select_param_mismatch() {
        let r = check(
            "en",
            &cat(&[
                (
                    "en",
                    json!({ "g": { "$select": { "param": "gender",
                    "cases": { "other": "o" } } } }),
                ),
                (
                    "es",
                    json!({ "g": { "$select": { "param": "genero",
                    "cases": { "other": "o" } } } }),
                ),
            ]),
        )
        .unwrap();
        assert!(has_error(&r, "g", "differs from canonical"));
    }

    #[test]
    fn flags_kind_mismatch() {
        let r = check(
            "en",
            &cat(&[
                ("en", json!({ "n": "just a string" })),
                ("es", json!({ "n": { "$plural": { "other": "x" } } })),
            ]),
        )
        .unwrap();
        assert!(has_error(&r, "n", "kind mismatch"));
    }
}
