# LTL Support Implementation Plan

## Overview

This document outlines the implementation plan for adding LTL (Linear Temporal Logic) support to the Context DSL syntax. LTL formulas will be translated to μ-calculus in an intermediate step, allowing the existing core functionality to remain untouched.

**Key Principles:**
- LTL and μ-calculus can coexist in the same DSL
- LTL → μ-calculus translation happens during realization
- Core μ-calculus evaluator remains unchanged
- Comprehensive test coverage for all LTL operators and patterns

---

## Architecture

### High-Level Flow

```
Context DSL Source
    ↓
Parser (context_dsl::parser)
    ↓
AST (MuFormula with LtlExpr or MuExpr)
    ↓
Realization (context_dsl::realize)
    ↓
LTL → μ-calculus Translation (new module: ltl_translator)
    ↓
μ-calculus Formula (existing structure)
    ↓
Existing μ-calculus Evaluator (unchanged)
```

### Design Decisions

1. **Unified Formula Section**: Keep `mu_formulas` section, but allow both LTL and μ-calculus syntax
2. **Syntax Detection**: Parser detects LTL syntax (keywords like `G`, `F`, `X`, `U`) vs μ-calculus syntax
3. **AST Representation**: Extend `MuExpr` to `FormulaExpr` enum supporting both LTL and μ-calculus
4. **Translation Point**: Translate LTL to μ-calculus during `realize()` before passing to evaluator
5. **Backward Compatibility**: Existing μ-calculus formulas continue to work unchanged

---

## File Structure

### New Files

```
src/ltl/
├── mod.rs                    # Module exports
├── ast.rs                    # LTL AST representation
├── parser.rs                 # LTL formula parser
├── translator.rs             # LTL → μ-calculus translation
└── tests.rs                  # Unit tests for LTL parsing and translation
```

### Modified Files

```
src/context_dsl/
├── ast.rs                    # Add FormulaExpr enum, update MuFormula
├── parser.rs                 # Add LTL parsing logic
├── realize.rs                # Add LTL translation step
└── token.rs                  # Add LTL keywords (G, F, X, U, W, R)
```

---

## Implementation Phases

### Phase 1: LTL AST and Basic Infrastructure (Week 1)

#### 1.1 Create LTL Module Structure

**File:** `src/ltl/mod.rs`
- Module exports
- Public API surface

**File:** `src/ltl/ast.rs`
- Define `LtlFormula` enum with all LTL operators:
  ```rust
  pub enum LtlFormula {
      // Atomic
      True,
      False,
      Predicate(String),
      
      // Propositional
      Not(Box<LtlFormula>),
      And(Box<LtlFormula>, Box<LtlFormula>),
      Or(Box<LtlFormula>, Box<LtlFormula>),
      Implies(Box<LtlFormula>, Box<LtlFormula>),
      
      // Temporal (basic)
      Next(Box<LtlFormula>),           // X φ
      Always(Box<LtlFormula>),          // G φ
      Eventually(Box<LtlFormula>),      // F φ
      Until {                            // φ U ψ
          left: Box<LtlFormula>,
          right: Box<LtlFormula>,
      },
      WeakUntil {                        // φ W ψ
          left: Box<LtlFormula>,
          right: Box<LtlFormula>,
      },
      Release {                          // φ R ψ
          left: Box<LtlFormula>,
          right: Box<LtlFormula>,
      },
      
      // Derived patterns (for convenience)
      Recurrence(Box<LtlFormula>),      // GF φ
      Stabilization(Box<LtlFormula>),   // FG φ
      Response {                         // G(φ → F(ψ))
          trigger: Box<LtlFormula>,
          response: Box<LtlFormula>,
      },
  }
  ```

**Tests:**
- `test_ltl_ast_creation` - Verify AST nodes can be created
- `test_ltl_ast_debug` - Verify Debug implementation

#### 1.2 Update Context DSL AST

**File:** `src/context_dsl/ast.rs`

**Changes:**
```rust
// Replace MuExpr with FormulaExpr
#[derive(Debug, Clone)]
pub enum FormulaExpr {
    MuCalculus(MuExpr),      // Existing μ-calculus syntax
    Ltl(LtlExpr),            // New LTL syntax
}

#[derive(Debug, Clone)]
pub struct LtlExpr {
    pub formula: ltl::ast::LtlFormula,
    pub span: Span,
}

// Update MuFormula
#[derive(Debug, Clone)]
pub struct MuFormula {
    pub name: Ident,
    pub meta: Meta,
    pub targets: FormulaTargets,
    pub body: FormulaExpr,  // Changed from MuExpr
}
```

