use super::{cat_char, recase_placeholders, Binding, Case, Emitter};
use crate::ir::{Ir, Kind, Message, MessageValue, ParamType};
use anyhow::{bail, Result};
use std::collections::{BTreeMap, HashMap};

pub struct SwiftEmitter {
    pub case: Case,
    pub binding: Binding,
}

fn swift_type(ty: &ParamType) -> &'static str {
    match ty {
        ParamType::Number => "Int",
        ParamType::String => "String",
    }
}

/// A valid Swift string literal. Unlike JSON, Swift uses `\u{XX}` (not `\uXXXX`)
/// and has no `\b`/`\f`, so control characters must be escaped specially.
fn swift_lit(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\0' => out.push_str("\\0"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{{{:x}}}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

const SWIFT_KEYWORDS: &[&str] = &[
    "associatedtype",
    "class",
    "deinit",
    "enum",
    "extension",
    "fileprivate",
    "func",
    "import",
    "init",
    "inout",
    "internal",
    "let",
    "open",
    "operator",
    "private",
    "precedencegroup",
    "protocol",
    "public",
    "rethrows",
    "static",
    "struct",
    "subscript",
    "typealias",
    "var",
    "break",
    "case",
    "continue",
    "default",
    "defer",
    "do",
    "else",
    "fallthrough",
    "for",
    "guard",
    "if",
    "in",
    "repeat",
    "return",
    "switch",
    "where",
    "while",
    "as",
    "catch",
    "false",
    "is",
    "nil",
    "super",
    "self",
    "Self",
    "throw",
    "throws",
    "true",
    "try",
    "_",
    "actor",
    "async",
    "await",
    "some",
    "any",
];

/// Backtick-escape an identifier if it's a Swift keyword (`repeat` -> `` `repeat` ``).
fn swift_ident(name: &str) -> String {
    if SWIFT_KEYWORDS.contains(&name) {
        format!("`{name}`")
    } else {
        name.to_string()
    }
}

/// Turn a locale tag into a valid Swift enum case name (`pt-BR` -> `pt_BR`). The
/// rawValue keeps the real tag, so runtime dictionary lookups still match.
fn locale_case(tag: &str) -> String {
    let mut s: String = tag
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if s.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        s.insert(0, '_');
    }
    if s.is_empty() {
        s.push('_');
    }
    swift_ident(&s)
}

fn sig(m: &Message, case: Case) -> String {
    m.params
        .iter()
        .map(|p| {
            format!(
                "{}: {}",
                swift_ident(&case.apply(&p.name)),
                swift_type(&p.ty)
            )
        })
        .collect::<Vec<_>>()
        .join(", ")
}

