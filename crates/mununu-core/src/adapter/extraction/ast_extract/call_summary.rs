//! Call summaries for external library methods.
//!
//! When the AST extractor encounters a call to an external method (e.g.,
//! `Map.set()`, `Vec.push()`, `dict.__setitem__`), it needs to know
//! what effect this has on the model state without seeing the implementation.
//!
//! Call summaries provide this information. Each domain profile includes
//! built-in summaries for standard library types. The user can override
//! or extend these in the `.extract.json` config.

use std::collections::HashMap;

/// Effect of an external call on model state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallEffect {
    /// Increments a bounded counter (e.g., Map.set, Vec.push).
    IncrementCounter,
    /// Decrements a bounded counter (e.g., Map.delete, Vec.pop).
    DecrementCounter,
    /// Resets a counter to zero (e.g., Map.clear).
    ResetToZero,
    /// Sets a boolean field to true.
    SetTrue,
    /// Sets a boolean field to false.
    SetFalse,
    /// Sets a presence field to present (Some).
    SetPresent,
    /// Sets a presence field to absent (None).
    SetAbsent,
    /// Read-only access — implies a guard but no state change.
    ReadOnly,
    /// No effect on model state (infrastructure call).
    None,
    /// Unknown effect — over-approximate with nondeterministic choice.
    Unknown,
}

/// Guard condition implied by a call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallGuard {
    /// Counter must be greater than zero (e.g., Map.get succeeds).
    CounterGtZero,
    /// Counter must equal zero (e.g., Map.has returns false).
    CounterEqZero,
    /// Field must be present (Some).
    MustBePresent,
    /// Field must be absent (None).
    MustBeAbsent,
    /// Field must be true.
    MustBeTrue,
    /// Field must be false.
    MustBeFalse,
    /// Field must equal a specific enum variant (e.g., match case guard).
    MustEqual(String),
    /// Disjunction of two guards: satisfied when EITHER side is. Used for
    /// same-field `||` patterns (e.g., `count == 0 || count > MAX`). Both
    /// inner guards must apply to the same field — cross-field disjunctions
    /// are over-approximated to `None` upstream.
    Disjunction(Box<CallGuard>, Box<CallGuard>),
    /// Conjunction of two guards on the same field: satisfied when BOTH
    /// sides are. Constructed when applying De Morgan to a negated
    /// disjunction (`!(a || b)` → `!a && !b`). Cross-field conjunctions
    /// continue to be expressed as multiple `Vec<Guard>` entries — one
    /// `Guard` per field.
    Conjunction(Box<CallGuard>, Box<CallGuard>),
    /// No guard.
    None,
}

/// Resolved summary for a call site.
#[derive(Debug, Clone)]
pub struct ResolvedCallSummary {
    /// Effect on the state field.
    pub effect: CallEffect,
    /// Guard implied by the call.
    pub guard: CallGuard,
    /// Which state field is affected (field name in the enclosing class).
    pub target_field: Option<String>,
}

/// Library of built-in call summaries, keyed by qualified method name.
pub struct CallSummaryLibrary {
    entries: HashMap<String, BuiltinSummary>,
}

#[derive(Debug, Clone)]
struct BuiltinSummary {
    effect: CallEffect,
    guard: CallGuard,
    /// How to determine the target field: "receiver" = the object the method
    /// is called on, "first_arg" = first argument.
    target_resolution: TargetResolution,
}

#[derive(Debug, Clone)]
enum TargetResolution {
    /// The target field is the object the method is called on (e.g., `this._map.set(...)` → `_map`).
    Receiver,
    /// No target — call doesn't affect state fields.
    None,
}

impl CallSummaryLibrary {
    /// Create a library with built-in summaries for a language.
    pub fn for_language(language: &str) -> Self {
        let entries = match language {
            "typescript" | "javascript" => typescript_summaries(),
            "python" => python_summaries(),
            "rust" => rust_summaries(),
            _ => HashMap::new(),
        };
        Self { entries }
    }

