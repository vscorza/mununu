//! Mununu extraction frontend — AST-based model extraction from source code.
//!
//! Thin CLI wrapper around `mununu_core::adapter::extraction::ast_extract::extract_from_source`.

use clap::Parser;
use mununu_core::adapter::extraction::ast_extract;
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(
    name = "mununu-extract",
    about = "AST-based model extraction from source code"
)]
struct Cli {
    /// Path to the extraction config (.extract.json).
    config: PathBuf,

    /// Path to the source file to extract from.
    #[arg(long)]
    source: PathBuf,

    /// Output path for the generated .espec.json. Defaults to stdout.
    #[arg(long, short)]
    output: Option<PathBuf>,

    /// Override the source language (typescript, python, rust).
    #[arg(long)]
    language: Option<String>,

    /// List available domain profiles and exit.
    #[arg(long)]
    list_domains: bool,
}

fn main() {
    let cli = Cli::parse();

    if cli.list_domains {
        println!("Available domain profiles:");
        for name in ast_extract::domain::available_profiles() {
            let profile = ast_extract::domain::get_profile(name).unwrap();
            println!("  {name:30} — {}", profile.description);
        }
        return;
    }

    match run(&cli) {
        Ok(()) => {}
        Err(e) => {
            eprintln!("Error: {e}");
            std::process::exit(1);
        }
    }
}

fn run(cli: &Cli) -> Result<(), String> {
    let config_content = std::fs::read_to_string(&cli.config)
        .map_err(|e| format!("Failed to read config '{}': {e}", cli.config.display()))?;

    let source_content = std::fs::read_to_string(&cli.source)
        .map_err(|e| format!("Failed to read source '{}': {e}", cli.source.display()))?;

    let language = cli
        .language
        .as_deref()
        .or_else(|| {
            cli.source.to_str().and_then(|p| {
                let ext = p.rsplit('.').next()?;
                match ext {
                    "ts" | "tsx" | "js" => Some("typescript"),
                    "py" => Some("python"),
                    "rs" => Some("rust"),
                    _ => None,
                }
            })
        })
        .unwrap_or("typescript");

    eprintln!(
        "Extracting from {} ({}) ...",
        cli.source.display(),
        language
    );

    let spec = ast_extract::extract_from_source(&config_content, &source_content, language)?;

    eprintln!(
        "Extracted: {} automata, {} properties",
        spec.model_config.automata.len(),
        spec.model_config.properties.len(),
    );
    for aut in &spec.model_config.automata {
        eprintln!(
            "  {} — {} states, {} transitions",
            aut.id,
            aut.states.len(),
            aut.transitions.len(),
        );
    }

    let json = serde_json::to_string_pretty(&spec)
        .map_err(|e| format!("Failed to serialize output: {e}"))?;

    if let Some(output_path) = &cli.output {
        std::fs::write(output_path, &json)
            .map_err(|e| format!("Failed to write '{}': {e}", output_path.display()))?;
        eprintln!("Written to {}", output_path.display());
    } else {
        println!("{json}");
    }

    Ok(())
}