// Build the `[String: String]` arg dict passed to interp/plural. Keys match the
// re-cased placeholders in the baked string; Int params are stringified.
fn arg_dict(m: &Message, case: Case) -> String {
    if m.params.is_empty() {
        return "[:]".into();
    }
    let entries = m
        .params
        .iter()
        .map(|p| {
            let name = case.apply(&p.name);
            let ident = swift_ident(&name);
            let val = match p.ty {
                ParamType::Number => format!("String({ident})"),
                ParamType::String => ident,
            };
            format!("{}: {val}", swift_lit(&name))
        })
        .collect::<Vec<_>>()
        .join(", ");
    format!("[{entries}]")
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

fn render_leaf(k: &str, m: &Message, case: Case) -> String {
    let key = swift_lit(&m.dotted());
    match m.kind {
        Kind::Plain if m.params.is_empty() => {
            format!("public var {k}: String {{ SteleData.s(locale, {key}) }}")
        }
        Kind::Plain => format!(
            "public func {k}({}) -> String {{\n    SteleData.interp(SteleData.s(locale, {key}), {})\n}}",
            sig(m, case),
            arg_dict(m, case)
        ),
        Kind::Plural => format!(
            "public func {k}({}) -> String {{\n    SteleData.plural(locale, {key}, {}, {})\n}}",
            sig(m, case),
            swift_ident(&case.apply("count")),
            arg_dict(m, case)
        ),
    }
}

fn render_struct(name: &str, branch: &BTreeMap<String, Node>, case: Case) -> String {
    let mut members = Vec::new();
    for (k, node) in branch {
        match node {
            Node::Leaf(m) => members.push(render_leaf(&swift_ident(&case.apply(k)), m, case)),
            Node::Branch(b) => {
                let prop = swift_ident(&case.apply(k));
                let ty = Case::Pascal.apply(k); // Swift types stay PascalCase
                                                // `Self.` qualifies the type so a same-named property (pascal case) can't shadow it
                members.push(format!(
                    "public var {prop}: Self.{ty} {{ Self.{ty}(locale) }}"
                ));
                members.push(render_struct(&ty, b, case));
            }
        }
    }
    let body = indent(&members.join("\n"), 4);
    format!(
        "public struct {name} {{\n    let locale: Locale\n    public init(_ locale: Locale) {{ self.locale = locale }}\n{body}\n}}"
    )
}

fn dict_block(label: &str, ir: &Ir, plural: bool, case: Case) -> String {
    let mut blocks = Vec::new();
    for loc in &ir.locales {
        let entries: Vec<String> = ir
            .messages
            .iter()
            .filter_map(|m| match (plural, m.values.get(loc)) {
                (false, Some(MessageValue::Plain(v))) => Some(format!(
                    "            {}: {},",
                    swift_lit(&m.dotted()),
                    swift_lit(&recase_placeholders(v, case))
                )),
                (true, Some(MessageValue::Plural(forms))) => {
                    let inner = forms
                        .iter()
                        .map(|(c, t)| {
                            format!(
                                "{}: {}",
                                swift_lit(c),
                                swift_lit(&recase_placeholders(t, case))
                            )
                        })
                        .collect::<Vec<_>>()
                        .join(", ");
                    Some(format!(
                        "            {}: [{inner}],",
                        swift_lit(&m.dotted())
                    ))
                }
                _ => None,
            })
            .collect();
        if entries.is_empty() {
            blocks.push(format!("        {}: [:],", swift_lit(loc)));
        } else {
            blocks.push(format!(
                "        {}: [\n{}\n        ],",
                swift_lit(loc),
                entries.join("\n")
            ));
        }
    }
    format!("    static let {label} = [\n{}\n    ]", blocks.join("\n"))
}

// Packed per-locale plural-category tables, baked from CLDR via ICU4X.
fn pcat_block(label: &str, ir: &Ir, modulo: bool) -> String {
    let entries = ir
        .locales
        .iter()
        .map(|loc| {
            let table = &ir.plural_rules[loc];
            let cats = if modulo { &table.modulo } else { &table.small };
            let packed: String = cats.iter().map(|c| cat_char(c)).collect();
            format!("        {}: {},", swift_lit(loc), swift_lit(&packed))
        })
        .collect::<Vec<_>>()
        .join("\n");
    format!("    static let {label}: [String: String] = [\n{entries}\n    ]")
}

const HELPERS: &str = r#"
    static func pcode(_ c: Character) -> String {
        switch c {
        case "z": return "zero"
        case "1": return "one"
        case "2": return "two"
        case "f": return "few"
        case "m": return "many"
        default: return "other"
        }
    }
    static func pluralCategory(_ l: Locale, _ n: Int) -> String {
        let i = n.magnitude // UInt — no abs() overflow on Int.min
        let table = i < 100 ? (PCAT_SMALL[l.rawValue] ?? "") : (PCAT_MOD[l.rawValue] ?? "")
        let chars = Array(table)
        let idx = Int(i < 100 ? i : i % 100)
        return idx < chars.count ? pcode(chars[idx]) : "other"
    }
    static func s(_ l: Locale, _ k: String) -> String {
        STRINGS[l.rawValue]?[k] ?? k
    }
    static func interp(_ t: String, _ a: [String: String]) -> String {
        var out = t
        for (k, v) in a { out = out.replacingOccurrences(of: "{{\(k)}}", with: v) }
        return out
    }
    static func plural(_ l: Locale, _ k: String, _ n: Int, _ a: [String: String]) -> String {
        let forms = PLURALS[l.rawValue]?[k]
        let cat = pluralCategory(l, n)
        let t = forms?[cat] ?? forms?["other"] ?? ""
        return interp(t, a)
    }"#;

// Type names Stele emits or Swift reserves — a namespace must not collide with
// these. The root copy type (`Stele` by default, configurable) is checked
// separately against the active binding.
const RESERVED_TYPES: &[&str] = &[
    "Locale",
    "SteleData",
    "String",
    "Int",
    "UInt",
    "Double",
    "Float",
    "Bool",
    "Character",
    "Any",
    "AnyObject",
    "Array",
    "Dictionary",
    "Optional",
    "Self",
    "Type",
    "Protocol",
    "Foundation",
];

impl Emitter for SwiftEmitter {
    fn validate(&self, ir: &Ir) -> Result<()> {
        // Nested namespaces become PascalCase struct types; ensure those names
        // don't collide with each other (e.g. `x` + `X`) or shadow a built-in.
        let mut groups: HashMap<Vec<String>, HashMap<String, String>> = HashMap::new();
        for m in &ir.messages {
            for i in 0..m.path.len().saturating_sub(1) {
                let parent = m.path[..i].to_vec();
                let seg = &m.path[i];
                let ty = Case::Pascal.apply(seg);
                if ty == self.binding.ty || RESERVED_TYPES.contains(&ty.as_str()) {
                    bail!("namespace '{seg}' becomes Swift type '{ty}', which collides with a built-in or emitted type — rename it");
                }
                let group = groups.entry(parent).or_default();
                match group.get(&ty) {
                    Some(orig) if orig != seg => {
                        bail!("namespaces '{orig}' and '{seg}' both become Swift type '{ty}'")
                    }
                    _ => {
                        group.insert(ty, seg.clone());
                    }
                }
            }
        }
        Ok(())
    }

    fn emit(&self, ir: &Ir) -> String {
        let mut tree = BTreeMap::new();
        for m in &ir.messages {
            insert(&mut tree, &m.path, m);
        }

        let cases = ir
            .locales
            .iter()
            .map(|l| format!("    case {} = {}", locale_case(l), swift_lit(l)))
            .collect::<Vec<_>>()
            .join("\n");

        let strings = dict_block("STRINGS: [String: [String: String]]", ir, false, self.case);
        let plurals = dict_block(
            "PLURALS: [String: [String: [String: String]]]",
            ir,
            true,
            self.case,
        );
        let pcat_small = pcat_block("PCAT_SMALL", ir, false);
        let pcat_mod = pcat_block("PCAT_MOD", ir, true);

        let mut out = String::new();
        out.push_str("// AUTO-GENERATED by stele — do not edit.\n");
        out.push_str("// Source of truth: locales/*.json\n");
        out.push_str("import Foundation\n\n");
        out.push_str(&format!("public enum Locale: String {{\n{cases}\n}}\n\n"));
        out.push_str(&render_struct(&self.binding.ty, &tree, self.case));
        out.push_str("\n\nenum SteleData {\n");
        out.push_str(&strings);
        out.push('\n');
        out.push_str(&plurals);
        out.push('\n');
        out.push_str(&pcat_small);
        out.push('\n');
        out.push_str(&pcat_mod);
        out.push_str(HELPERS);
        out.push_str("\n}\n");
        out
    }
}
