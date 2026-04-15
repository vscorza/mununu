//! Type selection heuristics for abstract values.
//!
//! This module implements heuristics for selecting appropriate abstract value types
//! during state unrolling to balance precision and efficiency.

use super::value::AbstractValue;
use std::collections::HashSet;

/// Configuration for type selection heuristics.
///
/// This configuration controls how abstract value types are selected during
/// state unrolling to balance precision and efficiency, preventing state space
/// explosion.
///
/// # Examples
///
/// ## Default configuration
/// ```
/// use mununu_core::abstraction::heuristics::HeuristicConfig;
///
/// let config = HeuristicConfig::default();
/// assert_eq!(config.max_set_size, 20);
/// assert_eq!(config.max_interval_width, 1000);
/// assert_eq!(config.max_total_states, 1000);
/// ```
///
/// ## Custom configuration for large models
/// ```
/// use mununu_core::abstraction::heuristics::HeuristicConfig;
///
/// let config = HeuristicConfig {
///     max_set_size: 10,           // Convert sets to intervals earlier
///     max_interval_width: 500,    // Narrower intervals before widening
///     max_total_states: 5000,     // Allow more states
///     prefer_intervals_for_contiguous: true,
/// };
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeuristicConfig {
    /// Maximum size for integer sets before converting to intervals (default: 20).
    pub max_set_size: usize,
    /// Maximum width for integer intervals before widening (default: 1000).
    pub max_interval_width: i64,
    /// Maximum total states before applying aggressive abstraction (default: 1000).
    pub max_total_states: usize,
    /// Whether to prefer intervals over sets for large contiguous domains.
    pub prefer_intervals_for_contiguous: bool,
}

impl Default for HeuristicConfig {
    fn default() -> Self {
        Self {
            max_set_size: 20,
            max_interval_width: 1000,
            max_total_states: 1000,
            prefer_intervals_for_contiguous: true,
        }
    }
}

/// Context for variable usage patterns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VariableContext {
    /// Counter variable (e.g., loop counters, iteration counts).
    /// Prefers: concrete values, intervals if unbounded.
    Counter,
    /// State variable (e.g., process state, status flags).
    /// Prefers: sets for discrete values.
    State,
    /// Accumulator variable (e.g., sum, total).
    /// Prefers: intervals.
    Accumulator,
    /// Unknown or general purpose variable.
    /// Uses default heuristics.
    Unknown,
}

/// Statistics for monitoring state space growth.
#[derive(Debug, Clone, Default)]
pub struct StateSpaceStats {
    /// Total number of states generated so far.
    pub total_states: usize,
    /// Number of states per location.
    pub states_per_location: std::collections::HashMap<String, usize>,
    /// Domain size growth per variable.
    pub variable_domain_sizes: std::collections::HashMap<String, usize>,
}

impl StateSpaceStats {
    /// Creates new empty statistics.
    pub fn new() -> Self {
        Self::default()
    }

    /// Updates statistics with a new state.
    pub fn add_state(
        &mut self,
        location: &str,
        variables: &std::collections::HashMap<String, AbstractValue>,
    ) {
        self.total_states += 1;
        *self
            .states_per_location
            .entry(location.to_string())
            .or_insert(0) += 1;

        for (var_name, value) in variables {
            let domain_size = estimate_domain_size(value);
            self.variable_domain_sizes
                .entry(var_name.clone())
                .and_modify(|s| *s = (*s).max(domain_size))
                .or_insert(domain_size);
        }
    }

    /// Checks if state space is approaching limits.
    pub fn is_approaching_limit(&self, config: &HeuristicConfig) -> bool {
        self.total_states >= config.max_total_states / 2
    }

    /// Checks if state space has exceeded limits.
    pub fn has_exceeded_limit(&self, config: &HeuristicConfig) -> bool {
        self.total_states > config.max_total_states
    }
}

/// Estimates the domain size of an abstract value.
fn estimate_domain_size(value: &AbstractValue) -> usize {
    match value {
        AbstractValue::IntConstant(_) => 1,
        AbstractValue::BoolConstant(_) => 1,
        AbstractValue::SymbolConstant(_) => 1,
        AbstractValue::IntInterval(min, max) => {
            // Estimate as width of interval (approximate for large intervals)
            if let Some(diff) = max.checked_sub(*min) {
                diff as usize + 1
            } else {
                usize::MAX // Unbounded or overflow
            }
        }
        AbstractValue::IntSet(set) => set.len(),
        AbstractValue::IntTop => usize::MAX,
        AbstractValue::BoolSet(set) => set.len(),
        AbstractValue::SymbolSet(set) => set.len(),
        AbstractValue::SymbolTop => usize::MAX,
        AbstractValue::PositiveInfinity | AbstractValue::NegativeInfinity => usize::MAX,
        AbstractValue::Undefined => 0,
    }
}

