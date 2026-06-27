use serde::Deserialize;
use std::path::PathBuf;

#[derive(Deserialize)]
pub struct Config {
    pub canonical: String,
    pub locales: PathBuf,
    #[serde(default)]
    pub target: Vec<Target>,
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