    /// Resolve a call at a specific site, given the receiver field name.
    pub fn resolve(
        &self,
        qualified_name: &str,
        receiver_field: Option<&str>,
    ) -> ResolvedCallSummary {
        if let Some(builtin) = self.entries.get(qualified_name) {
            let target_field = match &builtin.target_resolution {
                TargetResolution::Receiver => receiver_field.map(String::from),
                TargetResolution::None => None,
            };
            ResolvedCallSummary {
                effect: builtin.effect.clone(),
                guard: builtin.guard.clone(),
                target_field,
            }
        } else {
            // Unknown call — over-approximate
            ResolvedCallSummary {
                effect: CallEffect::Unknown,
                guard: CallGuard::None,
                target_field: receiver_field.map(String::from),
            }
        }
    }

    /// B6: Resolve a call by unqualified method name only — useful when the
    /// receiver type is unknown but the method name uniquely identifies the
    /// effect (e.g., `<thing>.push(...)` always increments, regardless of
    /// whether `<thing>` is an Array or a custom class with a similar API).
    ///
    /// Returns `Some(effect, guard)` only when ALL builtin entries whose key
    /// ends with `.<method_name>` (or equals it) agree on effect and guard.
    /// Disagreement (cross-type ambiguity) returns `None` — callers should
    /// fall back to over-approximation.
    pub fn resolve_unqualified(&self, method_name: &str) -> Option<(CallEffect, CallGuard)> {
        let suffix = format!(".{method_name}");
        let candidates: Vec<&BuiltinSummary> = self
            .entries
            .iter()
            .filter(|(k, _)| k.ends_with(&suffix) || k.as_str() == method_name)
            .map(|(_, v)| v)
            .collect();

        if candidates.is_empty() {
            return None;
        }

        let first = candidates[0];
        // Require ALL matches to agree — guards conservatively against
        // cross-type ambiguity (e.g., `clear` would match Map.clear, Set.clear,
        // dict.clear, all of which agree on ResetToZero, so OK).
        let all_agree = candidates.iter().all(|c| {
            c.effect == first.effect
                && c.guard == first.guard
                && std::mem::discriminant(&c.target_resolution)
                    == std::mem::discriminant(&first.target_resolution)
        });
        if !all_agree {
            return None;
        }

        // Only emit when the resolution is Receiver-based — the only case
        // we can wire from a `this.<field>.<method>()` call site without
        // additional type info.
        match first.target_resolution {
            TargetResolution::Receiver => Some((first.effect.clone(), first.guard.clone())),
            TargetResolution::None => None,
        }
    }