**Tests:**
- `test_formula_expr_enum` - Verify FormulaExpr works
- `test_mu_formula_with_ltl` - Verify MuFormula accepts LTL

---

### Phase 2: LTL Parser (Week 1-2)

#### 2.1 Add LTL Keywords to Tokenizer

**File:** `src/context_dsl/token.rs`

**Changes:**
```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Keyword {
    // ... existing keywords ...
    // LTL operators
    Always,      // G
    Eventually,  // F
    Next,        // X
    Until,       // U
    WeakUntil,   // W
    Release,     // R
}

impl Keyword {
    pub fn from_ident(ident: &str) -> Option<Self> {
        use Keyword::*;
        Some(match ident {
            // ... existing matches ...
            "G" | "always" => Always,
            "F" | "eventually" => Eventually,
            "X" | "next" => Next,
            "U" | "until" => Until,
            "W" | "weak_until" => WeakUntil,
            "R" | "release" => Release,
            _ => return None,
        })
    }
}
```

**Tests:**
- `test_ltl_keyword_parsing` - Verify keywords are recognized
- `test_ltl_keyword_case_insensitive` - Verify case handling

#### 2.2 Implement LTL Parser

**File:** `src/ltl/parser.rs`

**Implementation:**
- Recursive descent parser for LTL syntax
- Operator precedence: `!` > `G`, `F`, `X` > `U`, `W`, `R` > `&&` > `||` > `->`
- Support parentheses for grouping
- Support both prefix (`G(φ)`) and infix (`φ U ψ`) syntax

**Parser Structure:**
```rust
pub fn parse(input: &str) -> Result<LtlFormula, ParseError> {
    let mut parser = LtlParser::new(input);
    let formula = parser.parse_formula()?;
    parser.expect_eof()?;
    Ok(formula)
}

impl LtlParser {
    fn parse_formula(&mut self) -> Result<LtlFormula, ParseError> {
        self.parse_implies()
    }
    
    fn parse_implies(&mut self) -> Result<LtlFormula, ParseError> {
        // φ -> ψ
    }
    
    fn parse_or(&mut self) -> Result<LtlFormula, ParseError> {
        // φ || ψ
    }
    
    fn parse_and(&mut self) -> Result<LtlFormula, ParseError> {
        // φ && ψ
    }
    
    fn parse_unary(&mut self) -> Result<LtlFormula, ParseError> {
        // !φ, G φ, F φ, X φ
    }
    
    fn parse_until(&mut self) -> Result<LtlFormula, ParseError> {
        // φ U ψ, φ W ψ, φ R ψ
    }
    
    fn parse_primary(&mut self) -> Result<LtlFormula, ParseError> {
        // true, false, predicate, (φ)
    }
}
```

**Syntax Examples:**
```rust
// Basic operators
"G safe"                    // Always safe
"F completed"                // Eventually completed
"X alarm"                    // Next alarm
"request U grant"           // Request until grant

// Propositional
"G (!deadlock)"              // Always not deadlock
"G (safe && bounded)"        // Always (safe and bounded)
"request -> F grant"         // Request implies eventually grant

// Nested
"G (request -> F grant)"     // Always (request implies eventually grant)
"F G idle"                   // Eventually always idle
"G F heartbeat"             // Always eventually heartbeat

// Parentheses
"G ((a && b) || c)"         // Always ((a and b) or c)
```

**Tests:**
- `test_parse_always` - Parse `G(φ)` and `G φ`
- `test_parse_eventually` - Parse `F(φ)` and `F φ`
- `test_parse_next` - Parse `X(φ)` and `X φ`
- `test_parse_until` - Parse `φ U ψ`
- `test_parse_weak_until` - Parse `φ W ψ`
- `test_parse_release` - Parse `φ R ψ`
- `test_parse_not` - Parse `!φ` and `G !φ`
- `test_parse_and` - Parse `φ && ψ`
- `test_parse_or` - Parse `φ || ψ`
- `test_parse_implies` - Parse `φ -> ψ`
- `test_parse_precedence` - Verify operator precedence
- `test_parse_parentheses` - Verify parentheses grouping
- `test_parse_nested` - Parse nested formulas
- `test_parse_errors` - Test error handling (unclosed parens, unexpected tokens)

---

### Phase 3: LTL → μ-Calculus Translator (Week 2)

#### 3.1 Implement Translation Engine

**File:** `src/ltl/translator.rs`