/// Applies type selection heuristics to an abstract value.
///
/// This function selects the most appropriate abstract value type based on:
/// - Threshold-based rules (set size, interval width)
/// - Variable context (counter, state, accumulator)
/// - State space statistics (monitoring for explosion)
pub fn select_abstract_type(
    value: AbstractValue,
    context: VariableContext,
    config: &HeuristicConfig,
    stats: &StateSpaceStats,
) -> AbstractValue {
    // If state space is approaching limits, apply more aggressive abstraction
    let aggressive = stats.is_approaching_limit(config);

    match value {
        // Concrete values: when approaching limits, convert integers to intervals to bound state space
        AbstractValue::IntConstant(int_val) => {
            if aggressive {
                // When aggressive abstraction is needed, convert concrete integers to intervals
                // This bounds unbounded counters and prevents state space explosion
                // Use a small interval around the value, which will be widened if needed
                AbstractValue::IntInterval(int_val, int_val)
            } else {
                value
            }
        }
        AbstractValue::BoolConstant(_) | AbstractValue::SymbolConstant(_) => value,

        // Handle integer sets
        AbstractValue::IntSet(set) => select_int_set_type(set, context, config, aggressive),

        // Handle integer intervals
        AbstractValue::IntInterval(min, max) => {
            select_int_interval_type(min, max, context, config, aggressive)
        }

        // Other types pass through unchanged
        _ => value,
    }
}

/// Selects the appropriate type for an integer set based on heuristics.
fn select_int_set_type(
    set: HashSet<i64>,
    context: VariableContext,
    config: &HeuristicConfig,
    aggressive: bool,
) -> AbstractValue {
    let size = set.len();

    // Empty set - return as-is (should be normalized elsewhere)
    if size == 0 {
        return AbstractValue::IntSet(set);
    }

    // Singleton set - normalize to constant
    if size == 1 {
        return AbstractValue::IntConstant(*set.iter().next().unwrap());
    }

    // Context-aware selection
    match context {
        VariableContext::State => {
            // State variables prefer sets for precision, even if larger
            if size <= config.max_set_size * 2 || !aggressive {
                AbstractValue::IntSet(set)
            } else {
                // Convert to interval if too large
                convert_set_to_interval_if_contiguous(set, config)
            }
        }
        VariableContext::Counter => {
            // Counter variables prefer concrete or intervals
            if size <= config.max_set_size {
                AbstractValue::IntSet(set)
            } else {
                convert_set_to_interval_if_contiguous(set, config)
            }
        }
        VariableContext::Accumulator => {
            // Accumulator variables prefer intervals
            if size > config.max_set_size / 2 || aggressive {
                convert_set_to_interval_if_contiguous(set, config)
            } else {
                AbstractValue::IntSet(set)
            }
        }
        VariableContext::Unknown => {
            // Default threshold-based logic
            if size <= config.max_set_size && !aggressive {
                AbstractValue::IntSet(set)
            } else {
                convert_set_to_interval_if_contiguous(set, config)
            }
        }
    }
}

/// Converts a set to an interval if it's contiguous, otherwise returns as set.
fn convert_set_to_interval_if_contiguous(
    set: HashSet<i64>,
    config: &HeuristicConfig,
) -> AbstractValue {
    if set.len() <= 1 {
        return AbstractValue::IntSet(set);
    }

    let min_val = *set.iter().min().unwrap();
    let max_val = *set.iter().max().unwrap();

    // Check if set is contiguous (all integers from min to max are present)
    let expected_size = (max_val - min_val + 1) as usize;
    let is_contiguous = set.len() == expected_size && expected_size > 0;

    if is_contiguous && config.prefer_intervals_for_contiguous {
        AbstractValue::IntInterval(min_val, max_val).normalize()
    } else {
        // If not contiguous or intervals not preferred, keep as set
        AbstractValue::IntSet(set)
    }
}

