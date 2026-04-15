//! Mununu extraction frontend — AST-based model extraction from source code.
//!
//! Reads a `.extract.json` config and source file, parses the AST via
//! tree-sitter, derives automata from field domains and method behaviors,
//! and outputs a `.espec.json` extraction spec.

// Extractor module has WIP code — suppress until guard/effect extraction is complete.
// Tracked in Phase 2d of the restructure plan.
#![allow(unused_variables, clippy::collapsible_if, clippy::ptr_arg)]

pub mod extractor;
pub mod parser;

use clap::Parser;
use mununu_core::adapter::extraction::ast;
use mununu_core::adapter::extraction::ast_extract::{config, domain, state_space};
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
        for name in domain::available_profiles() {
            let profile = domain::get_profile(name).unwrap();
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
    // 1. Load config
    let config_content = std::fs::read_to_string(&cli.config)
        .map_err(|e| format!("Failed to read config '{}': {e}", cli.config.display()))?;
    let config: config::ExtractionConfig = serde_json::from_str(&config_content)
        .map_err(|e| format!("Failed to parse config: {e}"))?;

    // 2. Detect language
    let language_override = cli
        .language
        .as_deref()
        .or(config.language.as_deref())
        .and_then(parser::SourceLanguage::from_name);

    // 3. Parse source
    let parsed = parser::parse_file(&cli.source, language_override)?;
    eprintln!(
        "Parsed {:?} source: {} ({} bytes)",
        parsed.language,
        cli.source.display(),
        parsed.source.len()
    );

    // 4. Get domain profile
    let profile = config.domain.as_deref().and_then(domain::get_profile);

    if let Some(p) = profile {
        eprintln!("Domain profile: {} ({})", p.name, p.language);
    }

    // 5. Extract each target
    let mut all_automata = Vec::new();
    let mut all_warnings = Vec::new();
    let mut all_field_lines = Vec::new();
    let mut all_method_lines = Vec::new();

    for target in &config.targets {
        eprintln!(
            "Extracting target: {} → {}",
            target.class,
            target.automaton_id.as_deref().unwrap_or(&target.class)
        );
        let extracted = extractor::extract_target(&parsed, target, profile)?;

        eprintln!(
            "  {} fields, {} methods, {} warnings",
            extracted.fields.len(),
            extracted.methods.len(),
            extracted.warnings.len()
        );
        for w in &extracted.warnings {
            eprintln!("  WARN: {w}");
        }

        // 6. Derive automaton from extracted fields and methods
        let label_prefix = profile.map(|p| p.label_naming.prefix).unwrap_or("ev_");
        let add_noop = profile.map(|p| p.add_noop_self_loops).unwrap_or(true);

        let derived = state_space::derive_automaton(
            &extracted.automaton_id,
            &extracted.fields,
            &extracted.methods,
            &target.state_names,
            label_prefix,
            add_noop,
        );

        eprintln!(
            "  Derived: {} states, {} transitions, {} controllable labels",
            derived.states.len(),
            derived.transitions.len(),
            derived.controllable_labels.len()
        );

        // Convert to .espec.json format
        let automaton_def = to_automaton_def(&derived);
        all_automata.push(automaton_def);
        all_warnings.extend(extracted.warnings);
        all_field_lines.extend(extracted.field_lines);
        all_method_lines.extend(extracted.method_lines);
    }

    // 7. Build .espec.json output
    let context_name = config.context_name.clone().unwrap_or_else(|| {
        config
            .targets
            .first()
            .map(|t| t.class.to_lowercase())
            .unwrap_or_else(|| "extracted".to_string())
    });

    let composition = config.composition.as_ref().map(|c| ast::CompositionDef {
        type_: c.type_.clone(),
        name: c.name.clone(),
        members: all_automata.iter().map(|a| a.id.clone()).collect(),
    });

    let properties: Vec<ast::PropertyDef> = config
        .properties
        .iter()
        .map(|p| ast::PropertyDef {
            id: p.id.clone(),
            description: p.description.clone(),
            formula: Some(p.formula.clone()),
            formula_template: None,
            over: p.over.clone(),
            holds_in_fixed: None,
            holds_in_vulnerable: None,
        })
        .collect();

    let spec = ast::ExtractionSpec {
        schema: Some("extraction_spec_v1".to_string()),
        source: ast::SourceRef {
            repo: config.source.repo.clone(),
            commit: config.source.commit.clone(),
            file: Some(config.source.file.clone()),
            class: config.targets.first().map(|t| t.class.clone()),
            cve: None,
            ghsa: None,
            issue: None,
        },
        state_fields: vec![], // TODO: populate from extracted field lines
        methods: vec![],      // TODO: populate from extracted method lines
        bugs: vec![],
        model_config: ast::ModelConfig {
            context_name,
            controllable_labels: all_automata
                .iter()
                .flat_map(|a| a.controllable_labels.clone())
                .collect(),
            uncontrollable_labels: vec![],
            automata: all_automata,
            composition,
            properties,
            controllers: vec![],
        },
    };

    // 8. Serialize
    let json = serde_json::to_string_pretty(&spec)
        .map_err(|e| format!("Failed to serialize output: {e}"))?;

    if let Some(output_path) = &cli.output {
        std::fs::write(output_path, &json)
            .map_err(|e| format!("Failed to write '{}': {e}", output_path.display()))?;
        eprintln!("Written to {}", output_path.display());
    } else {
        println!("{json}");
    }

    if !all_warnings.is_empty() {
        eprintln!("\n{} warning(s) during extraction", all_warnings.len());
    }

    Ok(())
}

/// Convert a derived automaton to the `.espec.json` format.
fn to_automaton_def(derived: &state_space::DerivedAutomaton) -> ast::AutomatonDef {
    let states: Vec<ast::StateDef> = derived
        .states
        .iter()
        .map(|s| {
            ast::StateDef::Structured(ast::StateDefStructured {
                name: s.name.clone(),
                initial: s.is_initial,
            })
        })
        .collect();

    let transitions: Vec<ast::TransitionDef> = derived
        .transitions
        .iter()
        .map(|t| ast::TransitionDef {
            from: t.from.clone(),
            to: t.to.clone(),
            label: t.label.clone(),
            mode: "both".to_string(),
            derived_from: None,
            comment: None,
        })
        .collect();

    ast::AutomatonDef {
        id: derived.name.clone(),
        states,
        controllable_labels: derived.controllable_labels.clone(),
        transitions,
        fields: vec![],
        note: None,
        role: None,
    }
}