**Implementation:**
```rust
use crate::mu_calculus::{Formula, FormulaBuilder, Node, NodeId};

pub fn translate(ltl: &LtlFormula) -> Result<Formula, TranslationError> {
    let mut translator = Translator::new();
    let root = translator.translate_formula(ltl)?;
    Ok(Formula::new(root, translator.builder.nodes, translator.builder.vars))
}

struct Translator {
    builder: FormulaBuilder,
    var_counter: usize,
}

impl Translator {
    fn translate_formula(&mut self, ltl: &LtlFormula) -> Result<NodeId, TranslationError> {
        match ltl {
            LtlFormula::True => Ok(self.builder.push_node(Node::True)),
            LtlFormula::False => Ok(self.builder.push_node(Node::False)),
            LtlFormula::Predicate(name) => Ok(self.builder.push_node(Node::Predicate(name.clone()))),
            
            LtlFormula::Not(inner) => {
                let inner_id = self.translate_formula(inner)?;
                Ok(self.builder.push_node(Node::Not(inner_id)))
            },
            
            LtlFormula::And(left, right) => {
                let left_id = self.translate_formula(left)?;
                let right_id = self.translate_formula(right)?;
                Ok(self.builder.push_node(Node::And(left_id, right_id)))
            },
            
            LtlFormula::Or(left, right) => {
                let left_id = self.translate_formula(left)?;
                let right_id = self.translate_formula(right)?;
                Ok(self.builder.push_node(Node::Or(left_id, right_id)))
            },
            
            LtlFormula::Implies(left, right) => {
                // φ -> ψ = !φ || ψ
                let left_id = self.translate_formula(left)?;
                let right_id = self.translate_formula(right)?;
                let not_left = self.builder.push_node(Node::Not(left_id));
                Ok(self.builder.push_node(Node::Or(not_left, right_id)))
            },
            
            LtlFormula::Next(inner) => {
                // X φ = [] φ
                let inner_id = self.translate_formula(inner)?;
                Ok(self.builder.push_modal(
                    ModalKind::Box,
                    Guard::default(),
                    inner_id
                ))
            },
            
            LtlFormula::Always(inner) => {
                // G φ = ν X. (φ ∧ [] X)
                let inner_id = self.translate_formula(inner)?;
                let var_id = self.new_fixpoint_var("X");
                let var_node = self.builder.push_node(Node::Variable(var_id));
                let box_var = self.builder.push_modal(ModalKind::Box, Guard::default(), var_node);
                let and_node = self.builder.push_node(Node::And(inner_id, box_var));
                Ok(self.builder.push_node(Node::Nu { var: var_id, body: and_node }))
            },
            
            LtlFormula::Eventually(inner) => {
                // F φ = μ X. (φ ∨ [] X)
                let inner_id = self.translate_formula(inner)?;
                let var_id = self.new_fixpoint_var("X");
                let var_node = self.builder.push_node(Node::Variable(var_id));
                let box_var = self.builder.push_modal(ModalKind::Box, Guard::default(), var_node);
                let or_node = self.builder.push_node(Node::Or(inner_id, box_var));
                Ok(self.builder.push_node(Node::Mu { var: var_id, body: or_node }))
            },
            
            LtlFormula::Until { left, right } => {
                // φ U ψ = μ X. (ψ ∨ (φ ∧ [] X))
                let left_id = self.translate_formula(left)?;
                let right_id = self.translate_formula(right)?;
                let var_id = self.new_fixpoint_var("X");
                let var_node = self.builder.push_node(Node::Variable(var_id));
                let box_var = self.builder.push_modal(ModalKind::Box, Guard::default(), var_node);
                let and_left = self.builder.push_node(Node::And(left_id, box_var));
                let or_node = self.builder.push_node(Node::Or(right_id, and_left));
                Ok(self.builder.push_node(Node::Mu { var: var_id, body: or_node }))
            },
            
            LtlFormula::WeakUntil { left, right } => {
                // φ W ψ = (φ U ψ) ∨ G φ = μ X. (ψ ∨ (φ ∧ [] X)) ∨ (ν Y. (φ ∧ [] Y))
                let until = self.translate_until(left, right)?;
                let always_left = self.translate_always(left)?;
                Ok(self.builder.push_node(Node::Or(until, always_left)))
            },
            
            LtlFormula::Release { left, right } => {
                // φ R ψ = !(!φ U !ψ) = !(μ X. (!ψ ∨ (!φ ∧ [] X)))
                let not_left = self.translate_not(left)?;
                let not_right = self.translate_not(right)?;
                let until = self.translate_until(&not_left, &not_right)?;
                Ok(self.builder.push_node(Node::Not(until)))
            },
            
            LtlFormula::Recurrence(inner) => {
                // GF φ = G F φ = ν Y. (μ X. (φ ∨ [] X) ∧ [] Y)
                let eventually = self.translate_eventually(inner)?;
                let var_id = self.new_fixpoint_var("Y");
                let var_node = self.builder.push_node(Node::Variable(var_id));
                let box_var = self.builder.push_modal(ModalKind::Box, Guard::default(), var_node);
                let and_node = self.builder.push_node(Node::And(eventually, box_var));
                Ok(self.builder.push_node(Node::Nu { var: var_id, body: and_node }))
            },
            
            LtlFormula::Stabilization(inner) => {
                // FG φ = F G φ = μ Y. (ν X. (φ ∧ [] X) ∨ [] Y)
                let always = self.translate_always(inner)?;
                let var_id = self.new_fixpoint_var("Y");
                let var_node = self.builder.push_node(Node::Variable(var_id));
                let box_var = self.builder.push_modal(ModalKind::Box, Guard::default(), var_node);
                let or_node = self.builder.push_node(Node::Or(always, box_var));
                Ok(self.builder.push_node(Node::Mu { var: var_id, body: or_node }))
            },
            
            LtlFormula::Response { trigger, response } => {
                // G(φ → F(ψ)) = ν X. ((!φ ∨ μ Y. (ψ ∨ [] Y)) ∧ [] X)
                let trigger_id = self.translate_formula(trigger)?;
                let response_id = self.translate_formula(response)?;
                let not_trigger = self.builder.push_node(Node::Not(trigger_id));
                let eventually_response = self.translate_eventually(response)?;
                let or_node = self.builder.push_node(Node::Or(not_trigger, eventually_response));
                let var_id = self.new_fixpoint_var("X");
                let var_node = self.builder.push_node(Node::Variable(var_id));
                let box_var = self.builder.push_modal(ModalKind::Box, Guard::default(), var_node);
                let and_node = self.builder.push_node(Node::And(or_node, box_var));
                Ok(self.builder.push_node(Node::Nu { var: var_id, body: and_node }))
            },
        }
    }
    
    fn new_fixpoint_var(&mut self, prefix: &str) -> FormulaVarId {
        let name = format!("{}{}", prefix, self.var_counter);
        self.var_counter += 1;
        self.builder.push_var(name)
    }
}
```