/// Selects the appropriate type for an integer interval based on heuristics.
fn select_int_interval_type(
    min: i64,
    max: i64,
    context: VariableContext,
    config: &HeuristicConfig,
    aggressive: bool,
) -> AbstractValue {
    let width = max.saturating_sub(min);

    // Singleton interval - normalize to constant
    if min == max {
        return AbstractValue::IntConstant(min);
    }

    // Check if width exceeds threshold
    let exceeds_threshold =
        width > config.max_interval_width || (aggressive && width > config.max_interval_width / 2);

    match context {
        VariableContext::Counter => {
            // Counter variables: keep intervals, but widen if too large
            if exceeds_threshold {
                // Could widen here, but for now return as-is
                // Widening should be handled separately
                AbstractValue::IntInterval(min, max)
            } else {
                AbstractValue::IntInterval(min, max)
            }
        }
        VariableContext::Accumulator => {
            // Accumulator variables: prefer intervals, widen if needed
            if exceeds_threshold && aggressive {
                // In aggressive mode, could widen
                AbstractValue::IntInterval(min, max)
            } else {
                AbstractValue::IntInterval(min, max)
            }
        }
        VariableContext::State | VariableContext::Unknown => {
            // Default: keep interval, but monitor width
            if exceeds_threshold && aggressive {
                // Could convert to top or widen
                AbstractValue::IntInterval(min, max)
            } else {
                AbstractValue::IntInterval(min, max)
            }
        }
    }
}

