use crate::context::{ControllerSynthesisOptions, DiagnosticsOptions};

use super::ast::{ControllerOptions, DiagnosticsConfig};

/// Owned representation of controller options resolved from the DSL AST.
#[derive(Debug, Default, Clone)]
pub struct ResolvedControllerOptions {
    minimize: bool,
    diagnostics: Option<DiagnosticsOptions>,
}

impl ResolvedControllerOptions {
    /// Builds the resolved options from the AST representation parsed out of the DSL.
    pub fn from_ast(options: &ControllerOptions) -> Self {
        let minimize = options.minimize.unwrap_or(false);
        let diagnostics = options
            .diagnostics
            .as_ref()
            .map(diagnostics_options_from_config);

        Self {
            minimize,
            diagnostics,
        }
    }

    /// Returns the resolved diagnostics options, if present.
    pub fn diagnostics(&self) -> Option<&DiagnosticsOptions> {
        self.diagnostics.as_ref()
    }

    /// Returns the resolved minimise flag.
    pub fn minimize(&self) -> bool {
        self.minimize
    }

    /// Produces a borrowed `ControllerSynthesisOptions` that can be passed directly to
    /// `Context::synthesise_controller_with_options`.
    pub fn as_synthesis_options(&self) -> ControllerSynthesisOptions<'_> {
        ControllerSynthesisOptions {
            evaluation: None,
            diagnostics: self.diagnostics.as_ref(),
            minimize: self.minimize,
            extract_strategy: false,
            mode: crate::context::ControllerMode::default(),
        }
    }
}

fn diagnostics_options_from_config(config: &DiagnosticsConfig) -> DiagnosticsOptions {
    let mut options = DiagnosticsOptions::default();
    if let Some(flag) = config.counterexample {
        options.counterexample = flag;
    }
    if let Some(flag) = config.deadlock_traces {
        options.deadlock_traces = flag;
    }
    if let Some(limit) = config.max_counter_traces {
        options.max_counter_traces = Some(limit as usize);
    }
    if let Some(flag) = config.proof_obligations {
        options.proof_obligations = flag;
    }
    options
}
