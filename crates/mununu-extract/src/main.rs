//! Mununu extraction frontend — model extraction from various sources.
//!
//! Supports:
//! - `ast`: AST-based extraction from TypeScript/Python/Rust source code
//! - `circt`: Reactive system extraction from CIRCT MLIR output
//! - `llvm`: Extraction from LLVM IR via GEP-based analysis

mod circt;
mod llvm;

use clap::{Parser, Subcommand};
use mununu_core::adapter::extraction::ast_extract;
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(
    name = "mununu-extract",
    about = "Model extraction from source code and hardware descriptions"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// AST-based extraction from TypeScript/Python/Rust source code.
    Ast {
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
    },

    /// Extract reactive system from CIRCT MLIR output.
    ///
    /// Usage: circt-verilog design.sv | mununu-extract circt --output spec.espec.json
    Circt {
        /// MLIR input file (reads from stdin if omitted).
        input: Option<PathBuf>,

        /// Output path for the generated .espec.json. Defaults to stdout.
        #[arg(long, short)]
        output: Option<PathBuf>,
    },

    /// Extract from LLVM IR via GEP-based struct field analysis.
    ///
    /// Usage: rustc --emit=llvm-ir source.rs && mununu-extract llvm source.ll
    Llvm {
        /// LLVM IR input file (.ll).
        input: PathBuf,

        /// Output path for the generated .espec.json. Defaults to stdout.
        #[arg(long, short)]
        output: Option<PathBuf>,

        /// Target struct name (auto-detected if omitted).
        #[arg(long = "struct")]
        target_struct: Option<String>,
    },
}

fn main() {
    let cli = Cli::parse();

    let result = match cli.command {
        Command::Ast {
            config,
            source,
            output,
            language,
            list_domains,
        } => run_ast(
            &config,
            &source,
            output.as_deref(),
            language.as_deref(),
            list_domains,
        ),
        Command::Circt { input, output } => run_circt(input.as_deref(), output.as_deref()),
        Command::Llvm {
            input,
            output,
            target_struct,
        } => run_llvm(&input, output.as_deref(), target_struct.as_deref()),
    };

    if let Err(e) = result {
        eprintln!("Error: {e}");
        std::process::exit(1);
    }
}

fn run_ast(
    config_path: &std::path::Path,
    source_path: &std::path::Path,
    output: Option<&std::path::Path>,
    language: Option<&str>,
    list_domains: bool,
) -> Result<(), String> {
    if list_domains {
        println!("Available domain profiles:");
        for name in ast_extract::domain::available_profiles() {
            let profile = ast_extract::domain::get_profile(name).unwrap();
            println!("  {name:30} — {}", profile.description);
        }
        return Ok(());
    }

    let config_content = std::fs::read_to_string(config_path)
        .map_err(|e| format!("Failed to read config '{}': {e}", config_path.display()))?;

    let source_content = std::fs::read_to_string(source_path)
        .map_err(|e| format!("Failed to read source '{}': {e}", source_path.display()))?;

    let language = language
        .or_else(|| {
            source_path.to_str().and_then(|p| {
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
        source_path.display(),
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

    write_json(&spec, output)
}

fn run_circt(
    input: Option<&std::path::Path>,
    output: Option<&std::path::Path>,
) -> Result<(), String> {
    let mlir_text = if let Some(path) = input {
        std::fs::read_to_string(path)
            .map_err(|e| format!("Failed to read '{}': {e}", path.display()))?
    } else {
        use std::io::Read;
        let mut buf = String::new();
        std::io::stdin()
            .read_to_string(&mut buf)
            .map_err(|e| format!("Failed to read stdin: {e}"))?;
        buf
    };

    let module = circt::parse_mlir(&mlir_text);
    let module_name = module.name.clone().unwrap_or_else(|| "unknown".to_string());

    eprintln!("Module: {module_name}");
    eprintln!(
        "Inputs: {:?}",
        module.inputs.iter().map(|p| &p.name).collect::<Vec<_>>()
    );
    eprintln!(
        "Registers: {:?}",
        module.registers.iter().map(|r| &r.name).collect::<Vec<_>>()
    );
    eprintln!("SSA ops: {}", module.ops.len());

    let system = circt::extract_reactive_system(&module)?;

    eprintln!(
        "States: {}/{} reachable",
        system.reachable, system.total_enumerated
    );
    eprintln!("Transitions: {}", system.transitions.len());

    let spec = circt::build_espec(&system);
    write_json(&spec, output)
}

fn run_llvm(
    input: &std::path::Path,
    output: Option<&std::path::Path>,
    target_struct: Option<&str>,
) -> Result<(), String> {
    let ir_text = std::fs::read_to_string(input)
        .map_err(|e| format!("Failed to read '{}': {e}", input.display()))?;

    let module = llvm::parse_llvm_ir(&ir_text);

    eprintln!(
        "Source: {}",
        module.source_filename.as_deref().unwrap_or("unknown")
    );
    eprintln!(
        "Struct types: {:?}",
        module.struct_types.keys().collect::<Vec<_>>()
    );
    eprintln!("Functions: {}", module.functions.len());

    let spec = llvm::build_espec(&module, target_struct);

    for aut in &spec.model_config.automata {
        eprintln!(
            "  {} — {} states, {} transitions",
            aut.id,
            aut.states.len(),
            aut.transitions.len(),
        );
    }

    write_json(&spec, output)
}

fn write_json<T: serde::Serialize>(
    value: &T,
    output: Option<&std::path::Path>,
) -> Result<(), String> {
    let json = serde_json::to_string_pretty(value)
        .map_err(|e| format!("Failed to serialize output: {e}"))?;

    if let Some(path) = output {
        std::fs::write(path, &json)
            .map_err(|e| format!("Failed to write '{}': {e}", path.display()))?;
        eprintln!("Written to {}", path.display());
    } else {
        println!("{json}");
    }

    Ok(())
}
