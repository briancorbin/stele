mod check;
mod config;
mod emit;
mod exchange;
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
    /// Validate the catalog across locales (missing keys, placeholder drift,
    /// plural coverage). Exits non-zero on errors — wire it into CI.
    Check {
        #[arg(long, default_value = "stele.toml")]
        config: PathBuf,
        /// Treat warnings as failures too.
        #[arg(long)]
        strict: bool,
    },
    /// Export a locale to a translator-friendly file (CSV or XLIFF).
    Export {
        /// Target locale to translate into (e.g. `es`).
        #[arg(long)]
        locale: String,
        #[arg(long, default_value = "csv")]
        format: String,
        /// Output path (default: `<locale>.<csv|xliff>`).
        #[arg(long)]
        out: Option<PathBuf>,
        /// Only export entries the target locale hasn't translated yet.
        #[arg(long)]
        missing: bool,
        #[arg(long, default_value = "stele.toml")]
        config: PathBuf,
    },
    /// Import a completed translation file back into the JSON catalog.
    Import {
        /// The CSV or XLIFF file to import.
        file: PathBuf,
        /// Override the target locale (default: from the file).
        #[arg(long)]
        locale: Option<String>,
        /// Force a format (default: inferred from the file extension).
        #[arg(long)]
        format: Option<String>,
        #[arg(long, default_value = "stele.toml")]
        config: PathBuf,
    },
}