**Tests:**
- `test_translate_true` - `true` → `true`
- `test_translate_false` - `false` → `false`
- `test_translate_predicate` - `p` → `p`
- `test_translate_not` - `!φ` → `!φ`
- `test_translate_and` - `φ && ψ` → `φ && ψ`
- `test_translate_or` - `φ || ψ` → `φ || ψ`
- `test_translate_implies` - `φ -> ψ` → `!φ || ψ`
- `test_translate_next` - `X φ` → `[] φ`
- `test_translate_always` - `G φ` → `ν X. (φ ∧ [] X)`
- `test_translate_eventually` - `F φ` → `μ X. (φ ∨ [] X)`
- `test_translate_until` - `φ U ψ` → `μ X. (ψ ∨ (φ ∧ [] X))`
- `test_translate_weak_until` - `φ W ψ` → `(φ U ψ) || G φ`
- `test_translate_release` - `φ R ψ` → `!(!φ U !ψ)`
- `test_translate_recurrence` - `GF φ` → `ν Y. (μ X. (φ ∨ [] X) ∧ [] Y)`
- `test_translate_stabilization` - `FG φ` → `μ Y. (ν X. (φ ∧ [] X) ∨ [] Y)`
- `test_translate_response` - `G(φ -> F(ψ))` → `ν X. ((!φ ∨ μ Y. (ψ ∨ [] Y)) ∧ [] X)`
- `test_translate_nested` - Complex nested formulas
- `test_translate_fixpoint_names` - Verify unique fixpoint variable names

---

### Phase 3.2: LTL to μ-Calculus Translation Reference

This section provides the complete translation patterns from `docs/ltl_templates/ai_ltl_to_mu_cheatsheet.json`. These patterns serve as the authoritative reference for implementing the LTL → μ-calculus translator.

**Important Note:** All modalities (`[]` and `<>`) operate on the NEXT state only. Use fixpoints for temporal properties.

#### Basic Temporal Operators

| LTL Pattern | μ-Calculus Translation | Description |
|------------|------------------------|-------------|
| `X φ` | `[] φ` | In the next step, φ holds |
| `G φ` | `ν X. (φ ∧ [] X)` | Always φ (globally) |
| `F φ` | `μ X. (φ ∨ [] X)` | Eventually φ (finally) |
| `φ U ψ` | `μ X. (ψ ∨ (φ ∧ [] X))` | φ holds until ψ happens (and ψ eventually happens) |

