use super::{cat_char, recase_placeholders, Binding, Case, Emitter};
use crate::ir::{Ir, Kind, Message, MessageValue, ParamType};
use serde_json::{Map, Value};
use std::collections::BTreeMap;

pub struct TsEmitter {
    pub callable: bool,
    pub case: Case,
    pub binding: Binding,
}

fn ts_type(ty: &ParamType) -> String {
    match ty {
        ParamType::Number => "number".to_string(),
        // Placeholders interpolate any stringifiable value; accepting numbers too
        // avoids spurious `String(n)` wraps at call sites (a `count`-named param
        // is the one inferred as a strict `number`).
        ParamType::String => "string | number".to_string(),
        // A `$select` selector becomes a literal union of its case names.
        ParamType::Enum(cases) => cases
            .iter()
            .map(|c| format!("\"{c}\""))
            .collect::<Vec<_>>()
            .join(" | "),
    }
}

fn args(m: &Message, case: Case) -> String {
    m.params
        .iter()
        .map(|p| format!("{}: {}", case.apply(&p.name), ts_type(&p.ty)))
        .collect::<Vec<_>>()
        .join("; ")
}

fn sig(m: &Message, callable: bool, case: Case) -> String {
    if m.params.is_empty() {
        if callable {
            "() => string".into()
        } else {
            "string".into()
        }
    } else {
        format!("(a: {{ {} }}) => string", args(m, case))
    }
}

enum Node<'a> {
    Leaf(&'a Message),
    Branch(BTreeMap<String, Node<'a>>),
}

fn insert<'a>(branch: &mut BTreeMap<String, Node<'a>>, path: &[String], m: &'a Message) {
    if path.len() == 1 {
        branch.insert(path[0].clone(), Node::Leaf(m));
        return;
    }
    let child = branch
        .entry(path[0].clone())
        .or_insert_with(|| Node::Branch(BTreeMap::new()));
    if let Node::Branch(b) = child {
        insert(b, &path[1..], m);
    }
}

