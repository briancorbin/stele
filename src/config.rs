use serde::Deserialize;
use std::path::PathBuf;

#[derive(Deserialize)]
pub struct Config {
    pub canonical: String,
    pub locales: PathBuf,
    #[serde(default)]
    pub target: Vec<Target>,
    /// Optional `[package]` block: emit a self-contained node package (compiled
    /// `.js` + `.d.ts` + `package.json`) instead of (or alongside) loose `.ts`
    /// targets. Point `out` at `node_modules/<name>/` for a zero-repo-footprint
    /// "codegen as a dependency" setup.
    pub package: Option<Package>,
}

#[derive(Deserialize)]
pub struct Package {
    /// The package name written into `package.json` (e.g. `@myapp/copy`).
    pub name: String,
    /// Output directory for the package (e.g. `node_modules/@myapp/copy`).
    pub out: PathBuf,
    /// Version written into `package.json`. Defaults to `0.0.0`.
    pub version: Option<String>,
    /// Include the locale store (`store.js` / `store.d.ts`). Implied by `react`.
    #[serde(default)]
    pub store: bool,
    /// Include the React hooks (`react.js` / `react.d.ts`).
    #[serde(default)]
    pub react: bool,
    /// Emit no-arg leaves as `() => "..."` thunks (same as the `callable` target option).
    #[serde(default)]
    pub callable: bool,
    /// Output identifier case (same as the target `case` option).
    pub case: Option<String>,
    /// Brand name for the API (same as the target `binding` option).
    pub binding: Option<String>,
}

#[derive(Deserialize)]
pub struct Target {
    pub lang: String,
    pub out: PathBuf,
    /// When true, no-argument leaves are emitted as `() => "..."` thunks rather
    /// than bare string constants. Matches codebases where every copy leaf is
    /// callable (e.g. `copy.home.title()`). TypeScript only.
    #[serde(default)]
    pub callable: bool,
    /// For the `store` target: the import specifier to the core generated module
    /// (the `typescript` target's output). Defaults to `./stele.gen`.
    pub core: Option<String>,
    /// For the `react` target: the import specifier to the `store` target's
    /// output (where the hooks read/write the active locale). Defaults to
    /// `./stele.store`.
    pub store: Option<String>,
    /// Output identifier case: `camel` (default), `snake`, `pascal`, or
    /// `preserve`. Input keys may be in any case; this picks the output.
    pub case: Option<String>,
    /// The brand name the generated API is built around. Defaults to `stele`,
    /// giving the type `Stele`, the factory `createStele`, and (react target)
    /// `useStele` / `SteleProvider`. Set to e.g. `copy` for the classic
    /// `Copy` / `createCopy` / `useCopy` names.
    pub binding: Option<String>,
}
