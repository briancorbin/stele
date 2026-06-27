mod config;
mod emit;
mod ir;
mod model;
mod plural;

use anyhow::{anyhow, Context, Result};
use clap::{Parser, Subcommand};
use std::path::{Path, PathBuf};

#[derive(Parser)]
#[command(name = "stele", version, about = "JSON-first, type-safe i18n codegen")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Generate code for every target declared in stele.toml
    Generate {
        #[arg(long, default_value = "stele.toml")]
        config: PathBuf,
    },
    /// Dump the language-neutral intermediate representation as JSON
    Ir {
        #[arg(long)]
        locales: PathBuf,
        #[arg(long, default_value = "en")]
        canonical: String,
    },
}

fn main() -> Result<()> {
    match Cli::parse().cmd {
        Cmd::Generate { config } => cmd_generate(config),
        Cmd::Ir { locales, canonical } => cmd_ir(locales, canonical),
    }
}

fn cmd_generate(config_path: PathBuf) -> Result<()> {
    let text = std::fs::read_to_string(&config_path)
        .with_context(|| format!("reading {}", config_path.display()))?;
    let cfg: config::Config = toml::from_str(&text)?;
    let base = config_path.parent().unwrap_or_else(|| Path::new("."));

    let locales = model::load_locales(&base.join(&cfg.locales))?;
    let ir = ir::build_ir(&cfg.canonical, &locales)?;

    for target in &cfg.target {
        let case = emit::Case::parse(target.case.as_deref().unwrap_or("camel"))?;
        emit::validate_idents(&ir, case)?;
        let opts = emit::EmitOptions {
            callable: target.callable,
            core: target
                .core
                .clone()
                .unwrap_or_else(|| "./stele.gen".to_string()),
            store: target
                .store
                .clone()
                .unwrap_or_else(|| "./stele.store".to_string()),
            case,
            binding: emit::Binding::new(target.binding.as_deref().unwrap_or("stele")),
        };
        let emitter = emit::emitter_for(&target.lang, &opts)
            .ok_or_else(|| anyhow!("unknown target lang '{}'", target.lang))?;
        emitter.validate(&ir)?;
        let code = emitter.emit(&ir);
        let out = base.join(&target.out);
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&out, code)?;
        println!("\u{2713} {:<11} \u{2192} {}", target.lang, out.display());
    }

    if let Some(pkg) = &cfg.package {
        let case = emit::Case::parse(pkg.case.as_deref().unwrap_or("camel"))?;
        emit::validate_idents(&ir, case)?;
        let opts = emit::pkg::PackageOptions {
            name: pkg.name.clone(),
            version: pkg.version.clone().unwrap_or_else(|| "0.0.0".to_string()),
            store: pkg.store,
            react: pkg.react,
            callable: pkg.callable,
            case,
            binding: emit::Binding::new(pkg.binding.as_deref().unwrap_or("stele")),
        };
        let dir = base.join(&pkg.out);
        std::fs::create_dir_all(&dir)?;
        let files = emit::pkg::render(&ir, &opts);
        let count = files.len();
        for (name, content) in files {
            std::fs::write(dir.join(&name), content)?;
        }
        println!(
            "\u{2713} {:<11} \u{2192} {} ({} files)",
            "package",
            dir.display(),
            count
        );
    }

    Ok(())
}

fn cmd_ir(locales_dir: PathBuf, canonical: String) -> Result<()> {
    let locales = model::load_locales(&locales_dir)?;
    let ir = ir::build_ir(&canonical, &locales)?;
    println!("{}", serde_json::to_string_pretty(&ir)?);
    Ok(())
}