**Examples:**
- `X alarm` → `[] alarm` - Alarm is active in the next state
- `G safe` → `ν X. (safe ∧ [] X)` - Safe condition always holds
- `F completed` → `μ X. (completed ∨ [] X)` - Completion state is eventually reached
- `request U grant` → `μ X. (grant ∨ (request ∧ [] X))` - Request holds until grant is received

#### Derived Temporal Patterns

| LTL Pattern | μ-Calculus Translation | Description |
|------------|------------------------|-------------|
| `G F φ` | `ν Y. (μ X. (φ ∨ [] X) ∧ [] Y)` | Infinitely often φ (always eventually) |
| `F G φ` | `μ Y. (ν X. (φ ∧ [] X) ∨ [] Y)` | Eventually forever φ (stabilization) |
| `G ¬bad` | `ν X. (¬bad ∧ [] X)` | Nothing bad ever happens (safety property) |
| `F good` | `μ X. (good ∨ [] X)` | Something good eventually happens (liveness property) |
| `G (req → F grant)` | `ν X. ((¬req ∨ μ Y. (grant ∨ [] Y)) ∧ [] X)` | Every request is eventually granted (responsiveness) |

**Examples:**
- `G F heartbeat` → `ν Y. (μ X. (heartbeat ∨ [] X) ∧ [] Y)` - Heartbeat occurs infinitely often
- `F G idle` → `μ Y. (ν X. (idle ∧ [] X) ∨ [] Y)` - System eventually stabilizes to idle state
- `G ¬deadlock` → `ν X. (¬deadlock ∧ [] X)` - Deadlock never occurs
- `F completion` → `μ X. (completion ∨ [] X)` - Completion is eventually reached
- `G (request → F approval)` → `ν X. ((¬request ∨ μ Y. (approval ∨ [] Y)) ∧ [] X)` - Every request eventually receives approval

#### Additional Operators

| LTL Pattern | μ-Calculus Translation | Description |
|------------|------------------------|-------------|
| `φ W ψ` | `(φ U ψ) ∨ G φ` = `μ X. (ψ ∨ (φ ∧ [] X)) ∨ (ν Y. (φ ∧ [] Y))` | Weak until (φ holds until ψ, or φ always holds) |
| `φ R ψ` | `!(!φ U !ψ)` = `!(μ X. (!ψ ∨ (!φ ∧ [] X)))` | Release (ψ holds until φ releases it) |
| `F[0..N] φ` | `< steps = N > φ` | φ happens within N steps (bounded eventually) |

**Note:** The bounded eventually pattern uses the bounded modality syntax supported by the evaluator.

#### GR(1) Patterns

| LTL Pattern | μ-Calculus Translation | Description |
|------------|------------------------|-------------|
| `G Bi` | `ν Xi. (Bi ∧ [] Xi)` | GR(1) safety clause: assumption/guarantee Bi always holds |
| `G F Lj` | `ν Yj. (μ Zj. (Lj ∨ [] Zj) ∧ [] Yj)` | GR(1) liveness clause: Lj happens infinitely often |

**Examples:**
- `G env_releases_resource` → `ν X. (env_releases_resource ∧ [] X)` - Environment always releases resources
- `G F progress` → `ν Y. (μ Z. (progress ∨ [] Z) ∧ [] Y)` - Progress occurs infinitely often

#### Domain-Specific Patterns

**BPMN Patterns:**

1. **Compensation Completes:**
   - Pattern: If compensation is triggered, it eventually completes
   - Translation: `ν X. (!compensation_triggered || (μ Y. compensation_completed || <> Y)) && [] X`
   - Placeholders: `compensation_triggered`, `compensation_completed`

2. **Boundary Event Handled:**
   - Pattern: Boundary events are always handled
   - Translation: `ν X. (!boundary_event_sig || (μ Y. handled_sig || <> Y)) && [] X`
   - Placeholders: `boundary_event_sig`, `handled_sig`

3. **Response Within Deadline:**
   - Pattern: Every request receives a response within a deadline
   - Translation: `ν X. (!request_sig || < steps = N > response_sig) && [] X`
   - Placeholders: `request_sig`, `response_sig`, `N`

4. **No Dead Tasks:**
   - Pattern: No task remains permanently blocked
   - Translation: `nu X. (has_enabled_transition && (mu Reach_task. task_signal || <> Reach_task)) && [] X`
   - Placeholders: `has_enabled_transition`, `task_signal`