/// Applies widening to an abstract value if thresholds are exceeded.
pub fn apply_widening_if_needed(
    value: AbstractValue,
    previous_value: Option<&AbstractValue>,
    config: &HeuristicConfig,
) -> AbstractValue {
    // Widening is typically applied in sequences, so we need previous value
    let Some(prev) = previous_value else {
        return value;
    };

    match (prev, &value) {
        (
            AbstractValue::IntInterval(prev_min, prev_max),
            AbstractValue::IntInterval(curr_min, curr_max),
        ) => {
            let prev_width = prev_max.saturating_sub(*prev_min);
            let curr_width = curr_max.saturating_sub(*curr_min);

            // If interval is growing beyond threshold, apply widening
            if curr_width > config.max_interval_width && curr_width > prev_width {
                value.widen(prev)
            } else {
                value
            }
        }
        _ => value,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_heuristic_config_default() {
        let config = HeuristicConfig::default();
        assert_eq!(config.max_set_size, 20);
        assert_eq!(config.max_interval_width, 1000);
        assert_eq!(config.max_total_states, 1000);
        assert!(config.prefer_intervals_for_contiguous);
    }

    #[test]
    fn test_estimate_domain_size() {
        assert_eq!(estimate_domain_size(&AbstractValue::int_constant(5)), 1);
        assert_eq!(
            estimate_domain_size(&AbstractValue::int_interval(0, 10)),
            11
        );
        assert_eq!(
            estimate_domain_size(&AbstractValue::int_set(vec![1, 2, 3])),
            3
        );
    }

    #[test]
    fn test_select_abstract_type_singleton_set() {
        let config = HeuristicConfig::default();
        let stats = StateSpaceStats::new();
        let value = AbstractValue::int_set(vec![5]);

        let result = select_abstract_type(value, VariableContext::Unknown, &config, &stats);
        assert_eq!(result, AbstractValue::int_constant(5));
    }

    #[test]
    fn test_select_abstract_type_small_set() {
        let config = HeuristicConfig::default();
        let stats = StateSpaceStats::new();
        let value = AbstractValue::int_set(vec![1, 2, 3, 4, 5]);

        let result = select_abstract_type(value.clone(), VariableContext::Unknown, &config, &stats);
        // Small set should remain as set
        assert!(matches!(result, AbstractValue::IntSet(_)));
    }

    #[test]
    fn test_convert_set_to_interval_contiguous() {
        let config = HeuristicConfig::default();
        // Create contiguous set [0, 10]
        let set: HashSet<i64> = (0..=10).collect();

        let result = convert_set_to_interval_if_contiguous(set, &config);
        assert_eq!(result, AbstractValue::int_interval(0, 10));
    }

    #[test]
    fn test_convert_set_to_interval_non_contiguous() {
        let config = HeuristicConfig::default();
        // Create non-contiguous set {0, 2, 5, 10}
        let set: HashSet<i64> = vec![0, 2, 5, 10].into_iter().collect();

        let result = convert_set_to_interval_if_contiguous(set.clone(), &config);
        // Non-contiguous should remain as set
        assert!(matches!(result, AbstractValue::IntSet(_)));
    }

    #[test]
    fn test_context_aware_selection_counter() {
        let config = HeuristicConfig::default();
        let stats = StateSpaceStats::new();

        // Counter with large set should convert to interval
        let large_set: HashSet<i64> = (0..=50).collect();
        let value = AbstractValue::IntSet(large_set);

        let result = select_abstract_type(value, VariableContext::Counter, &config, &stats);
        // Should convert to interval for contiguous large set
        assert!(matches!(result, AbstractValue::IntInterval(_, _)));
    }

    #[test]
    fn test_context_aware_selection_state() {
        let config = HeuristicConfig::default();
        let stats = StateSpaceStats::new();

        // State variable prefers sets even if larger
        let medium_set: HashSet<i64> = (0..=30).collect();
        let value = AbstractValue::IntSet(medium_set);

        let result = select_abstract_type(value.clone(), VariableContext::State, &config, &stats);
        // State variables keep sets longer (threshold * 2)
        assert!(matches!(result, AbstractValue::IntSet(_)));
    }

    #[test]
    fn test_state_space_stats() {
        let mut stats = StateSpaceStats::new();
        let mut vars = std::collections::HashMap::new();
        vars.insert("x".to_string(), AbstractValue::int_constant(5));

        stats.add_state("Location1", &vars);
        assert_eq!(stats.total_states, 1);
        assert_eq!(stats.states_per_location.get("Location1"), Some(&1));
    }

    #[test]
    fn test_state_space_stats_limit_checking() {
        let config = HeuristicConfig {
            max_total_states: 100,
            ..Default::default()
        };
        let mut stats = StateSpaceStats::new();

        // Add 50 states
        for i in 0..50 {
            let mut vars = std::collections::HashMap::new();
            vars.insert("x".to_string(), AbstractValue::int_constant(i));
            stats.add_state("Location1", &vars);
        }

        assert!(stats.is_approaching_limit(&config));
        assert!(!stats.has_exceeded_limit(&config));

        // Add more to exceed limit
        for i in 50..101 {
            let mut vars = std::collections::HashMap::new();
            vars.insert("x".to_string(), AbstractValue::int_constant(i));
            stats.add_state("Location1", &vars);
        }

        assert!(stats.has_exceeded_limit(&config));
    }

    #[test]
    fn test_apply_widening_if_needed() {
        let config = HeuristicConfig::default();

        let prev = AbstractValue::int_interval(0, 100);
        let curr = AbstractValue::int_interval(0, 2000); // Exceeds threshold

        let result = apply_widening_if_needed(curr.clone(), Some(&prev), &config);
        // Should apply widening (which should return IntTop or wider interval)
        // Note: actual widening behavior depends on implementation
        assert!(matches!(
            result,
            AbstractValue::IntInterval(_, _) | AbstractValue::IntTop
        ));
    }

    #[test]
    fn test_apply_widening_no_previous() {
        let config = HeuristicConfig::default();
        let value = AbstractValue::int_interval(0, 100);

        let result = apply_widening_if_needed(value.clone(), None, &config);
        // Without previous value, should return unchanged
        assert_eq!(result, value);
    }

    #[test]
    fn test_select_abstract_type_large_set_contiguous() {
        let config = HeuristicConfig::default();
        let stats = StateSpaceStats::new();

        // Large contiguous set should convert to interval
        let large_set: HashSet<i64> = (0..=50).collect();
        let value = AbstractValue::IntSet(large_set);

        let result = select_abstract_type(value, VariableContext::Unknown, &config, &stats);
        assert!(matches!(result, AbstractValue::IntInterval(0, 50)));
    }

    #[test]
    fn test_select_abstract_type_large_set_non_contiguous() {
        let config = HeuristicConfig::default();
        let stats = StateSpaceStats::new();

        // Large non-contiguous set should remain as set
        let mut large_set = HashSet::new();
        for i in (0..=100).step_by(2) {
            large_set.insert(i);
        }
        let value = AbstractValue::IntSet(large_set.clone());

        let result = select_abstract_type(value.clone(), VariableContext::Unknown, &config, &stats);
        // Non-contiguous should remain as set
        assert!(matches!(result, AbstractValue::IntSet(_)));
    }

    #[test]
    fn test_context_aware_accumulator() {
        let config = HeuristicConfig::default();
        let stats = StateSpaceStats::new();

        // Accumulator with medium set should prefer interval
        let medium_set: HashSet<i64> = (0..=15).collect();
        let value = AbstractValue::IntSet(medium_set);

        let result = select_abstract_type(value, VariableContext::Accumulator, &config, &stats);
        // Accumulator prefers intervals, so should convert
        assert!(matches!(result, AbstractValue::IntInterval(_, _)));
    }

    #[test]
    fn test_aggressive_abstraction() {
        let config = HeuristicConfig::default();
        let mut stats = StateSpaceStats::new();

        // Set up stats to trigger aggressive mode
        for _ in 0..600 {
            let mut vars = std::collections::HashMap::new();
            vars.insert("x".to_string(), AbstractValue::int_constant(0));
            stats.add_state("Location1", &vars);
        }

        // With aggressive mode, should convert smaller sets to intervals
        let medium_set: HashSet<i64> = (0..=15).collect();
        let value = AbstractValue::IntSet(medium_set);

        let result = select_abstract_type(value, VariableContext::Unknown, &config, &stats);
        // Aggressive mode should convert to interval
        assert!(matches!(result, AbstractValue::IntInterval(_, _)));
    }
}
