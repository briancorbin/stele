use super::{cat_char, recase_placeholders, Binding, Case, Emitter};
use crate::ir::{Ir, Kind, Message, MessageValue, ParamType};
use serde_json::{Map, Value};
use std::collections::BTreeMap;

pub struct TsEmitter {
    pub callable: bool,
    pub case: Case,
    pub binding: Binding,
}

fn ts_type(ty: &ParamType) -> &'static str {
    match ty {
        ParamType::Number => "number",
        // Placeholders interpolate any stringifiable value; accepting numbers too
        // avoids spurious `String(n)` wraps at call sites (a `count`-named param
        // is the one inferred as a strict `number`).
        ParamType::String => "string | number",
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

fn render_accessor(
    branch: &BTreeMap<String, Node>,
    indent: usize,
    callable: bool,
    case: Case,
) -> String {
    let pad = "  ".repeat(indent);
    let count = case.apply("count");
    branch
        .iter()
        .map(|(k, node)| {
            let k = case.apply(k);
            match node {
                Node::Leaf(m) => {
                    // JSON-escape the lookup key so any raw character in the key
                    // (quote, backslash) stays valid and matches the DATA key.
                    let key = serde_json::to_string(&m.dotted()).unwrap();
                    match m.kind {
                        Kind::Plain if m.params.is_empty() && callable => {
                            format!("{pad}{k}: () => D[{key}] as string,")
                        }
                        Kind::Plain if m.params.is_empty() => {
                            format!("{pad}{k}: D[{key}] as string,")
                        }
                        Kind::Plain => format!(
                            "{pad}{k}: (a: {{ {} }}) => interp(D[{key}] as string, a),",
                            args(m, case)
                        ),
                        Kind::Plural => format!(
                            "{pad}{k}: (a: {{ {} }}) => plural(locale, D[{key}] as Forms, a.{count}, a),",
                            args(m, case)
                        ),
                    }
                }
                Node::Branch(b) => format!(
                    "{pad}{k}: {{\n{}\n{pad}}},",
                    render_accessor(b, indent + 1, callable, case)
                ),
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn build_data(ir: &Ir, case: Case) -> Value {
    let mut top = Map::new();
    for loc in &ir.locales {
        let mut inner = Map::new();
        for m in &ir.messages {
            let v = match m.values.get(loc) {
                Some(MessageValue::Plain(s)) => Value::String(recase_placeholders(s, case)),
                Some(MessageValue::Plural(forms)) => {
                    let mut o = Map::new();
                    for (cat, t) in forms {
                        o.insert(cat.clone(), Value::String(recase_placeholders(t, case)));
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
            "\nconst DATA: Record<Locale, Record<string, string | Forms>> = {data};\n"
        ));
        out.push_str(&format!(
            "\nexport function {}(locale: Locale): {} {{\n",
            self.binding.factory(),
            self.binding.ty
        ));
        out.push_str("  const D = DATA[locale];\n");
        out.push_str(&format!(
            "  return {{\n{}\n  }};\n}}\n",
            render_accessor(&tree, 2, self.callable, self.case)
        ));
        out
    }
}
