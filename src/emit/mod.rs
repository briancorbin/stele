use crate::ir::Ir;
use anyhow::{bail, Result};
use heck::{ToLowerCamelCase, ToSnakeCase, ToUpperCamelCase};
use regex::Regex;
use std::collections::HashMap;
use std::sync::LazyLock;

pub mod react;
pub mod store;
pub mod swift;
pub mod ts;

/// Every language backend implements this. Adding a language is one impl + one
/// line in `emitter_for` — the core IR never changes. This is the seam where
/// contributors plug in new targets.
pub trait Emitter {
    fn emit(&self, ir: &Ir) -> String;

    /// Target-specific validation run before `emit` (e.g. Swift type-name
    /// collisions). Default: nothing extra beyond the shared `validate_idents`.
    fn validate(&self, _ir: &Ir) -> Result<()> {
        Ok(())
    }
}

/// Keys that are valid identifiers but collide with JS object/prototype members.
/// Emitting an accessor or DATA entry named any of these silently corrupts the
/// object (prototype setter, shadowed builtin) — so reject them outright.
const RESERVED_KEYS: &[&str] = &[
    "__proto__",
    "prototype",
    "constructor",
    "toString",
    "toLocaleString",
    "valueOf",
    "hasOwnProperty",
    "isPrototypeOf",
    "propertyIsEnumerable",
    "__defineGetter__",
    "__defineSetter__",
    "__lookupGetter__",
    "__lookupSetter__",
];

/// Output identifier casing for generated accessors. Input keys may be authored
/// in any case (camel / snake / kebab); the chosen output case is applied
/// uniformly to namespace names, leaf names, and parameter names.
#[derive(Clone, Copy, PartialEq)]
pub enum Case {
    Camel,
    Snake,
    Pascal,
    Preserve,
}

impl Case {
    pub fn parse(s: &str) -> Result<Case> {
        Ok(match s {
            "camel" => Case::Camel,
            "snake" => Case::Snake,
            "pascal" => Case::Pascal,
            "preserve" => Case::Preserve,
            other => bail!("unknown case '{other}' (use: camel | snake | pascal | preserve)"),
        })
    }

    pub fn apply(self, s: &str) -> String {
        match self {
            Case::Camel => s.to_lower_camel_case(),
            Case::Snake => s.to_snake_case(),
            Case::Pascal => s.to_upper_camel_case(),
            Case::Preserve => s.to_string(),
        }
    }
}

/// The brand name the generated API is built around. `stele` (the default)
/// yields the type `Stele`, the factory `createStele`, and the React bindings
/// `useStele` / `SteleProvider`. Set `binding = "copy"` for the classic
/// `Copy` / `createCopy` / `useCopy` names. One word in `stele.toml` renames the
/// whole emitted surface, uniformly across every target.
#[derive(Clone)]
pub struct Binding {
    /// PascalCase type name (e.g. `Stele`).
    pub ty: String,
}

impl Binding {
    pub fn new(raw: &str) -> Binding {
        Binding {
            ty: raw.to_upper_camel_case(),
        }
    }
    /// Factory function name, e.g. `createStele`.
    pub fn factory(&self) -> String {
        format!("create{}", self.ty)
    }
    /// React hook name, e.g. `useStele`.
    pub fn hook(&self) -> String {
        format!("use{}", self.ty)
    }
    /// Store accessor getter name, e.g. `getStele`.
    pub fn getter(&self) -> String {
        format!("get{}", self.ty)
    }
}

/// Per-target emitter options, threaded from `stele.toml`.
#[derive(Clone)]
pub struct EmitOptions {
    pub callable: bool,
    /// Import specifier to the core module (used by the `store` target).
    pub core: String,
    /// Import specifier to the store module (used by the `react` target).
    pub store: String,
    /// Output identifier case.
    pub case: Case,
    /// The brand name the generated API is built around.
    pub binding: Binding,
}

pub fn emitter_for(lang: &str, opts: &EmitOptions) -> Option<Box<dyn Emitter>> {
    match lang {
        "typescript" | "ts" => Some(Box::new(ts::TsEmitter {
            callable: opts.callable,
            case: opts.case,
            binding: opts.binding.clone(),
        })),
        "swift" => Some(Box::new(swift::SwiftEmitter {
            case: opts.case,
            binding: opts.binding.clone(),
        })),
        "store" => Some(Box::new(store::StoreEmitter {
            core: opts.core.clone(),
            binding: opts.binding.clone(),
        })),
        "react" => Some(Box::new(react::ReactEmitter {
            store: opts.store.clone(),
            binding: opts.binding.clone(),
        })),
        _ => None,
    }
}

static PLACEHOLDER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\{\{(\w+)\}\}").unwrap());

