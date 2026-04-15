//! Domain profiles — non-trivial, language-aware defaults for extraction.
//!
//! Each domain profile encodes heuristics for controllability, abstraction,
//! composition, and labeling that are specific to a class of systems.
//! The agent selects a domain; the tool applies its heuristics; config
//! overrides take precedence.

use super::config::AbstractionType;

/// A domain profile providing defaults for extraction.
#[derive(Debug, Clone)]
pub struct DomainProfile {
    /// Profile identifier.
    pub name: &'static str,
    /// Primary language this profile targets.
    pub language: &'static str,
    /// Human-readable description.
    pub description: &'static str,
    /// Controllability heuristics.
    pub controllability: ControllabilityHeuristics,
    /// Abstraction heuristics for field types.
    pub abstraction: AbstractionHeuristics,
    /// Composition defaults.
    pub composition: CompositionHeuristics,
    /// Label naming convention.
    pub label_naming: LabelNaming,
    /// Whether to add noop self-loops on all states.
    pub add_noop_self_loops: bool,
}

/// Rules for determining whether a method is controllable.
#[derive(Debug, Clone)]
pub struct ControllabilityHeuristics {
    /// Default controllability when no rule matches.
    pub default: Controllability,
    /// Method name patterns that are controllable (glob-like: "start*", "send*").
    pub controllable_patterns: &'static [&'static str],
    /// Method name patterns that are uncontrollable.
    pub uncontrollable_patterns: &'static [&'static str],
    /// Rationale for this classification.
    pub rationale: &'static str,
}

/// Controllability classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Controllability {
    Controllable,
    Uncontrollable,
}

/// Rules for how field types map to abstract domains.
#[derive(Debug, Clone)]
pub struct AbstractionHeuristics {
    /// Default abstraction for boolean fields.
    pub boolean_default: AbstractionType,
    /// Default abstraction for optional/nullable fields.
    pub optional_default: AbstractionType,
    /// Default abstraction for Map/dict/HashMap fields.
    pub map_default: AbstractionType,
    /// Default bound for bounded_counter abstractions.
    pub default_counter_bound: i64,
    /// Default abstraction for enum/union fields.
    pub enum_default: AbstractionType,
    /// Default for string fields (typically Ignored unless explicitly included).
    pub string_default: AbstractionType,
    /// Default for numeric fields.
    pub numeric_default: AbstractionType,
}

/// Default composition behavior.
#[derive(Debug, Clone)]
pub struct CompositionHeuristics {
    /// Default composition type: "synchronous" or "asynchronous".
    pub default_type: &'static str,
    /// Rationale.
    pub rationale: &'static str,
}

/// Label naming convention.
#[derive(Debug, Clone)]
pub struct LabelNaming {
    /// Prefix for event labels.
    pub prefix: &'static str,
    /// Case convention: "preserve", "snake_case", "camelCase".
    pub case: &'static str,
}

/// Look up a domain profile by name.
pub fn get_profile(name: &str) -> Option<&'static DomainProfile> {
    PROFILES.iter().find(|p| p.name == name)
}

/// List available domain profile names.
pub fn available_profiles() -> Vec<&'static str> {
    PROFILES.iter().map(|p| p.name).collect()
}

/// Classify a method's controllability using the profile's heuristics.
pub fn classify_controllability(profile: &DomainProfile, method_name: &str) -> Controllability {
    for pattern in profile.controllability.controllable_patterns {
        if matches_glob(method_name, pattern) {
            return Controllability::Controllable;
        }
    }
    for pattern in profile.controllability.uncontrollable_patterns {
        if matches_glob(method_name, pattern) {
            return Controllability::Uncontrollable;
        }
    }
    profile.controllability.default
}

/// Determine abstraction type for a field based on its source type string.
pub fn infer_abstraction(profile: &DomainProfile, type_str: &str) -> AbstractionType {
    let lower = type_str.to_lowercase();
    if lower == "boolean" || lower == "bool" {
        profile.abstraction.boolean_default
    } else if lower.contains("option") || lower.contains("undefined") || lower.ends_with('?') {
        profile.abstraction.optional_default
    } else if lower.contains("map")
        || lower.contains("dict")
        || lower.contains("hashmap")
        || lower.contains("set")
        || lower.contains("vec")
        || lower.contains("array")
        || lower.contains("list")
    {
        profile.abstraction.map_default
    } else if lower.contains("enum") || lower.contains('|') {
        profile.abstraction.enum_default
    } else if lower.contains("string") || lower == "str" {
        profile.abstraction.string_default
    } else if lower.contains("number")
        || lower.contains("i32")
        || lower.contains("u32")
        || lower.contains("i64")
        || lower.contains("u64")
        || lower.contains("int")
        || lower.contains("float")
        || lower.contains("f64")
    {
        profile.abstraction.numeric_default
    } else {
        // Unknown type — default to ignored with a warning
        AbstractionType::Ignored
    }
}

