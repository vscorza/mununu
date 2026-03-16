## Abstraction and State Unrolling

This document describes the abstraction layer used by Henos to handle state
variables, dynamic guards, and effects during unrolling. It is intended for
contributors who want to understand or extend the implementation in
`src/abstraction/*`.

The main goals are:

- Bound the state-space explosion that comes from variables and dynamic guards.
- Keep the μ-calculus and CLTS layers independent from concrete variable types.
- Provide enough structure (constraints, domains, heuristics) to make
  performance tuning and future extensions predictable.

The core components live in:

- `abstraction::value` – abstract domains (`AbstractValue`).
- `abstraction::state` – abstract states and naming.
- `abstraction::constraint` / `abstraction::constraints` – constraint
  representation and management (`Constraint`, `ConstraintManager`).
- `abstraction::expression` / `abstraction::evaluator` – expression and guard
  evaluation (`ExpressionEvaluator`).
- `abstraction::refinement` – splitting states when a guard is `Maybe`.
- `abstraction::unrolling` – main unrolling pipeline and heuristics.

---

## Abstract Domains (`AbstractValue`)

Variables are not stored as concrete values directly. Instead, the
`AbstractValue` type represents different abstract domains:

- **Boolean domain**
  - `BoolConstant(bool)`
  - `BoolSet(HashSet<bool>)`
- **Integer domain**
  - `IntConstant(i64)`
  - `IntInterval(i64, i64)` – closed interval \([min, max]\)
  - `IntSet(HashSet<i64>)`
  - `IntTop` – any integer value
  - `PositiveInfinity`, `NegativeInfinity`
- **Symbol domain**
  - `SymbolConstant(String)`
  - `SymbolSet(HashSet<String>)`
  - `SymbolTop`
- **Special**
  - `Undefined` – uninitialised or unknown

The domain supports:

- Normalisation (e.g. collapsing singleton intervals into `IntConstant`).
- Arithmetic (`+`, `-`, `*`, `/`) with error reporting (type mismatch, division
  by zero, etc.).
- Lattice-style operations (join/meet, widening/narrowing) implemented in the
  `value` and `heuristics` modules.

The `ValueOperations` trait (in `abstraction::operations`) centralises checked
arithmetic:

- `normalize_value(self) -> AbstractValue`
- `add_checked(self, other: AbstractValue) -> Result<AbstractValue, EvaluationError>`
- `sub_checked`, `mul_checked`, `div_checked`

This keeps the evaluator and unrolling pipeline free from low-level arithmetic
details and makes it easier to evolve the domains.

---

## Abstract States and Constraints

An `AbstractState` (see `abstraction::state`) combines:

- `location: String` – the original CLTS state name (e.g. `"Executing"`).
- `variables: HashMap<String, AbstractValue>` – the current abstract value of
  each declared variable.
- `constraints: ConstraintManager` – global constraints that must hold whenever
  this abstract state is considered reachable.

The `Constraint` type (in `abstraction::constraint`) captures common patterns:

- Variable–constant constraints: `x op c`
- Variable–variable constraints: `x op y`
- More complex arithmetic expressions.

`ConstraintManager` (in `abstraction::constraints`) is a thin wrapper around
`Vec<Constraint>` that:

- Stores constraints attached to an `AbstractState`.
- Evaluates them via `ExpressionEvaluator`, returning:
  - `Ok(true)` if all constraints are satisfied or inconclusive,
  - `Ok(false)` if any constraint is definitely violated,
  - `Err(...)` for non-trivial evaluation errors.
- Supports refinement via `refine(&mut self, new_constraints: &[Constraint])`.

Abstract states provide:

- `set_variable` / `get_variable` – updates and queries on the variable map.
- `add_constraint` – adds a new `Constraint` to the internal manager.
- `state_name()` – a stable, human-readable name that combines `location` and
  a compact encoding of variable values (used during unrolling and for CLTS
  state naming).

---

## Expression and Guard Evaluation

Expression and guard evaluation is centralised in
`abstraction::evaluator::ExpressionEvaluator`:

- `ExpressionEvaluator::new(state: &AbstractState, predicates: &HashMap<String, bool>)`
  binds an evaluator to a specific abstract state and a predicate environment.
- `evaluate(&self, expr: &Expr) -> Result<AbstractValue, EvaluationError>` computes
  the abstract value of an arithmetic or symbolic expression.
- `evaluate_guard(&self, guard: &GuardExpr) -> Result<GuardResult, EvaluationError>`
  evaluates a guard to one of:
  - `GuardResult::AlwaysTrue`
  - `GuardResult::AlwaysFalse`
  - `GuardResult::Maybe`

The evaluator uses `ValueOperations` for arithmetic to ensure that:

- Domain-specific rules (e.g. interval arithmetic, set arithmetic) are applied
  consistently.
- Type errors and loss of precision are surfaced as `EvaluationError` instead of
  silently producing incorrect domains.

---

## Refinement and Constraint-Based Splitting