/// Rewrite `{{param}}` placeholders in a baked string to the chosen output case,
/// so they stay in sync with the re-cased function parameter names.
pub fn recase_placeholders(s: &str, case: Case) -> String {
    if case == Case::Preserve {
        return s.to_string();
    }
    PLACEHOLDER
        .replace_all(s, |c: &regex::Captures| {
            format!("{{{{{}}}}}", case.apply(&c[1]))
        })
        .into_owned()
}

/// Validate that, under the chosen output case, every key and parameter becomes a
/// valid, non-colliding identifier — so generation fails loudly instead of
/// emitting code that won't compile or that silently clobbers a duplicate key.
pub fn validate_idents(ir: &Ir, case: Case) -> Result<()> {
    let mut siblings: HashMap<Vec<String>, HashMap<String, String>> = HashMap::new();
    for m in &ir.messages {
        for i in 0..m.path.len() {
            let parent = m.path[..i].to_vec();
            let seg = &m.path[i];
            let cased = case.apply(seg);
            check_ident(seg, &cased)?;
            let group = siblings.entry(parent).or_default();
            match group.get(&cased) {
                Some(orig) if orig != seg => bail!(
                    "keys '{orig}' and '{seg}' both become '{cased}' under the chosen output case"
                ),
                _ => {
                    group.insert(cased, seg.clone());
                }
            }
        }
        let mut params: HashMap<String, String> = HashMap::new();
        for p in &m.params {
            let cased = case.apply(&p.name);
            check_ident(&p.name, &cased)?;
            match params.get(&cased) {
                Some(orig) if orig != &p.name => {
                    bail!("params '{orig}' and '{}' both become '{cased}'", p.name)
                }
                _ => {
                    params.insert(cased, p.name.clone());
                }
            }
        }
    }
    Ok(())
}

fn check_ident(orig: &str, cased: &str) -> Result<()> {
    let valid = cased
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && cased.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    if !valid {
        bail!("key '{orig}' becomes '{cased}', which isn't a valid identifier");
    }
    if RESERVED_KEYS.contains(&cased) {
        bail!("key '{orig}' becomes '{cased}', which collides with a built-in object member — rename it");
    }
    Ok(())
}

/// Single-character encoding for a plural category, used to pack the baked
/// per-locale tables compactly into generated code.
pub fn cat_char(name: &str) -> char {
    match name {
        "zero" => 'z',
        "one" => '1',
        "two" => '2',
        "few" => 'f',
        "many" => 'm',
        _ => 'o',
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{Kind, Message};
    use std::collections::BTreeMap;

    fn leaf(path: &[&str]) -> Message {
        Message {
            path: path.iter().map(|s| s.to_string()).collect(),
            params: vec![],
            kind: Kind::Plain,
            values: BTreeMap::new(),
        }
    }

    fn ir(messages: Vec<Message>) -> Ir {
        Ir {
            canonical: "en".into(),
            locales: vec!["en".into()],
            messages,
            plural_rules: BTreeMap::new(),
        }
    }

    #[test]
    fn case_apply_normalizes_any_input() {
        assert_eq!(Case::Camel.apply("walker_today"), "walkerToday");
        assert_eq!(Case::Camel.apply("greeting-text"), "greetingText");
        assert_eq!(Case::Snake.apply("walkerToday"), "walker_today");
        assert_eq!(Case::Pascal.apply("dog_count"), "DogCount");
        assert_eq!(Case::Preserve.apply("dog_count"), "dog_count");
    }

    #[test]
    fn recase_placeholders_track_the_output_case() {
        assert_eq!(
            recase_placeholders("Hi {{first_name}}", Case::Camel),
            "Hi {{firstName}}"
        );
        assert_eq!(
            recase_placeholders("Hi {{firstName}}", Case::Snake),
            "Hi {{first_name}}"
        );
    }

    #[test]
    fn validate_flags_collisions_and_passes_clean() {
        let collide = ir(vec![leaf(&["dog_count"]), leaf(&["dogCount"])]);
        assert!(validate_idents(&collide, Case::Camel).is_err());

        let clean = ir(vec![leaf(&["dog_count"]), leaf(&["walker_today"])]);
        assert!(validate_idents(&clean, Case::Camel).is_ok());
    }

    #[test]
    fn validate_rejects_invalid_identifiers() {
        let bad = ir(vec![leaf(&["2fa"])]);
        assert!(validate_idents(&bad, Case::Preserve).is_err());
    }

    #[test]
    fn validate_rejects_reserved_object_keys() {
        assert!(validate_idents(&ir(vec![leaf(&["toString"])]), Case::Camel).is_err());
        assert!(validate_idents(&ir(vec![leaf(&["__proto__"])]), Case::Preserve).is_err());
        assert!(validate_idents(&ir(vec![leaf(&["constructor"])]), Case::Camel).is_err());
    }
}
