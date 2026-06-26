use super::{cat_char, Emitter};
use crate::ir::{Ir, Kind, Message, MessageValue, ParamType};
use serde_json::{Map, Value};
use std::collections::BTreeMap;

pub struct TsEmitter;

fn ts_type(ty: &ParamType) -> &'static str {
    match ty {
        ParamType::Number => "number",
        ParamType::String => "string",
    }
}

fn args(m: &Message) -> String {
    m.params
        .iter()
        .map(|p| format!("{}: {}", p.name, ts_type(&p.ty)))
        .collect::<Vec<_>>()
        .join("; ")
}

fn sig(m: &Message) -> String {
    if m.params.is_empty() {
        "string".into()
    } else {
        format!("(a: {{ {} }}) => string", args(m))
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

fn render_type(branch: &BTreeMap<String, Node>, indent: usize) -> String {
    let pad = "  ".repeat(indent);
    branch
        .iter()
        .map(|(k, node)| match node {
            Node::Leaf(m) => format!("{pad}{k}: {};", sig(m)),
            Node::Branch(b) => {
                format!("{pad}{k}: {{\n{}\n{pad}}};", render_type(b, indent + 1))
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn render_accessor(branch: &BTreeMap<String, Node>, indent: usize) -> String {
    let pad = "  ".repeat(indent);
    branch
        .iter()
        .map(|(k, node)| match node {
            Node::Leaf(m) => {
                let dotted = m.dotted();
                match m.kind {
                    Kind::Plain if m.params.is_empty() => {
                        format!("{pad}{k}: D[\"{dotted}\"] as string,")
                    }
                    Kind::Plain => format!(
                        "{pad}{k}: (a: {{ {} }}) => interp(D[\"{dotted}\"] as string, a),",
                        args(m)
                    ),
                    Kind::Plural => format!(
                        "{pad}{k}: (a: {{ {} }}) => plural(locale, D[\"{dotted}\"] as Forms, a.count, a),",
                        args(m)
                    ),
                }
            }
            Node::Branch(b) => {
                format!("{pad}{k}: {{\n{}\n{pad}}},", render_accessor(b, indent + 1))
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn build_data(ir: &Ir) -> Value {
    let mut top = Map::new();
    for loc in &ir.locales {
        let mut inner = Map::new();
        for m in &ir.messages {
            let v = match m.values.get(loc) {
                Some(MessageValue::Plain(s)) => Value::String(s.clone()),
                Some(MessageValue::Plural(forms)) => {
                    let mut o = Map::new();
                    for (cat, t) in forms {
                        o.insert(cat.clone(), Value::String(t.clone()));
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
            format!("  {loc}: {},", serde_json::to_string(&packed).unwrap())
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
  return t.replace(/\{(\w+)\}/g, (_, k) => String(a[k]));
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
        let data = serde_json::to_string_pretty(&build_data(ir)).unwrap();

        let mut out = String::new();
        out.push_str("// AUTO-GENERATED by stele — do not edit.\n");
        out.push_str("// Source of truth: locales/*.json\n\n");
        out.push_str(&format!("export type Locale = {locale_union};\n\n"));
        out.push_str(&format!(
            "export interface Copy {{\n{}\n}}\n",
            render_type(&tree, 1)
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
        out.push_str("\nexport function createCopy(locale: Locale): Copy {\n");
        out.push_str("  const D = DATA[locale];\n");
        out.push_str(&format!(
            "  return {{\n{}\n  }};\n}}\n",
            render_accessor(&tree, 2)
        ));
        out
    }
}