When `evaluate_guard` returns `GuardResult::Maybe`, the abstraction cannot decide
if a transition is enabled or not. In that case, the unrolling pipeline asks
`abstraction::refinement` to split the current abstract state into several
more precise states.

- `refine_state_with_guard(state: &AbstractState, guard: &GuardExpr) -> Vec<AbstractState>`
  implements this logic:
  - Simple boolean structure is handled recursively:
    - `And` – refine using the left, then refine each result using the right.
    - `Or` – refine with each side; both branches are kept for now.
    - `Not` – negate and refine.
  - `Predicate` guards are not refined (they stay as-is).
  - `Comparison` guards delegate to `refine_comparison`.

The current implementation focuses on the common case of **variable–constant**
comparisons with integer intervals:

- `refine_comparison` recognises `Expr::Var` vs `Expr::Const` and current
  `IntInterval(min, max)` values.
- `refine_interval_comparison` then:
  - Splits the interval into sub-intervals that make the comparison definitely
    true or definitely false.
  - For each resulting branch, updates the variable interval and *adds a
    `Constraint`* describing the branch:
    - Example for `x > c`:
      - `[min, c]` branch: guard is false; add constraint `x <= c`.
      - `[c+1, max]` branch: guard is true; add constraint `x > c`.

By attaching explicit constraints to each branch (via `ConstraintManager`),
downstream analyses (or future passes) can reason about the feasible valuations
in each branch without re-deriving them from the intervals alone.

---

## Unrolling Pipeline

The main entry point for unrolling is `abstraction::unrolling::unroll_states`,
which is implemented in terms of an internal `UnrollingPipeline` helper.
Conceptually, the pipeline works as follows:

1. **Initial abstract states**
   - Construct `AbstractState` instances from the original CLTS states and
     variable declarations (initial values, top elements where needed).
2. **Worklist exploration**
   - Maintain a queue of abstract states to process.
   - For each state, consider outgoing CLTS transitions together with their
     guards and effects.
3. **Guard evaluation**
   - Use `ExpressionEvaluator::evaluate_guard`:
     - `AlwaysTrue` – transition is kept; effects are applied to produce a new
       abstract state.
     - `AlwaysFalse` – transition is discarded.
     - `Maybe` – call `refine_state_with_guard` and enqueue refined states.
4. **Effect application**
   - Use `ExpressionEvaluator::evaluate` to compute right-hand sides and update
     variable values in the target abstract state.
5. **Heuristics and limits**
   - Apply normalisation and domain selection heuristics (e.g. converting large
     intervals into coarser representations) to control state-space growth.
   - Use widening/thresholds to avoid infinite ascending chains when necessary.
6. **Materialisation**
   - Once exploration stabilises (or hits limits), materialise the resulting
     abstract states as concrete CLTS states, using `AbstractState::state_name`
     for names and translating guards/effects into static structure where
     possible.

The unrolling pipeline is invoked from higher layers (primarily
`context_dsl::realize`) when:

- An automaton declares variables, **and**
- There are guards or effects that reference those variables (dynamic guards).

Predicate-only guards (e.g. named boolean conditions evaluated in the
environment) do **not** trigger unrolling.

---

## Integration with the DSL and CLI

The abstraction layer is wired into the Context DSL realization pipeline:

- `context_dsl::realize` decides, per automaton, whether unrolling is required
  based on variables and dynamic guards.
- When required, it hands the automaton and variable declarations to
  `abstraction::unrolling::unroll_states`.
- The resulting CLTS uses unrolled state names (e.g. `Executing_x_5`) and no
  longer depends on variable evaluation at runtime.

Sidecar DSL files and predicates interact with unrolled states via pattern
matching (see `context_dsl::state_matching::StateNameMatcher`), so structural
predicates remain stable under unrolling.

On the CLI side, nothing special is required: the `henos context ...` and
`henos translate ...` commands simply operate on the realized CLTS, which may
have been produced via unrolling.

---

## Extending the Abstraction Layer

When extending or adjusting the abstraction layer, typical changes fall into one
of the following categories:

- **New domains or value variants**
  - Add a new `AbstractValue` variant and update:
    - `ValueOperations` (for arithmetic support, if applicable).
    - `ExpressionEvaluator` (for expression/guard evaluation).
    - `AbstractState::state_name` (for debug-friendly naming).
- **Richer constraints**
  - Add new constructors or kinds in `Constraint` / `ConstraintKind`.
  - Update `ConstraintManager::evaluate` if the semantics differ from the
    existing comparison-based interpretation.
- **Refinement strategies**
  - Extend `refine_state_with_guard` and `refine_comparison` to handle more
    complex guards (e.g. multi-variable comparisons, non-linear arithmetic).
  - Optionally attach richer constraints per branch so the information is not
    lost.
- **Heuristics and performance**
  - Tune `unrolling` and `heuristics` thresholds (e.g. maximum state count,
    widening triggers).
  - Profile common translation workloads and adjust the abstract domains /
    splitting granularity to balance precision and runtime.

For a high-level introduction and workflow examples, see the “State Variable
Abstraction” section in `README.md`. This document focuses on the internal
implementation details and how the pieces fit together.


