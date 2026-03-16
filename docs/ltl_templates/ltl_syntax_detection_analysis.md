# LTL Syntax Detection: Architecture Analysis

## Current State

**Problem:** The DSL parser currently treats all formula bodies as μ-calculus raw strings. There's no mechanism to detect or parse LTL syntax.

**Current Implementation:**
- All formula bodies → `FormulaExpr::MuCalculus(MuExpr { raw: String })`
- LTL operators (`G`, `F`, `X`, `U`, `W`, `R`) are **not** DSL keywords
- No syntax detection logic exists
- LTL parser exists but isn't integrated into DSL parsing

## Architectural Concerns

### Issue 1: No Syntax Detection

The implementation plan proposed `is_ltl_syntax()` that would peek ahead for LTL keywords, but:
- LTL operators are **not** DSL keywords (to allow use as identifiers)
- This makes keyword-based detection impossible
- The DSL parser has no way to distinguish LTL from μ-calculus

### Issue 2: Separation of Concerns Violation

**Current approach (if implemented as planned):**
```
DSL Parser → Detects LTL syntax → Calls LTL parser
```

**Problems:**
- DSL parser needs to know about LTL syntax (violates separation)
- Detection logic is fragile (what if formula starts with identifier?)
- Ambiguous cases: `G safe` could be LTL or μ-calculus with identifier `G`

### Issue 3: Ambiguity

LTL operators can be identifiers:
- `formula F { ... }` - formula named `F`
- `group G = { ... }` - state group named `G`
- `body = G safe` - LTL "always safe" or μ-calculus with identifier `G`?

## Proposed Solutions

### Option 1: Explicit Syntax Marker (Recommended)

**Approach:** Use explicit syntax markers in the DSL:

```clts
mu_formulas {
    formula safety {
        over A;
        body = mu true;  // Explicit μ-calculus
    }
    formula liveness {
        over A;
        body = ltl G F heartbeat;  // Explicit LTL
    }
}
```

**Pros:**
- ✅ Clear and unambiguous
- ✅ No detection logic needed
- ✅ Perfect separation of concerns
- ✅ Easy to parse (just check first token)
- ✅ Backward compatible (default to μ-calculus if no marker)

**Cons:**
- ⚠️ Requires users to specify syntax explicitly
- ⚠️ Slightly more verbose

**Implementation:**
```rust
fn parse_formula_body(&mut self) -> Result<FormulaExpr, ParseError> {
    self.skip_whitespace();
    if self.try_consume_keyword("ltl") {
        // Parse as LTL
        let formula = ltl::parser::parse(&self.read_until_semicolon()?)?;
        Ok(FormulaExpr::Ltl(LtlExpr { formula, span }))
    } else if self.try_consume_keyword("mu") {
        // Parse as μ-calculus (explicit)
        let raw = self.read_until_semicolon()?;
        Ok(FormulaExpr::MuCalculus(MuExpr { raw, span }))
    } else {
        // Default to μ-calculus (backward compatibility)
        let raw = self.read_until_semicolon()?;
        Ok(FormulaExpr::MuCalculus(MuExpr { raw, span }))
    }
}
```

### Option 2: Separate Sections

**Approach:** Have separate sections for LTL and μ-calculus:

```clts
mu_formulas {
    formula safety {
        over A;
        body = nu X. (safe && [] X);
    }
}

ltl_formulas {
    formula liveness {
        over A;
        body = G F heartbeat;
    }
}
```

**Pros:**
- ✅ Clear separation
- ✅ No ambiguity
- ✅ Easy to parse

**Cons:**
- ⚠️ Requires DSL grammar changes
- ⚠️ Less flexible (can't mix in same section)
- ⚠️ More breaking changes

### Option 3: Heuristic Detection (Not Recommended)

**Approach:** Try parsing as LTL first, fall back to μ-calculus:

```rust
fn parse_formula_body(&mut self) -> Result<FormulaExpr, ParseError> {
    let raw = self.read_until_semicolon()?;
    
    // Try LTL first
    if let Ok(ltl_formula) = ltl::parser::parse(&raw) {
        // Check if it looks like LTL (starts with G, F, X, etc.)
        if raw.trim_start().starts_with(|c| matches!(c, 'G' | 'F' | 'X')) {
            return Ok(FormulaExpr::Ltl(LtlExpr { formula: ltl_formula, span }));
        }
    }
    
    // Fall back to μ-calculus
    Ok(FormulaExpr::MuCalculus(MuExpr { raw, span }))
}
```

**Pros:**
- ✅ No syntax changes needed
- ✅ Automatic detection

**Cons:**
- ❌ Fragile and error-prone
- ❌ Ambiguous cases (what if μ-calculus starts with `G`?)
- ❌ Poor error messages
- ❌ Violates separation of concerns (DSL parser knows about LTL)

### Option 4: Context-Aware Keywords (Complex)

**Approach:** Make LTL operators keywords only in formula body context:

```rust
// In formula body parsing context
fn parse_formula_body(&mut self) -> Result<FormulaExpr, ParseError> {
    // Temporarily enable LTL keywords
    self.enable_ltl_keywords();
    // ... parse ...
    self.disable_ltl_keywords();
}
```

**Pros:**
- ✅ LTL operators are keywords where needed
- ✅ Can still be identifiers elsewhere

**Cons:**
- ❌ Complex implementation (context-aware lexing)
- ❌ Still ambiguous (what if identifier `G` is used in formula?)
- ❌ Violates separation (lexer needs context)

## Recommendation: Option 1 (Explicit Syntax Marker)

**Rationale:**
1. **Clear and unambiguous**: No guessing, no heuristics
2. **Perfect separation**: DSL parser doesn't need LTL knowledge
3. **Backward compatible**: Default to μ-calculus if no marker
4. **Simple implementation**: Just check first token
5. **User-friendly**: Makes intent explicit

**Syntax:**
```clts
mu_formulas {
    // Explicit μ-calculus (optional, default)
    formula safety {
        over A;
        body = nu X. (safe && [] X);
    }
    
    // Explicit μ-calculus (explicit marker)
    formula safety2 {
        over A;
        body = mu nu X. (safe && [] X);
    }
    
    // Explicit LTL
    formula liveness {
        over A;
        body = ltl G F heartbeat;
    }
}
```

**Implementation:**
- Add `ltl` and `mu` as optional keywords (only in formula body context)
- Parse body: if starts with `ltl`, parse as LTL; otherwise μ-calculus
- No detection logic needed
- Clean separation of concerns

## Migration Path

1. **Phase 1 (Current)**: All formulas default to μ-calculus
2. **Phase 2**: Add optional `ltl` marker, parse accordingly
3. **Phase 3**: Add optional `mu` marker for explicitness
4. **Future**: Consider making markers required for clarity

## Conclusion

The current approach (no syntax detection) is actually **correct** from a separation of concerns perspective, but it means:
- We need an explicit way to specify LTL syntax
- The DSL parser shouldn't try to "guess" the syntax
- Users should be explicit about their intent

**Recommendation:** Implement Option 1 (explicit syntax markers) to maintain clean architecture while providing the needed functionality.
