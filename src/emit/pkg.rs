use super::{react, store, ts, Binding, Case};
use crate::ir::Ir;

/// Options for emitting a self-contained node package (compiled `.js`, `.d.ts`,
/// and a `package.json`) rather than loose `.ts` files — the "codegen as a
/// dependency" model (think Prisma Client). The package can be generated
/// straight into `node_modules/<name>/` by a postinstall step and imported by
/// name, so nothing lands in the repo's source tree.
pub struct PackageOptions {
    pub name: String,
    pub version: String,
    pub store: bool,
    pub react: bool,
    pub callable: bool,
    pub case: Case,
    pub binding: Binding,
}

/// Render every file of the package as `(filename, contents)` pairs. The store
/// is included whenever react is (react binds to it). Intra-package imports are
/// relative (`./index.js`, `./store.js`) so the package is portable wherever it
/// lands.
pub fn render(ir: &Ir, o: &PackageOptions) -> Vec<(String, String)> {
    let want_store = o.store || o.react;
    let mut files = vec![
        (
            "index.js".to_string(),
            ts::core_js(ir, o.callable, o.case, &o.binding),
        ),
        (
            "index.d.ts".to_string(),
            ts::core_dts(ir, o.callable, o.case, &o.binding),
        ),
    ];
    if want_store {
        files.push(("store.js".to_string(), store::store_js(ir, &o.binding)));
        files.push(("store.d.ts".to_string(), store::store_dts(&o.binding)));
    }
    if o.react {
        files.push(("react.js".to_string(), react::react_js(&o.binding)));
        files.push(("react.d.ts".to_string(), react::react_dts(&o.binding)));
    }
    files.push(("package.json".to_string(), package_json(o, want_store)));
    files
}

// A single `exports` subpath. `types` MUST come before `default`: Node/TS match
// conditions in declaration order, and `default` matches everything — so if it
// came first, the `types` condition would never be reached and TS would resolve
// the `.js` instead of the `.d.ts`.
fn subpath(key: &str, js: &str, dts: &str) -> String {
    format!("    {key}: {{ \"types\": \"{dts}\", \"default\": \"{js}\" }}")
}

fn package_json(o: &PackageOptions, want_store: bool) -> String {
    let mut exports = vec![subpath("\".\"", "./index.js", "./index.d.ts")];
    if want_store {
        exports.push(subpath("\"./store\"", "./store.js", "./store.d.ts"));
    }
    if o.react {
        exports.push(subpath("\"./react\"", "./react.js", "./react.d.ts"));
    }
    // No `"type": "module"` → CommonJS, so RN / Metro / Jest consume it from
    // node_modules with zero config. Hand-formatted (not serde) to keep field +
    // condition order deterministic without pulling `preserve_order` in globally
    // (which would disturb the sorted DATA output elsewhere). Values JSON-escaped.
    format!(
        "{{\n  \"name\": {},\n  \"version\": {},\n  \"main\": \"./index.js\",\n  \"types\": \"./index.d.ts\",\n  \"exports\": {{\n{}\n  }}\n}}\n",
        serde_json::to_string(&o.name).unwrap(),
        serde_json::to_string(&o.version).unwrap(),
        exports.join(",\n"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::PluralTable;
    use std::collections::BTreeMap;

    fn ir() -> Ir {
        // core_js packs the plural tables, so every locale needs one (the real
        // build_ir path guarantees this; a minimal "other"-only table suffices).
        let table = || PluralTable {
            categories: vec!["other".into()],
            small: vec!["other".into()],
            modulo: vec!["other".into()],
        };
        let mut plural_rules = BTreeMap::new();
        plural_rules.insert("en".to_string(), table());
        plural_rules.insert("es".to_string(), table());
        Ir {
            canonical: "en".into(),
            locales: vec!["en".into(), "es".into()],
            messages: vec![],
            plural_rules,
        }
    }

    fn opts(store: bool, react: bool) -> PackageOptions {
        PackageOptions {
            name: "@myapp/copy".into(),
            version: "1.2.3".into(),
            store,
            react,
            callable: false,
            case: Case::Camel,
            binding: Binding::new("stele"),
        }
    }

    fn names(files: &[(String, String)]) -> Vec<&str> {
        files.iter().map(|(n, _)| n.as_str()).collect()
    }

    #[test]
    fn full_package_has_all_files() {
        let files = render(&ir(), &opts(true, true));
        assert_eq!(
            names(&files),
            vec![
                "index.js",
                "index.d.ts",
                "store.js",
                "store.d.ts",
                "react.js",
                "react.d.ts",
                "package.json",
            ]
        );
    }

    #[test]
    fn react_implies_store() {
        // store=false but react=true must still emit the store (react binds to it)
        let files = render(&ir(), &opts(false, true));
        assert!(names(&files).contains(&"store.js"));
    }

    #[test]
    fn core_only_package() {
        let files = render(&ir(), &opts(false, false));
        assert_eq!(
            names(&files),
            vec!["index.js", "index.d.ts", "package.json"]
        );
    }

    #[test]
    fn package_json_orders_types_before_default() {
        let files = render(&ir(), &opts(true, true));
        let pj = &files.iter().find(|(n, _)| n == "package.json").unwrap().1;
        // `types` must precede `default` in each subpath, or TS resolves the .js
        let dot = pj.find("\"types\": \"./index.d.ts\"").unwrap();
        let def = pj.find("\"default\": \"./index.js\"").unwrap();
        assert!(dot < def);
        assert!(pj.contains("\"@myapp/copy\""));
        assert!(pj.contains("\"./store\""));
        assert!(pj.contains("\"./react\""));
    }

    #[test]
    fn core_js_is_runtime_only_no_types() {
        let files = render(&ir(), &opts(false, false));
        let js = &files.iter().find(|(n, _)| n == "index.js").unwrap().1;
        // CommonJS, not ESM (zero-config for RN/Metro/Jest)
        assert!(js.contains("function createStele(locale) {"));
        assert!(js.contains("module.exports = { createStele };"));
        assert!(!js.contains("export ")); // no ESM export keyword
        assert!(!js.contains(": Locale")); // no type annotations leaked into .js
        assert!(!js.contains("export interface"));
        let dts = &files.iter().find(|(n, _)| n == "index.d.ts").unwrap().1;
        assert!(dts.contains("export declare function createStele(locale: Locale): Stele;"));
        assert!(!dts.contains("const DATA")); // no runtime in .d.ts
    }
}
