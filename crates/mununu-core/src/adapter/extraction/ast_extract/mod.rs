//! AST-based extraction — configuration, domain profiles, state space derivation,
//! and (behind `ast-extract` feature) tree-sitter parsing and extraction.
//!
//! Shared types (config, domain, call_summary, state_space) are always available.
//! Tree-sitter dependent code (parser, extractor, extract_from_source) requires
//! the `ast-extract` feature flag.

pub mod call_summary;
pub mod config;
pub mod domain;
pub mod state_space;

#[cfg(feature = "ast-extract")]
#[allow(clippy::collapsible_if)]
pub mod extractor;
#[cfg(feature = "ast-extract")]
pub mod parser;

/// Extract a model from source code using the AST-based extraction pipeline.
///
/// This is the main entry point for in-process extraction. It:
/// 1. Parses the extraction config (.extract.json)
/// 2. Parses the source file via tree-sitter
/// 3. Extracts fields, methods, guards, effects per target
/// 4. Derives automata via state space enumeration
/// 5. Returns a complete ExtractionSpec (.espec.json)
///
/// Requires the `ast-extract` feature flag.
#[cfg(feature = "ast-extract")]
pub fn extract_from_source(
    config_json: &str,
    source_code: &str,
    language: &str,
) -> Result<super::ast::ExtractionSpec, String> {
    let config: config::ExtractionConfig = serde_json::from_str(config_json)
        .map_err(|e| format!("Failed to parse extraction config: {e}"))?;

    let lang = language
        .parse::<String>()
        .ok()
        .and_then(|s| parser::SourceLanguage::from_name(&s))
        .or_else(|| {
            config
                .language
                .as_deref()
                .and_then(parser::SourceLanguage::from_name)
        })
        .ok_or_else(|| format!("Unknown language: {language}"))?;

    let parsed = parser::parse_source(source_code, lang)?;

    let profile = config.domain.as_deref().and_then(domain::get_profile);

    let mut all_automata = Vec::new();
    let mut all_warnings = Vec::new();

    for target in &config.targets {
        let extracted = extractor::extract_target(&parsed, target, profile)?;

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

        let automaton_def = super::ast::AutomatonDef {
            id: derived.name.clone(),
            states: derived
                .states
                .iter()
                .map(|s| {
                    super::ast::StateDef::Structured(super::ast::StateDefStructured {
                        name: s.name.clone(),
                        initial: s.is_initial,
                    })
                })
                .collect(),
            controllable_labels: derived.controllable_labels.clone(),
            transitions: derived
                .transitions
                .iter()
                .map(|t| super::ast::TransitionDef {
                    from: t.from.clone(),
                    to: t.to.clone(),
                    label: t.label.clone(),
                    mode: "both".to_string(),
                    derived_from: None,
                    comment: None,
                })
                .collect(),
            fields: vec![],
            note: None,
            role: None,
        };

        all_automata.push(automaton_def);
        all_warnings.extend(extracted.warnings);
    }

    let context_name = config.context_name.clone().unwrap_or_else(|| {
        config
            .targets
            .first()
            .map(|t| t.class.to_lowercase())
            .unwrap_or_else(|| "extracted".to_string())
    });

    let composition = config
        .composition
        .as_ref()
        .map(|c| super::ast::CompositionDef {
            type_: c.type_.clone(),
            name: c.name.clone(),
            members: all_automata.iter().map(|a| a.id.clone()).collect(),
        });

    let properties: Vec<super::ast::PropertyDef> = config
        .properties
        .iter()
        .map(|p| super::ast::PropertyDef {
            id: p.id.clone(),
            description: p.description.clone(),
            formula: Some(p.formula.clone()),
            formula_template: None,
            template_ref: None,
            over: p.over.clone(),
            holds_in_fixed: None,
            holds_in_vulnerable: None,
        })
        .collect();

    Ok(super::ast::ExtractionSpec {
        schema: Some("extraction_spec_v1".to_string()),
        source: super::ast::SourceRef {
            repo: config.source.repo.clone(),
            commit: config.source.commit.clone(),
            file: Some(config.source.file.clone()),
            class: config.targets.first().map(|t| t.class.clone()),
            cve: None,
            ghsa: None,
            issue: None,
            fix_pr: None,
            fix_commit: None,
        },
        state_fields: vec![],
        methods: vec![],
        bugs: vec![],
        model_config: super::ast::ModelConfig {
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
    })
}