**RTL Patterns:**

1. **Signal Stability:**
   - Pattern: Signal stabilizes after reset
   - Translation: `μ Y. (ν X. (stable ∧ [] X) ∨ [] Y)`
   - Placeholders: `stable`

#### Usage Guidelines

When implementing the translator, follow these guidelines:

1. **Always use fixpoints (μ or ν) for temporal properties** - `[]` and `<>` only look one step ahead
2. **Use ν (greatest fixpoint) for 'always' properties** - These require infinite satisfaction
3. **Use μ (least fixpoint) for 'eventually' properties** - These require finite satisfaction
4. **For bounded 'eventually'**, use `< steps = N >` syntax when available
5. **Combine patterns**: Responsiveness = always (request → eventually grant)
6. **Consider controllability**: `[]` accounts for uncontrollable moves in CLTS

#### Common Mistakes to Avoid

1. **Using `[]` for 'always' without fixpoint:**
   - ❌ Wrong: `[] safe`
   - ✅ Correct: `ν X. (safe ∧ [] X)`
   - Explanation: `[]` only checks next state, need `ν` for always

2. **Using `<>` for 'eventually' without fixpoint:**
   - ❌ Wrong: `<> goal`
   - ✅ Correct: `μ X. (goal ∨ [] X)`
   - Explanation: `<>` only checks next state, need `μ` for eventually

#### Implementation Reference

The translator implementation in `src/ltl/translator.rs` should follow these patterns exactly. Each translation rule should be implemented as a match arm in the `translate_formula` method, using the μ-calculus builder API to construct the corresponding formula structure.

**Reference File:** `docs/ltl_templates/ai_ltl_to_mu_cheatsheet.json` contains the complete JSON specification with all patterns, examples, and metadata.

---

### Phase 4: DSL Parser Integration (Week 2-3)

#### 4.1 Update Context DSL Parser

**File:** `src/context_dsl/parser.rs`

**Changes:**
1. Modify `parse_formula()` to detect LTL vs μ-calculus syntax
2. Add `parse_ltl_body()` method
3. Update `parse_mu_formulas_section()` to handle both

**Implementation:**
```rust
fn parse_formula(&mut self) -> Result<MuFormula, ParseError> {
    self.expect_keyword(Keyword::Formula)?;
    let name = self.expect_ident()?;
    self.expect_symbol(Symbol::LBrace)?;

    let mut meta = Meta::default();
    if self.check_keyword(Keyword::Meta) {
        meta = self.parse_meta_block()?;
    }

    self.expect_keyword(Keyword::Over)?;
    let targets = /* ... existing code ... */;

    self.expect_keyword(Keyword::Body)?;
    self.expect_symbol(Symbol::Assign)?;
    
    // Detect LTL vs μ-calculus syntax
    let body = if self.is_ltl_syntax() {
        FormulaExpr::Ltl(self.parse_ltl_body()?)
    } else {
        FormulaExpr::MuCalculus(self.parse_mu_body()?)
    };
    
    self.expect_symbol(Symbol::Semicolon)?;
    self.expect_symbol(Symbol::RBrace)?;

    Ok(MuFormula {
        name,
        meta,
        targets,
        body,
    })
}

fn is_ltl_syntax(&mut self) -> bool {
    // Peek ahead to detect LTL keywords
    let saved_pos = self.pos;
    let is_ltl = matches!(
        self.peek_kind(),
        TokenKind::Keyword(Keyword::Always)
            | TokenKind::Keyword(Keyword::Eventually)
            | TokenKind::Keyword(Keyword::Next)
            | TokenKind::Keyword(Keyword::Until)
            | TokenKind::Keyword(Keyword::WeakUntil)
            | TokenKind::Keyword(Keyword::Release)
    );
    self.pos = saved_pos;
    is_ltl
}

fn parse_ltl_body(&mut self) -> Result<LtlExpr, ParseError> {
    let start_token = self.peek().span;
    let start = start_token.start;
    
    // Parse LTL formula using ltl::parser
    let mut formula_text = String::new();
    while !self.check_symbol(Symbol::Semicolon) {
        formula_text.push_str(&self.advance().to_string());
        formula_text.push(' ');
    }
    
    let formula = ltl::parser::parse(&formula_text.trim())
        .map_err(|e| ParseError::InvalidExpr {
            span: start_token,
            message: format!("LTL parse error: {}", e),
        })?;
    
    let end = self.previous_span().end;
    Ok(LtlExpr {
        formula,
        span: Span::new(start, end, start_token.line, start_token.column),
    })
}
```

