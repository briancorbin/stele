use super::Emitter;
use crate::ir::{Ir, Kind, Message, MessageValue, ParamType};
use std::collections::BTreeMap;

pub struct SwiftEmitter;

fn swift_type(ty: &ParamType) -> &'static str {
    match ty {
        ParamType::Number => "Int",
        ParamType::String => "String",
    }
}

fn swift_lit(s: &str) -> String {
    serde_json::to_string(s).unwrap()
}

fn sig(m: &Message) -> String {
    m.params
        .iter()
        .map(|p| format!("{}: {}", p.name, swift_type(&p.ty)))
        .collect::<Vec<_>>()
        .join(", ")
}

// Build the `[String: String]` arg dict passed to interp/plural. Int params are
// stringified at the call site so the runtime stays format-agnostic.
fn arg_dict(m: &Message) -> String {
    if m.params.is_empty() {
        return "[:]".into();
    }
    let entries = m
        .params
        .iter()
        .map(|p| {
            let val = match p.ty {
                ParamType::Number => format!("String({})", p.name),
                ParamType::String => p.name.clone(),
            };
            format!("\"{}\": {}", p.name, val)
        })
        .collect::<Vec<_>>()
        .join(", ");
    format!("[{entries}]")
}

fn pascal(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

fn indent(s: &str, n: usize) -> String {
    let pad = " ".repeat(n);
    s.lines()
        .map(|l| {
            if l.is_empty() {
                String::new()
            } else {
                format!("{pad}{l}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
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

fn render_leaf(k: &str, m: &Message) -> String {
    let d = m.dotted();
    match m.kind {
        Kind::Plain if m.params.is_empty() => {
            format!("public var {k}: String {{ SteleData.s(locale, \"{d}\") }}")
        }
        Kind::Plain => format!(
            "public func {k}({}) -> String {{\n    SteleData.interp(SteleData.s(locale, \"{d}\"), {})\n}}",
            sig(m),
            arg_dict(m)
        ),
        Kind::Plural => format!(
            "public func {k}({}) -> String {{\n    SteleData.plural(locale, \"{d}\", count, {})\n}}",
            sig(m),
            arg_dict(m)
        ),
    }
}

fn render_struct(name: &str, branch: &BTreeMap<String, Node>) -> String {
    let mut members = Vec::new();
    for (k, node) in branch {
        match node {
            Node::Leaf(m) => members.push(render_leaf(k, m)),
            Node::Branch(b) => {
                let ty = pascal(k);
                members.push(format!("public var {k}: {ty} {{ {ty}(locale) }}"));
                members.push(render_struct(&ty, b));
            }
        }
    }
    let body = indent(&members.join("\n"), 4);
    format!(
        "public struct {name} {{\n    let locale: Locale\n    public init(_ locale: Locale) {{ self.locale = locale }}\n{body}\n}}"
    )
}

const HELPERS: &str = r#"
    static func s(_ l: Locale, _ k: String) -> String {
        STRINGS[l.rawValue]?[k] ?? k
    }
    static func interp(_ t: String, _ a: [String: String]) -> String {
        var out = t
        for (k, v) in a { out = out.replacingOccurrences(of: "{\(k)}", with: v) }
        return out
    }
    static func plural(_ l: Locale, _ k: String, _ n: Int, _ a: [String: String]) -> String {
        let forms = PLURALS[l.rawValue]?[k]
        let cat = pluralCategory(l, n)
        let t = forms?[cat] ?? forms?["other"] ?? ""
        return interp(t, a)
    }"#;

fn dict_block(label: &str, ir: &Ir, plural: bool) -> String {
    let mut blocks = Vec::new();
    for loc in &ir.locales {
        let entries: Vec<String> = ir
            .messages
            .iter()
            .filter_map(|m| match (plural, m.values.get(loc)) {
                (false, Some(MessageValue::Plain(v))) => {
                    Some(format!("            \"{}\": {},", m.dotted(), swift_lit(v)))
                }
                (true, Some(MessageValue::Plural(forms))) => {
                    let inner = forms
                        .iter()
                        .map(|(c, t)| format!("\"{}\": {}", c, swift_lit(t)))
                        .collect::<Vec<_>>()
                        .join(", ");
                    Some(format!("            \"{}\": [{inner}],", m.dotted()))
                }
                _ => None,
            })
            .collect();
        if entries.is_empty() {
            blocks.push(format!("        \"{loc}\": [:],"));
        } else {
            blocks.push(format!("        \"{loc}\": [\n{}\n        ],", entries.join("\n")));
        }
    }
    format!("    static let {label} = [\n{}\n    ]", blocks.join("\n"))
}

impl Emitter for SwiftEmitter {
    fn emit(&self, ir: &Ir) -> String {
        let mut tree = BTreeMap::new();
        for m in &ir.messages {
            insert(&mut tree, &m.path, m);
        }

        let cases = ir
            .locales
            .iter()
            .map(|l| format!("    case {l}"))
            .collect::<Vec<_>>()
            .join("\n");
        let plural_cases = ir
            .locales
            .iter()
            .map(|l| format!("        case .{l}: return n == 1 ? \"one\" : \"other\""))
            .collect::<Vec<_>>()
            .join("\n");

        let strings = dict_block(
            "STRINGS: [String: [String: String]]",
            ir,
            false,
        );
        let plurals = dict_block(
            "PLURALS: [String: [String: [String: String]]]",
            ir,
            true,
        );

        let mut out = String::new();
        out.push_str("// AUTO-GENERATED by stele — do not edit.\n");
        out.push_str("// Source of truth: locales/*.json\n");
        out.push_str("import Foundation\n\n");
        out.push_str(&format!("public enum Locale: String {{\n{cases}\n}}\n\n"));
        out.push_str(&render_struct("Copy", &tree));
        out.push_str("\n\nenum SteleData {\n");
        out.push_str(&strings);
        out.push('\n');
        out.push_str(&plurals);
        out.push('\n');
        out.push_str(&format!(
            "    static func pluralCategory(_ l: Locale, _ n: Int) -> String {{\n        switch l {{\n{plural_cases}\n        }}\n    }}"
        ));
        out.push_str(HELPERS);
        out.push_str("\n}\n");
        out
    }
}
