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
}