/// Simple glob matching: supports trailing `*` only.
fn matches_glob(name: &str, pattern: &str) -> bool {
    if let Some(prefix) = pattern.strip_suffix('*') {
        name.starts_with(prefix)
    } else {
        name == pattern
    }
}

// ---------------------------------------------------------------------------
// Built-in profiles
// ---------------------------------------------------------------------------

static PROFILES: &[DomainProfile] = &[
    // MCP Server (TypeScript)
    DomainProfile {
        name: "mcp_server",
        language: "typescript",
        description: "MCP server-side components. Client requests are uncontrollable; \
                      server lifecycle methods (start, close, send) are controllable.",
        controllability: ControllabilityHeuristics {
            default: Controllability::Uncontrollable,
            controllable_patterns: &["start", "close", "send", "emit", "stop", "shutdown"],
            uncontrollable_patterns: &["handle*", "on*", "process*", "validate*"],
            rationale: "Server chooses when to start/close/send (controllable), \
                       but client requests arrive nondeterministically (uncontrollable)",
        },
        abstraction: AbstractionHeuristics {
            boolean_default: AbstractionType::Boolean,
            optional_default: AbstractionType::Presence,
            map_default: AbstractionType::BoundedCounter,
            default_counter_bound: 3,
            enum_default: AbstractionType::EnumValues,
            string_default: AbstractionType::Ignored,
            numeric_default: AbstractionType::Ignored,
        },
        composition: CompositionHeuristics {
            default_type: "asynchronous",
            rationale: "Multiple clients access the server concurrently; \
                       nondeterministic interleaving models concurrent HTTP requests",
        },
        label_naming: LabelNaming {
            prefix: "ev_",
            case: "preserve",
        },
        add_noop_self_loops: true,
    },
    // Protocol Implementation (Rust)
    DomainProfile {
        name: "protocol_implementation",
        language: "rust",
        description: "Protocol implementations (e.g., QUIC, TLS). Public API is \
                      controllable; internal handlers are uncontrollable.",
        controllability: ControllabilityHeuristics {
            default: Controllability::Controllable,
            controllable_patterns: &["connect", "send", "close", "accept", "listen", "init*"],
            uncontrollable_patterns: &["handle_*", "on_*", "process_*", "recv*", "poll*"],
            rationale: "Caller controls the public API; internal handlers respond to events",
        },
        abstraction: AbstractionHeuristics {
            boolean_default: AbstractionType::Boolean,
            optional_default: AbstractionType::Presence,
            map_default: AbstractionType::BoundedCounter,
            default_counter_bound: 3,
            enum_default: AbstractionType::EnumValues,
            string_default: AbstractionType::Ignored,
            numeric_default: AbstractionType::Ignored,
        },
        composition: CompositionHeuristics {
            default_type: "synchronous",
            rationale: "Single-struct internals are synchronous; \
                       use asynchronous when composing multiple modules",
        },
        label_naming: LabelNaming {
            prefix: "ev_",
            case: "snake_case",
        },
        add_noop_self_loops: true,
    },
    // Python Server
    DomainProfile {
        name: "python_server",
        language: "python",
        description: "Python async server components (e.g., FastMCP, Django, Flask).",
        controllability: ControllabilityHeuristics {
            default: Controllability::Uncontrollable,
            controllable_patterns: &["start*", "stop*", "close*", "send*", "shutdown*"],
            uncontrollable_patterns: &["handle_*", "on_*", "_*"],
            rationale: "Public methods without underscore prefix are interface points; \
                       private methods are internal",
        },
        abstraction: AbstractionHeuristics {
            boolean_default: AbstractionType::Boolean,
            optional_default: AbstractionType::Presence,
            map_default: AbstractionType::BoundedCounter,
            default_counter_bound: 3,
            enum_default: AbstractionType::EnumValues,
            string_default: AbstractionType::Ignored,
            numeric_default: AbstractionType::Ignored,
        },
        composition: CompositionHeuristics {
            default_type: "asynchronous",
            rationale: "Asyncio event loop allows concurrent request handling",
        },
        label_naming: LabelNaming {
            prefix: "ev_",
            case: "snake_case",
        },
        add_noop_self_loops: true,
    },
    // Hardware RTL
    DomainProfile {
        name: "hardware_rtl",
        language: "systemverilog",
        description: "Hardware RTL designs. Input ports are uncontrollable (environment); \
                      output ports are controllable (system).",
        controllability: ControllabilityHeuristics {
            default: Controllability::Uncontrollable,
            controllable_patterns: &[],
            uncontrollable_patterns: &[],
            rationale: "Port direction determines controllability; \
                       input = environment, output = system",
        },
        abstraction: AbstractionHeuristics {
            boolean_default: AbstractionType::Boolean,
            optional_default: AbstractionType::Presence,
            map_default: AbstractionType::BoundedCounter,
            default_counter_bound: 3,
            enum_default: AbstractionType::EnumValues,
            string_default: AbstractionType::Ignored,
            numeric_default: AbstractionType::BoundedCounter,
        },
        composition: CompositionHeuristics {
            default_type: "synchronous",
            rationale: "Hardware is clocked; all transitions happen synchronously",
        },
        label_naming: LabelNaming {
            prefix: "",
            case: "snake_case",
        },
        add_noop_self_loops: false,
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn get_mcp_server_profile() {
        let profile = get_profile("mcp_server").unwrap();
        assert_eq!(profile.language, "typescript");
        assert_eq!(
            profile.controllability.default,
            Controllability::Uncontrollable
        );
    }

    #[test]
    fn classify_mcp_controllability() {
        let profile = get_profile("mcp_server").unwrap();
        assert_eq!(
            classify_controllability(profile, "start"),
            Controllability::Controllable
        );
        assert_eq!(
            classify_controllability(profile, "close"),
            Controllability::Controllable
        );
        assert_eq!(
            classify_controllability(profile, "send"),
            Controllability::Controllable
        );
        assert_eq!(
            classify_controllability(profile, "handlePostRequest"),
            Controllability::Uncontrollable
        );
        assert_eq!(
            classify_controllability(profile, "onMessage"),
            Controllability::Uncontrollable
        );
        assert_eq!(
            classify_controllability(profile, "unknownMethod"),
            Controllability::Uncontrollable
        ); // default
    }

    #[test]
    fn classify_protocol_controllability() {
        let profile = get_profile("protocol_implementation").unwrap();
        assert_eq!(
            classify_controllability(profile, "connect"),
            Controllability::Controllable
        );
        assert_eq!(
            classify_controllability(profile, "handle_packet"),
            Controllability::Uncontrollable
        );
        assert_eq!(
            classify_controllability(profile, "some_other_fn"),
            Controllability::Controllable
        ); // default for protocol
    }

    #[test]
    fn infer_boolean_abstraction() {
        let profile = get_profile("mcp_server").unwrap();
        assert_eq!(
            infer_abstraction(profile, "boolean"),
            AbstractionType::Boolean
        );
        assert_eq!(infer_abstraction(profile, "bool"), AbstractionType::Boolean);
    }

    #[test]
    fn infer_map_abstraction() {
        let profile = get_profile("mcp_server").unwrap();
        assert_eq!(
            infer_abstraction(profile, "Map<string, StreamMapping>"),
            AbstractionType::BoundedCounter
        );
        assert_eq!(
            infer_abstraction(profile, "HashMap<K, V>"),
            AbstractionType::BoundedCounter
        );
    }

    #[test]
    fn infer_optional_abstraction() {
        let profile = get_profile("mcp_server").unwrap();
        assert_eq!(
            infer_abstraction(profile, "Option<ZeroRttCrypto>"),
            AbstractionType::Presence
        );
        assert_eq!(
            infer_abstraction(profile, "string | undefined"),
            AbstractionType::Presence
        );
    }

    #[test]
    fn infer_string_ignored() {
        let profile = get_profile("mcp_server").unwrap();
        assert_eq!(
            infer_abstraction(profile, "string"),
            AbstractionType::Ignored
        );
    }

    #[test]
    fn all_profiles_accessible() {
        let names = available_profiles();
        assert!(names.contains(&"mcp_server"));
        assert!(names.contains(&"protocol_implementation"));
        assert!(names.contains(&"python_server"));
        assert!(names.contains(&"hardware_rtl"));
    }

    #[test]
    fn glob_matching() {
        assert!(matches_glob("handlePostRequest", "handle*"));
        assert!(matches_glob("start", "start"));
        assert!(!matches_glob("start", "stop"));
        assert!(matches_glob("onMessage", "on*"));
        assert!(!matches_glob("connect", "handle*"));
    }
}
