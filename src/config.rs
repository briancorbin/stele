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
}