    /// Merge user-provided summaries from config (overrides built-in).
    pub fn merge_config_summaries(
        &mut self,
        config_summaries: &HashMap<String, super::config::CallSummary>,
    ) {
        for (name, summary) in config_summaries {
            let effect = match summary.effect.as_str() {
                "increment_counter" => CallEffect::IncrementCounter,
                "decrement_counter" => CallEffect::DecrementCounter,
                "reset_to_zero" => CallEffect::ResetToZero,
                "set_true" => CallEffect::SetTrue,
                "set_false" => CallEffect::SetFalse,
                "set_present" => CallEffect::SetPresent,
                "set_absent" => CallEffect::SetAbsent,
                "read_only" => CallEffect::ReadOnly,
                "none" => CallEffect::None,
                _ => CallEffect::Unknown,
            };
            let guard = match summary.guard.as_deref() {
                Some("counter_gt_zero") => CallGuard::CounterGtZero,
                Some("counter_eq_zero") => CallGuard::CounterEqZero,
                Some("must_be_present") => CallGuard::MustBePresent,
                Some("must_be_absent") => CallGuard::MustBeAbsent,
                Some("must_be_true") => CallGuard::MustBeTrue,
                Some("must_be_false") => CallGuard::MustBeFalse,
                _ => CallGuard::None,
            };
            let target_resolution = match summary.on_field.as_deref() {
                Some("receiver") => TargetResolution::Receiver,
                _ => TargetResolution::None,
            };
            self.entries.insert(
                name.clone(),
                BuiltinSummary {
                    effect,
                    guard,
                    target_resolution,
                },
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Built-in summaries per language
// ---------------------------------------------------------------------------

/// Helper: build a `BuiltinSummary` whose target is the receiver object.
fn receiver_entry(effect: CallEffect, guard: CallGuard) -> BuiltinSummary {
    BuiltinSummary {
        effect,
        guard,
        target_resolution: TargetResolution::Receiver,
    }
}

fn typescript_summaries() -> HashMap<String, BuiltinSummary> {
    let mut m = HashMap::new();

    // Map
    m.insert(
        "Map.prototype.set".into(),
        receiver_entry(CallEffect::IncrementCounter, CallGuard::None),
    );
    m.insert(
        "Map.prototype.delete".into(),
        receiver_entry(CallEffect::DecrementCounter, CallGuard::None),
    );
    m.insert(
        "Map.prototype.clear".into(),
        receiver_entry(CallEffect::ResetToZero, CallGuard::None),
    );
    m.insert(
        "Map.prototype.get".into(),
        receiver_entry(CallEffect::ReadOnly, CallGuard::CounterGtZero),
    );
    m.insert(
        "Map.prototype.has".into(),
        receiver_entry(CallEffect::ReadOnly, CallGuard::None),
    );

    // Set
    m.insert(
        "Set.prototype.add".into(),
        receiver_entry(CallEffect::IncrementCounter, CallGuard::None),
    );
    m.insert(
        "Set.prototype.delete".into(),
        receiver_entry(CallEffect::DecrementCounter, CallGuard::None),
    );
    m.insert(
        "Set.prototype.clear".into(),
        receiver_entry(CallEffect::ResetToZero, CallGuard::None),
    );

    // Array
    m.insert(
        "Array.prototype.push".into(),
        receiver_entry(CallEffect::IncrementCounter, CallGuard::None),
    );
    m.insert(
        "Array.prototype.pop".into(),
        receiver_entry(CallEffect::DecrementCounter, CallGuard::CounterGtZero),
    );

    // Infrastructure (no effect)
    m.insert(
        "console.log".into(),
        BuiltinSummary {
            effect: CallEffect::None,
            guard: CallGuard::None,
            target_resolution: TargetResolution::None,
        },
    );
    m.insert(
        "crypto.randomUUID".into(),
        BuiltinSummary {
            effect: CallEffect::None,
            guard: CallGuard::None,
            target_resolution: TargetResolution::None,
        },
    );

    m
}

fn python_summaries() -> HashMap<String, BuiltinSummary> {
    let mut m = HashMap::new();

    // dict
    m.insert(
        "dict.__setitem__".into(),
        receiver_entry(CallEffect::IncrementCounter, CallGuard::None),
    );
    m.insert(
        "dict.__delitem__".into(),
        receiver_entry(CallEffect::DecrementCounter, CallGuard::None),
    );
    m.insert(
        "dict.clear".into(),
        receiver_entry(CallEffect::ResetToZero, CallGuard::None),
    );
    m.insert(
        "dict.get".into(),
        receiver_entry(CallEffect::ReadOnly, CallGuard::None),
    );
    m.insert(
        "dict.pop".into(),
        receiver_entry(CallEffect::DecrementCounter, CallGuard::None),
    );

    // list
    m.insert(
        "list.append".into(),
        receiver_entry(CallEffect::IncrementCounter, CallGuard::None),
    );
    m.insert(
        "list.pop".into(),
        receiver_entry(CallEffect::DecrementCounter, CallGuard::CounterGtZero),
    );
    m.insert(
        "list.clear".into(),
        receiver_entry(CallEffect::ResetToZero, CallGuard::None),
    );

    // set
    m.insert(
        "set.add".into(),
        receiver_entry(CallEffect::IncrementCounter, CallGuard::None),
    );
    m.insert(
        "set.discard".into(),
        receiver_entry(CallEffect::DecrementCounter, CallGuard::None),
    );

    // contextvars.ContextVar — token-sequence depth abstracted as a bounded
    // counter. `set(value) → token`: increment. `reset(token)`: decrement
    // (guarded gt-zero — must have set first). `get(default)`: read-only.
    // GAP-005a: surfaced by MCP-003 validation; field detection (GAP-005
    // step 2) worked, but methods produced no effects because these entries
    // were missing — the resulting automaton was still 1-state degenerate.
    m.insert(
        "ContextVar.set".into(),
        receiver_entry(CallEffect::IncrementCounter, CallGuard::None),
    );
    m.insert(
        "ContextVar.reset".into(),
        receiver_entry(CallEffect::DecrementCounter, CallGuard::CounterGtZero),
    );
    m.insert(
        "ContextVar.get".into(),
        receiver_entry(CallEffect::ReadOnly, CallGuard::None),
    );

    m
}

fn rust_summaries() -> HashMap<String, BuiltinSummary> {
    let mut m = HashMap::new();

    // HashMap
    m.insert(
        "HashMap.insert".into(),
        receiver_entry(CallEffect::IncrementCounter, CallGuard::None),
    );
    m.insert(
        "HashMap.remove".into(),
        receiver_entry(CallEffect::DecrementCounter, CallGuard::None),
    );
    m.insert(
        "HashMap.clear".into(),
        receiver_entry(CallEffect::ResetToZero, CallGuard::None),
    );

    // Vec
    m.insert(
        "Vec.push".into(),
        receiver_entry(CallEffect::IncrementCounter, CallGuard::None),
    );
    m.insert(
        "Vec.pop".into(),
        receiver_entry(CallEffect::DecrementCounter, CallGuard::CounterGtZero),
    );
    m.insert(
        "Vec.clear".into(),
        receiver_entry(CallEffect::ResetToZero, CallGuard::None),
    );

    // Option
    m.insert(
        "Option.take".into(),
        receiver_entry(CallEffect::SetAbsent, CallGuard::MustBePresent),
    );
    m.insert(
        "Option.replace".into(),
        receiver_entry(CallEffect::SetPresent, CallGuard::None),
    );

    m
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typescript_map_set() {
        let lib = CallSummaryLibrary::for_language("typescript");
        let resolved = lib.resolve("Map.prototype.set", Some("_streamMapping"));
        assert_eq!(resolved.effect, CallEffect::IncrementCounter);
        assert_eq!(resolved.target_field.as_deref(), Some("_streamMapping"));
    }

    #[test]
    fn typescript_map_clear() {
        let lib = CallSummaryLibrary::for_language("typescript");
        let resolved = lib.resolve("Map.prototype.clear", Some("_requestResponseMap"));
        assert_eq!(resolved.effect, CallEffect::ResetToZero);
    }

    #[test]
    fn unknown_call_over_approximates() {
        let lib = CallSummaryLibrary::for_language("typescript");
        let resolved = lib.resolve("SomeLibrary.doThing", Some("_field"));
        assert_eq!(resolved.effect, CallEffect::Unknown);
        assert_eq!(resolved.target_field.as_deref(), Some("_field"));
    }

    #[test]
    fn python_dict_operations() {
        let lib = CallSummaryLibrary::for_language("python");
        assert_eq!(
            lib.resolve("dict.__setitem__", Some("store")).effect,
            CallEffect::IncrementCounter
        );
        assert_eq!(
            lib.resolve("dict.clear", Some("store")).effect,
            CallEffect::ResetToZero
        );
    }

    #[test]
    fn rust_option_operations() {
        let lib = CallSummaryLibrary::for_language("rust");
        let resolved = lib.resolve("Option.take", Some("zero_rtt_crypto"));
        assert_eq!(resolved.effect, CallEffect::SetAbsent);
        assert_eq!(resolved.guard, CallGuard::MustBePresent);
    }

    #[test]
    fn config_overrides_builtin() {
        let mut lib = CallSummaryLibrary::for_language("typescript");
        let mut overrides = HashMap::new();
        overrides.insert(
            "Map.prototype.set".to_string(),
            super::super::config::CallSummary {
                effect: "none".to_string(),
                on_field: None,
                guard: None,
                note: Some("Override for testing".to_string()),
            },
        );
        lib.merge_config_summaries(&overrides);
        let resolved = lib.resolve("Map.prototype.set", Some("_map"));
        assert_eq!(resolved.effect, CallEffect::None); // overridden
    }
}