fn render_type(
    branch: &BTreeMap<String, Node>,
    indent: usize,
    callable: bool,
    case: Case,
) -> String {
    let pad = "  ".repeat(indent);
    branch
        .iter()
        .map(|(k, node)| {
            let k = case.apply(k);
            match node {
                Node::Leaf(m) => format!("{pad}{k}: {};", sig(m, callable, case)),
                Node::Branch(b) => format!(
                    "{pad}{k}: {{\n{}\n{pad}}};",
                    render_type(b, indent + 1, callable, case)
                ),
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

// Renders the accessor object literal. With `typed` it's TypeScript (type-cast
// lookups + typed arrow params, for the `.ts` / single-file target); without, it's
// plain JavaScript (no `as` casts, bare `(a)` params, for the package `.js`).
fn render_accessor(
    branch: &BTreeMap<String, Node>,
    indent: usize,
    callable: bool,
    case: Case,
    typed: bool,
) -> String {
    let pad = "  ".repeat(indent);
    let count = case.apply("count");
    let cast_s = if typed { " as string" } else { "" };
    let cast_f = if typed { " as Forms" } else { "" };
    let cast_c = if typed { " as Cases" } else { "" };
    branch
        .iter()
        .map(|(k, node)| {
            let k = case.apply(k);
            match node {
                Node::Leaf(m) => {
                    // JSON-escape the lookup key so any raw character in the key
                    // (quote, backslash) stays valid and matches the DATA key.
                    let key = serde_json::to_string(&m.dotted()).unwrap();
                    let sig = if typed {
                        format!("(a: {{ {} }})", args(m, case))
                    } else {
                        "(a)".to_string()
                    };
                    match m.kind {
                        Kind::Plain if m.params.is_empty() && callable => {
                            format!("{pad}{k}: () => D[{key}]{cast_s},")
                        }
                        Kind::Plain if m.params.is_empty() => {
                            format!("{pad}{k}: D[{key}]{cast_s},")
                        }
                        Kind::Plain => {
                            format!("{pad}{k}: {sig} => interp(D[{key}]{cast_s}, a),")
                        }
                        Kind::Plural => format!(
                            "{pad}{k}: {sig} => plural(locale, D[{key}]{cast_f}, a.{count}, a),"
                        ),
                        Kind::Select => {
                            let sel = case.apply(m.selector.as_deref().unwrap_or("value"));
                            format!("{pad}{k}: {sig} => select(D[{key}]{cast_c}, a.{sel}, a),")
                        }
                    }
                }
                Node::Branch(b) => format!(
                    "{pad}{k}: {{\n{}\n{pad}}},",
                    render_accessor(b, indent + 1, callable, case, typed)
                ),
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn build_tree(ir: &Ir) -> BTreeMap<String, Node<'_>> {
    let mut tree = BTreeMap::new();
    for m in &ir.messages {
        insert(&mut tree, &m.path, m);
    }
    tree
}

fn locale_union(ir: &Ir) -> String {
    ir.locales
        .iter()
        .map(|l| format!("\"{l}\""))
        .collect::<Vec<_>>()
        .join(" | ")
}

/// The package `.d.ts` for the core: just the type surface (no runtime).
pub fn core_dts(ir: &Ir, callable: bool, case: Case, binding: &Binding) -> String {
    let tree = build_tree(ir);
    let mut out = String::new();
    out.push_str("// AUTO-GENERATED by stele — do not edit.\n");
    out.push_str("// Source of truth: locales/*.json\n\n");
    out.push_str(&format!("export type Locale = {};\n\n", locale_union(ir)));
    out.push_str(&format!(
        "export interface {} {{\n{}\n}}\n",
        binding.ty,
        render_type(&tree, 1, callable, case)
    ));
    out.push_str(&format!(
        "\nexport declare function {}(locale: Locale): {};\n",
        binding.factory(),
        binding.ty
    ));
    out
}

/// The package `.js` for the core: runtime only (data, plural table, factory).
pub fn core_js(ir: &Ir, callable: bool, case: Case, binding: &Binding) -> String {
    let tree = build_tree(ir);
    let data = serde_json::to_string_pretty(&build_data(ir, case)).unwrap();
    let mut out = String::new();
    out.push_str("// AUTO-GENERATED by stele — do not edit.\n");
    out.push_str("// Source of truth: locales/*.json\n\n");
    out.push_str(&format!(
        "const PCAT_SMALL = {{\n{}\n}};\n",
        pack(ir, false)
    ));
    out.push_str(&format!("const PCAT_MOD = {{\n{}\n}};\n", pack(ir, true)));
    out.push_str(RUNTIME_JS);
    out.push_str(&format!("\nconst DATA = {data};\n"));
    out.push_str(&format!(
        "\nexport function {}(locale) {{\n",
        binding.factory()
    ));
    out.push_str("  const D = DATA[locale];\n");
    out.push_str(&format!(
        "  return {{\n{}\n  }};\n}}\n",
        render_accessor(&tree, 2, callable, case, false)
    ));
    out
}

fn build_data(ir: &Ir, case: Case) -> Value {
    let mut top = Map::new();
    for loc in &ir.locales {
        let mut inner = Map::new();
        for m in &ir.messages {
            let v = match m.values.get(loc) {
                Some(MessageValue::Plain(s)) => Value::String(recase_placeholders(s, case)),
                Some(MessageValue::Branches(branches)) => {
                    let mut o = Map::new();
                    for (k, t) in branches {
                        o.insert(k.clone(), Value::String(recase_placeholders(t, case)));
                    }
                    Value::Object(o)
                }
                None => Value::Null,
            };
            inner.insert(m.dotted(), v);
        }
        top.insert(loc.clone(), Value::Object(inner));
    }
    Value::Object(top)
}

// Pack a per-locale category table (small or modulo) into a 100-char string.
fn pack(ir: &Ir, modulo: bool) -> String {
    ir.locales
        .iter()
        .map(|loc| {
            let table = &ir.plural_rules[loc];
            let cats = if modulo { &table.modulo } else { &table.small };
            let packed: String = cats.iter().map(|c| cat_char(c)).collect();
            format!(
                "  {}: {},",
                serde_json::to_string(loc).unwrap(),
                serde_json::to_string(&packed).unwrap()
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

const RUNTIME: &str = r#"
type Forms = Partial<Record<"zero" | "one" | "two" | "few" | "many" | "other", string>>;

// Baked from CLDR via ICU4X at generate time. Pure table lookup — never calls
// Intl.PluralRules (which Hermes / React Native does not implement).
const PCODE: Record<string, keyof Forms> = {
  z: "zero", "1": "one", "2": "two", f: "few", m: "many", o: "other",
};

function pluralCategory(locale: Locale, n: number): keyof Forms {
  const i = Math.abs(Math.trunc(n));
  const table = i < 100 ? PCAT_SMALL[locale] : PCAT_MOD[locale];
  return PCODE[table[i < 100 ? i : i % 100]];
}

function interp(t: string, a: Record<string, string | number>): string {
  return t.replace(/\{\{(\w+)\}\}/g, (_, k) => String(a[k]));
}

function plural(locale: Locale, forms: Forms, n: number, a: Record<string, string | number>): string {
  return interp(forms[pluralCategory(locale, n)] ?? forms.other ?? "", a);
}

type Cases = Partial<Record<string, string>>;

function select(cases: Cases, value: string, a: Record<string, string | number>): string {
  return interp(cases[value] ?? cases.other ?? "", a);
}
"#;

// The plural runtime as plain JavaScript (no type annotations) for the package
// `.js`. Behaviour is identical to RUNTIME above — kept in lockstep by hand.
const RUNTIME_JS: &str = r#"
const PCODE = {
  z: "zero", "1": "one", "2": "two", f: "few", m: "many", o: "other",
};

function pluralCategory(locale, n) {
  const i = Math.abs(Math.trunc(n));
  const table = i < 100 ? PCAT_SMALL[locale] : PCAT_MOD[locale];
  return PCODE[table[i < 100 ? i : i % 100]];
}

function interp(t, a) {
  return t.replace(/\{\{(\w+)\}\}/g, (_, k) => String(a[k]));
}

function plural(locale, forms, n, a) {
  return interp(forms[pluralCategory(locale, n)] ?? forms.other ?? "", a);
}

function select(cases, value, a) {
  return interp(cases[value] ?? cases.other ?? "", a);
}
"#;

impl Emitter for TsEmitter {
    fn emit(&self, ir: &Ir) -> String {
        let mut tree = BTreeMap::new();
        for m in &ir.messages {
            insert(&mut tree, &m.path, m);
        }

        let locale_union = ir
            .locales
            .iter()
            .map(|l| format!("\"{l}\""))
            .collect::<Vec<_>>()
            .join(" | ");
        let data = serde_json::to_string_pretty(&build_data(ir, self.case)).unwrap();

        let mut out = String::new();
        out.push_str("// AUTO-GENERATED by stele — do not edit.\n");
        out.push_str("// Source of truth: locales/*.json\n\n");
        out.push_str(&format!("export type Locale = {locale_union};\n\n"));
        out.push_str(&format!(
            "export interface {} {{\n{}\n}}\n",
            self.binding.ty,
            render_type(&tree, 1, self.callable, self.case)
        ));
        out.push_str(&format!(
            "\nconst PCAT_SMALL: Record<Locale, string> = {{\n{}\n}};\n",
            pack(ir, false)
        ));
        out.push_str(&format!(
            "const PCAT_MOD: Record<Locale, string> = {{\n{}\n}};\n",
            pack(ir, true)
        ));
        out.push_str(RUNTIME);
        out.push_str(&format!(
            "\nconst DATA: Record<Locale, Record<string, string | Forms | Cases>> = {data};\n"
        ));
        out.push_str(&format!(
            "\nexport function {}(locale: Locale): {} {{\n",
            self.binding.factory(),
            self.binding.ty
        ));
        out.push_str("  const D = DATA[locale];\n");
        out.push_str(&format!(
            "  return {{\n{}\n  }};\n}}\n",
            render_accessor(&tree, 2, self.callable, self.case, true)
        ));
        out
    }
}