fn main() -> Result<()> {
    match Cli::parse().cmd {
        Cmd::Generate { config } => cmd_generate(config),
        Cmd::Ir { locales, canonical } => cmd_ir(locales, canonical),
        Cmd::Check { config, strict } => cmd_check(config, strict),
        Cmd::Export {
            locale,
            format,
            out,
            missing,
            config,
        } => cmd_export(config, locale, format, out, missing),
        Cmd::Import {
            file,
            locale,
            format,
            config,
        } => cmd_import(config, file, locale, format),
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

fn cmd_check(config_path: PathBuf, strict: bool) -> Result<()> {
    let text = std::fs::read_to_string(&config_path)
        .with_context(|| format!("reading {}", config_path.display()))?;
    let cfg: config::Config = toml::from_str(&text)?;
    let base = config_path.parent().unwrap_or_else(|| Path::new("."));
    let locales = model::load_locales(&base.join(&cfg.locales))?;
    if !locales.contains_key(&cfg.canonical) {
        return Err(anyhow!("canonical locale '{}' not found", cfg.canonical));
    }

    let report = check::check(&cfg.canonical, &locales)?;

    println!(
        "stele check — {} locales, {} keys (canonical: {})\n",
        locales.len(),
        report.key_count,
        cfg.canonical
    );

    use check::Severity;
    for loc in locales.keys() {
        let mut ds: Vec<_> = report
            .diagnostics
            .iter()
            .filter(|d| &d.locale == loc)
            .collect();
        if ds.is_empty() {
            println!("  \u{2713} {loc}");
            continue;
        }
        ds.sort_by_key(|d| (d.severity != Severity::Error, &d.key));
        let errs = ds.iter().filter(|d| d.severity == Severity::Error).count();
        let warns = ds.len() - errs;
        println!("  \u{2717} {loc} — {errs} error(s), {warns} warning(s)");
        for d in ds {
            let tag = match d.severity {
                Severity::Error => "error  ",
                Severity::Warning => "warning",
            };
            println!("      {tag}  {}  \u{2014}  {}", d.key, d.message);
        }
    }

    let errors = report.errors();
    let warnings = report.warnings();
    println!();
    if errors > 0 || (strict && warnings > 0) {
        println!("\u{2717} check failed — {errors} error(s), {warnings} warning(s)");
        std::process::exit(1);
    }
    if warnings > 0 {
        println!("\u{2713} no errors — {warnings} warning(s)");
    } else {
        println!("\u{2713} all locales complete and consistent");
    }
    Ok(())
}

fn load_config(config_path: &Path) -> Result<(config::Config, PathBuf)> {
    let text = std::fs::read_to_string(config_path)
        .with_context(|| format!("reading {}", config_path.display()))?;
    let cfg: config::Config = toml::from_str(&text)?;
    let base = config_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .to_path_buf();
    Ok((cfg, base))
}

fn cmd_export(
    config_path: PathBuf,
    locale: String,
    format: String,
    out: Option<PathBuf>,
    missing: bool,
) -> Result<()> {
    let fmt = exchange::parse_format(&format)?;
    let (cfg, base) = load_config(&config_path)?;
    if locale == cfg.canonical {
        return Err(anyhow!(
            "export target '{locale}' is the canonical locale — choose a locale to translate into"
        ));
    }
    let locales = model::load_locales(&base.join(&cfg.locales))?;
    let canonical = locales
        .get(&cfg.canonical)
        .ok_or_else(|| anyhow!("canonical locale '{}' not found", cfg.canonical))?;

    let mut units = exchange::export_units(canonical, locales.get(&locale), &locale)?;
    if missing {
        units.retain(|u| u.target.is_empty());
    }
    let total = units.len();

    let ext = if fmt == "csv" { "csv" } else { "xliff" };
    let out = out.unwrap_or_else(|| PathBuf::from(format!("{locale}.{ext}")));
    let rendered = match fmt {
        "csv" => exchange::to_csv(&cfg.canonical, &locale, &units)?,
        _ => exchange::to_xliff(&cfg.canonical, &locale, &units),
    };
    std::fs::write(&out, rendered)?;
    println!(
        "\u{2713} exported {total} {} \u{2192} {} ({fmt})",
        if missing {
            "untranslated entries"
        } else {
            "entries"
        },
        out.display()
    );
    Ok(())
}

fn cmd_import(
    config_path: PathBuf,
    file: PathBuf,
    locale: Option<String>,
    format: Option<String>,
) -> Result<()> {
    let (cfg, base) = load_config(&config_path)?;
    let text =
        std::fs::read_to_string(&file).with_context(|| format!("reading {}", file.display()))?;

    let fmt = match format {
        Some(f) => exchange::parse_format(&f)?,
        None => match file.extension().and_then(|e| e.to_str()) {
            Some("csv") => "csv",
            Some("xliff") | Some("xlf") => "xliff",
            _ => {
                return Err(anyhow!(
                    "can't infer format from '{}'; pass --format",
                    file.display()
                ))
            }
        },
    };

    let (file_locale, units) = match fmt {
        "csv" => exchange::from_csv(&text)?,
        _ => exchange::from_xliff(&text)?,
    };
    let target = locale.unwrap_or(file_locale);
    if target.is_empty() {
        return Err(anyhow!("no target locale in the file; pass --locale"));
    }
    if target == cfg.canonical {
        return Err(anyhow!(
            "refusing to import over the canonical locale '{target}'"
        ));
    }

    let locales_dir = base.join(&cfg.locales);
    if locales_dir.join(&target).is_dir() {
        return Err(anyhow!(
            "locale '{target}' is a folder ({}/) — import currently writes a single {target}.json; not yet supported for folder locales",
            target
        ));
    }

    // Structure comes from the canonical catalog; only strings come from the file.
    let all = model::load_locales(&locales_dir)?;
    let canonical = all
        .get(&cfg.canonical)
        .ok_or_else(|| anyhow!("canonical locale '{}' not found", cfg.canonical))?;
    let paths = exchange::build_paths(canonical, &units);
    let written = paths.len();

    // Merge onto the existing target file (leaf-level replace), preserving any
    // translations not present in this import.
    let target_file = locales_dir.join(format!("{target}.json"));
    let mut root = if target_file.exists() {
        serde_json::from_str(&std::fs::read_to_string(&target_file)?)?
    } else {
        serde_json::Value::Object(serde_json::Map::new())
    };
    for (path, value) in paths {
        exchange::set_path(&mut root, &path, value);
    }
    std::fs::write(
        &target_file,
        format!("{}\n", serde_json::to_string_pretty(&root)?),
    )?;
    println!(
        "\u{2713} imported {written} entries \u{2192} {} (run `stele check` to verify)",
        target_file.display()
    );
    Ok(())
}