**Tests:**
- `test_parse_ltl_formula` - Parse LTL formula in DSL
- `test_parse_mu_formula` - Parse μ-calculus formula (backward compatibility)
- `test_parse_mixed_formulas` - Parse both LTL and μ-calculus in same section
- `test_parse_ltl_syntax_detection` - Verify correct syntax detection

#### 4.2 Update Realization

**File:** `src/context_dsl/realize.rs`

**Changes:**
```rust
for doc in &docs {
    for formula in &doc.mu_formulas {
        let name = formula.name.name.clone();
        if name == "__input_signals__" {
            continue;
        }
        if formulas.contains_key(&name) {
            return Err(RealizationError::Duplicate {
                kind: "μ-formula",
                name,
            });
        }
        
        let (parsed, parse_error) = match &formula.body {
            FormulaExpr::MuCalculus(mu_expr) => {
                // Existing μ-calculus parsing
                match mu_parser::parse(&mu_expr.raw) {
                    Ok(parsed) => (parsed, None),
                    Err(error) => (
                        mu_parser::parse("true").expect("fallback parses"),
                        Some(error.to_string()),
                    ),
                }
            },
            FormulaExpr::Ltl(ltl_expr) => {
                // New LTL translation
                match ltl::translator::translate(&ltl_expr.formula) {
                    Ok(translated) => (translated, None),
                    Err(error) => (
                        mu_parser::parse("true").expect("fallback parses"),
                        Some(format!("LTL translation error: {}", error)),
                    ),
                }
            },
        };
        
        // ... rest of existing code ...
    }
}
```

**Tests:**
- `test_realize_ltl_formula` - Realize LTL formula
- `test_realize_mu_formula` - Realize μ-calculus formula (backward compatibility)
- `test_realize_ltl_translation_error` - Handle translation errors gracefully

---

### Phase 5: Comprehensive Testing (Week 3)

#### 5.1 Unit Tests for Each LTL Operator

**File:** `src/ltl/tests.rs`

**Test Categories:**

1. **Basic Temporal Operators:**
   - `test_parse_and_translate_always` - `G safe` → `ν X. (safe ∧ [] X)`
   - `test_parse_and_translate_eventually` - `F completed` → `μ X. (completed ∨ [] X)`
   - `test_parse_and_translate_next` - `X alarm` → `[] alarm`
   - `test_parse_and_translate_until` - `request U grant` → `μ X. (grant ∨ (request ∧ [] X))`

2. **Propositional Operators:**
   - `test_parse_and_translate_not` - `!deadlock`
   - `test_parse_and_translate_and` - `safe && bounded`
   - `test_parse_and_translate_or` - `error || warning`
   - `test_parse_and_translate_implies` - `request -> F grant`

3. **Derived Patterns:**
   - `test_parse_and_translate_recurrence` - `GF heartbeat`
   - `test_parse_and_translate_stabilization` - `FG idle`
   - `test_parse_and_translate_response` - `G(request -> F(grant))`

4. **Complex Formulas:**
   - `test_parse_and_translate_nested_always` - `G(G safe)`
   - `test_parse_and_translate_mixed_operators` - `G(safe && F(completed))`
   - `test_parse_and_translate_gr1_pattern` - `G(env_assume) && GF(env_justice)`

5. **Edge Cases:**
   - `test_parse_empty_formula` - Error handling
   - `test_parse_unclosed_paren` - Error handling
   - `test_parse_invalid_operator` - Error handling
   - `test_translate_deeply_nested` - Performance test

#### 5.2 Integration Tests

**File:** `src/context_dsl/tests.rs` (additions)

**Test Cases:**
```rust
#[test]
fn parses_ltl_safety_property() {
    let doc = parse_context(
        r#"
        context test {
            automata {
                automaton Machine {
                    states { state Idle initial; }
                    transitions { transition Idle -> Idle on epsilon; }
                }
            }
            mu_formulas {
                formula safety {
                    over Machine;
                    body = G !deadlock;
                }
            }
        }
        "#,
    );
    
    assert_eq!(doc.mu_formulas.len(), 1);
    match &doc.mu_formulas[0].body {
        FormulaExpr::Ltl(ltl_expr) => {
            assert!(matches!(ltl_expr.formula, LtlFormula::Always(_)));
        },
        _ => panic!("Expected LTL formula"),
    }
}

#[test]
fn parses_ltl_liveness_property() {
    // Similar test for F completed
}

#[test]
fn parses_ltl_response_property() {
    // Test G(request -> F(grant))
}

#[test]
fn parses_mixed_ltl_and_mu_formulas() {
    // Test both LTL and μ-calculus in same section
}

#[test]
fn realizes_ltl_formula_correctly() {
    // Test end-to-end: parse → translate → evaluate
}
```

#### 5.3 Pattern-Based Tests

**File:** `tests/ltl_patterns.rs` (new)

Test all patterns from `docs/temporal_logic_patterns.md`:

```rust
#[test]
fn test_safety_mutual_exclusion() {
    // G(!(in_critical_section_1 && in_critical_section_2))
}

#[test]
fn test_safety_bounded_buffer() {
    // G(buffer_count <= N)
}

#[test]
fn test_liveness_request_response() {
    // G(request -> F(response))
}

#[test]
fn test_reactiveness_conditional_response() {
    // G((req1 -> F(grant1)) && (req2 -> F(grant2)))
}

#[test]
fn test_gr1_contract() {
    // G(env_assume) && GF(env_justice) -> G(sys_guarantee) && GF(sys_justice)
}
```

---

### Phase 6: Documentation and Examples (Week 3-4)

#### 6.1 Update DSL Documentation

**File:** `docs/clts_spec.md` (update)

Add LTL syntax section:
- LTL operator syntax
- Examples for each operator
- Comparison with μ-calculus
- When to use LTL vs μ-calculus

#### 6.2 Create LTL Tutorial

**File:** `docs/ltl_tutorial.md` (new)

- Introduction to LTL in Context DSL
- Basic operators with examples
- Common patterns (safety, liveness, reactiveness)
- GR(1) patterns
- Migration guide from μ-calculus

#### 6.3 Update Examples

Update existing examples to include LTL formulas:
- `examples/bpmn/` - Add LTL properties
- `examples/synchronous/` - Add LTL safety/liveness
- `examples/asynchronous/` - Add LTL reactiveness

---

## Test Coverage Requirements

### Unit Tests

- **LTL Parser:** 100% coverage of all operators
- **LTL Translator:** 100% coverage of all translation rules
- **Error Handling:** All error paths tested

### Integration Tests

- **DSL Parsing:** LTL formulas parse correctly in Context DSL
- **Realization:** LTL formulas translate and realize correctly
- **Backward Compatibility:** Existing μ-calculus formulas still work

### Pattern Tests

- **Safety Patterns:** All safety patterns from `temporal_logic_patterns.md`
- **Liveness Patterns:** All liveness patterns
- **Reactiveness Patterns:** All reactiveness patterns
- **GR(1) Patterns:** All GR(1) patterns

---

## Success Criteria

1. ✅ LTL formulas can be written in Context DSL alongside μ-calculus
2. ✅ All LTL operators (G, F, X, U, W, R) are supported
3. ✅ LTL → μ-calculus translation is correct for all operators
4. ✅ Existing μ-calculus formulas continue to work (backward compatibility)
5. ✅ All patterns from `temporal_logic_patterns.md` can be expressed
6. ✅ Comprehensive test coverage (>95%)
7. ✅ Documentation is complete and accurate

---

## Timeline

- **Week 1:** Phase 1-2 (AST, Parser)
- **Week 2:** Phase 3-4 (Translator, Integration)
- **Week 3:** Phase 5 (Testing)
- **Week 4:** Phase 6 (Documentation)

**Total:** 4 weeks

---

## Risk Mitigation

### Risk 1: Syntax Ambiguity
**Mitigation:** Clear precedence rules, comprehensive parser tests

### Risk 2: Translation Correctness
**Mitigation:** Reference implementation from `ai_ltl_to_mu_cheatsheet.json`, extensive translation tests

### Risk 3: Performance Impact
**Mitigation:** Translation happens once during realization, not during evaluation

### Risk 4: Backward Compatibility
**Mitigation:** Syntax detection ensures existing μ-calculus formulas are unchanged

---

## Future Enhancements (Out of Scope)

- CTL support (computation tree logic)
- Past operators (Y, H, O, S)
- Bounded operators (F[0..N], G[0..N])
- LTL formula simplification/optimization
- LTL formula pretty-printing

---

## References

- `docs/ltl_templates/ai_ltl_to_mu_cheatsheet.json` - **LTL to μ-Calculus translation patterns** (authoritative reference for all translation rules)
- `docs/ltl_templates/ltl_implementation_plan.md` - This document
- `docs/mu_calculus_grammar_semantics.md` - μ-calculus reference

**Note:** The translation patterns in Phase 3.2 are extracted from `ai_ltl_to_mu_cheatsheet.json`. For the complete specification including domain-specific patterns, usage guidelines, and common mistakes, refer to the JSON file directly.
