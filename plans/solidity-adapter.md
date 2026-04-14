# Solidity / Smart Contract Adapter — Implementation & Tracking Plan

> **Status:** IN PROGRESS — Phase 0 done
> **Last updated:** 2026-04-08
> **Tracking:** Update the "Progress Log" and phase status markers as work proceeds.

---

## Agent Instructions

**Before summarizing the current chat or ending a session, the agent MUST:**

1. Read this file to get the latest state.
2. Update the **Progress Log** section at the bottom with a dated entry summarizing:
   - What was accomplished in this session
   - What decisions were made and why
   - What files were created or modified
   - Any blockers, open questions, or deferred items
   - What the next step should be
3. Update the status marker (NOT STARTED / IN PROGRESS / BLOCKED / DONE) on every phase heading to reflect current reality.
4. If a viability concern was discovered, add it to the **Risk Register** section.
5. Save the file before responding to the user.

This ensures continuity across sessions — the plan file IS the source of truth for project state.

---

## Context

Mununu is a formal verification / reactive synthesis tool for CLTS (Compositional Labeled Transition Systems). The adapter pipeline is:

```
Source → Parser → Format AST → to_ir() → AdapterIR → emit() → CTXDSL → CLTS realization → mu-calculus evaluation / synthesis
```

Three adapters exist (TLSF, AIGER, Promela). Two more are planned first (XState, SystemVerilog). Solidity comes third and presents unique challenges: 256-bit integers, dynamic mappings, and the need for aggressive abstraction. The game-theoretic framing ("can an attacker force a bad state regardless of what the owner does?") is novel for smart contract security and maps directly to Mununu's Ramadge-Wonham supervisory control paradigm.

### Why Solidity is a good fit
- Contracts ARE state machines: storage variables = state, functions = transitions, `msg.sender` = controllability
- `onlyOwner` / access-controlled functions = controllable, `public` / `external` = uncontrollable (environment/attacker)
- `require()` / modifiers = transition guards
- Contract interactions = asynchronous composition
- Reentrancy is fundamentally a turn-based game
- No existing tool frames smart contract security as reactive synthesis

### Key challenge
Solidity uses 256-bit integers and dynamic mappings. Mununu uses explicit state enumeration (practical limit ~18 bits / 262K states). This adapter MUST use aggressive abstraction to be viable.

---

## Phase 0: Viability Evaluation — `DONE`

**Goal:** Determine whether Solidity contracts can be meaningfully abstracted into the AdapterIR's state space limits. Gate decision: proceed or abandon.

### 0.1 Abstraction Strategy Research

Investigate which abstraction approach makes contract state spaces tractable:

| Strategy | Description | Pros | Cons |
|----------|-------------|------|------|
| **Manual bounding** | User provides bounds for each variable via annotations | Simple, transparent | User burden |
| **Enum abstraction** | Map `uint` to a small enum: `{ZERO, LOW, MED, HIGH, MAX}` | Compact | Loses precision |
| **Predicate abstraction** | Track boolean predicates over variables (e.g., `balance > 0`, `balance == totalSupply`) | Precise for invariants | Combinatorial if many predicates |
| **Role abstraction** | Abstract addresses to roles: `{OWNER, USER, ATTACKER}` instead of 2^160 addresses | Essential — addresses are infinite | Requires user annotation or convention |

**Spike tests (no parser needed):**

1. **ERC-20 Token — hand-translated to CTXDSL:**
   - Abstract `balances[addr]` to `{ZERO, POSITIVE}` per role (OWNER, USER)
   - Abstract `totalSupply` to `{ZERO, POSITIVE}`
   - Functions: `transfer`, `approve`, `mint`, `burn`
   - States: 2 roles × 2 balance levels × 2 totalSupply levels = 8 states
   - Property: `G(sum_balances == totalSupply)` — token conservation
   - Hand-write the CTXDSL, load it, evaluate/synthesize. **Does the property hold?**

2. **Simple Vault — hand-translated to CTXDSL:**
   - Storage: `balance` (ZERO/POSITIVE), `locked` (bool), `owner` (role)
   - Functions: `deposit` (public), `withdraw` (onlyOwner), `lock` (onlyOwner)
   - States: 2 × 2 × 1 = 4 states (owner is fixed)
   - Property: `G(locked -> no_withdraw_possible)` — lock prevents withdrawal
   - Reentrancy test: add `externalCall` transition between state changes

3. **Reentrancy-vulnerable contract — hand-translated:**
   - The classic DAO pattern: `withdraw()` calls external contract before updating balance
   - Model the attacker as an environment automaton that can re-enter during the external call
   - Property: `G(balance >= 0)` or `G(no_double_withdraw)`
   - **Expected: UNREALIZABLE** (the attacker can drain the contract)

4. **Reentrancy-safe contract — hand-translated:**
   - Same contract but with checks-effects-interactions pattern (state updated before external call)
   - **Expected: REALIZABLE**

Tests live in `tests/adapter_solidity_viability.rs`.

### 0.2 Abstraction Framework Design

Define how the adapter creates abstract state machines from Solidity:

```
Solidity source
  → Parse (ABI + simplified control flow)
  → Identify storage variables + their abstract domains
  → Identify functions + their access control (controllability)
  → Build abstract transition system
  → AdapterIR (explicit-automaton path)
```

**Key abstractions to define:**
- **Address abstraction**: `{OWNER, APPROVED, ANYONE}` — derived from access modifiers
- **Uint abstraction**: User-provided bounds or auto-derived enum `{ZERO, BELOW_THRESHOLD, ABOVE_THRESHOLD, MAX}`
- **Mapping abstraction**: `mapping(address => uint)` → per-role balance state (ZERO/POSITIVE/THRESHOLD)
- **Bool**: direct mapping (no abstraction needed)
- **Enum**: direct mapping (no abstraction needed)

### 0.3 Viability Gate Criteria

| Criterion | Pass Condition |
|-----------|---------------|
| ERC-20 token conservation holds in abstract model | Safety property evaluates TRUE on hand-written CTXDSL |
| Reentrancy-vulnerable contract is UNREALIZABLE | Synthesis detects attacker strategy |
| Reentrancy-safe contract is REALIZABLE | Synthesis produces valid controller |
| Abstract state space for typical DeFi contract ≤ 1000 states | Manual estimate on 3+ real contracts |
| Abstraction preserves key safety properties | No false negatives on known vulnerabilities |

### 0.4 Testing Approach for Phase 0

- All tests are CTXDSL-only (no Solidity parser yet)
- Each hand-translated contract includes inline comments mapping CTXDSL constructs back to Solidity
- Tests use the standard `translate_and_realize` pattern but loading CTXDSL directly
- Assertions: realizability verdicts, state counts, counterexample traces for unrealizable cases

---

## Phase 1: Core Adapter — Solidity → CTXDSL — `NOT STARTED`

**Prerequisite:** Phase 0 viability gate passed.

### 1.1 Input Format Decision

Two options (decide during Phase 0):

| Option | Input | Parser complexity | Precision |
|--------|-------|-------------------|-----------|
| A. Solidity source (`.sol`) | Full Solidity | Very high (full parser) | High |
| B. Annotated ABI + state schema (`.sol.json`) | JSON with ABI + annotations | Low (serde) | Medium |
| C. Solidity AST from `solc --ast-compact-json` | Compiler AST output | Medium (JSON traversal) | High |

**Recommendation:** Start with **Option C** (solc AST). It avoids writing a full Solidity parser while preserving semantic information. Require `solc` as an external dependency for the adapter. Fallback: Option B for users who can't run `solc`.

### 1.2 File Structure

```
src/adapter/solidity/
  mod.rs          — SolidityAdapter, FormatAdapter impl, to_ir()
  ast.rs          — Types for the supported Solidity subset (from solc AST)
  parser.rs       — solc AST JSON traversal OR direct .sol parsing (subset)
  abstraction.rs  — Variable abstraction (uint→enum, address→role, mapping→per-role)
  annotations.rs  — Parse @mununu annotations from NatSpec comments
```

### 1.3 Annotation Format

Solidity source annotations via NatSpec comments:

```solidity
/// @mununu:bounds balance: [0, 100]
/// @mununu:roles OWNER, USER, ATTACKER
/// @mununu:controllable withdraw, lock
/// @mununu:uncontrollable deposit, transfer, approve
/// @mununu:property safety token_conservation: G(total == sum_balances)
/// @mununu:property liveness withdrawal_possible: G(balance > 0 -> F withdrawn)
contract Token {
    mapping(address => uint256) public balances;
    uint256 public total;

    /// @mununu:access ANYONE
    function deposit() external payable { ... }

    /// @mununu:access OWNER
    function withdraw(uint256 amount) external { ... }
}
```

### 1.4 IR Construction

1. Parse contract → identify storage variables
2. Apply abstraction rules → bounded domains
3. Each function → set of guarded transitions:
   - Source: abstract pre-state
   - Guard: `require()` conditions abstracted to predicates
   - Target: abstract post-state (from assignments)
   - Label: function name
4. Controllability from access modifiers:
   - `onlyOwner` / admin-only → controllable (system can choose)
   - `public` / `external` → uncontrollable (attacker can call anytime)
5. Multi-contract → asynchronous composition (each contract = `AutomatonSpec`)
6. External calls → explicit interleaving points (reentrancy modeling)
7. Reentrancy: model as environment's ability to re-invoke public functions during an external call (before state update)

### 1.5 Reentrancy Encoding

The key novel contribution. For a function that calls an external contract:

```solidity
function withdraw() external {
    uint amount = balances[msg.sender];
    // EXTERNAL CALL — attacker can re-enter here
    (bool ok,) = msg.sender.call{value: amount}("");
    // STATE UPDATE
    balances[msg.sender] = 0;
}
```

This becomes two transitions with an interleaving point:
1. `withdraw_pre`: check guard (`balance > 0`), transition to intermediate state `withdrawing`
2. In `withdrawing` state: ALL public functions are enabled as uncontrollable transitions (attacker re-entrance)
3. `withdraw_post`: update balance to 0, return to normal state

For the safe (checks-effects-interactions) version:
1. `withdraw_safe`: update balance to 0, THEN call external (intermediate state, but balance is already 0, so re-entrance is harmless)

### 1.6 Changes to Existing Code

- [adapter/mod.rs](src/adapter/mod.rs): `pub mod solidity`, `SourceFormat::Solidity`, `.sol` / `.sol.json` extension
- [adapter/ir.rs](src/adapter/ir.rs): No changes needed
- [main.rs](src/main.rs): `"solidity" | "sol"` case

---

## Phase 2: Property Specification — `NOT STARTED`

### 2.1 Common Smart Contract Properties

| Property | LTL / mu-calculus | Category |
|----------|-------------------|----------|
| Token conservation | `G(total == sum_balances)` | Safety invariant |
| No unauthorized access | `G(admin_action -> caller == OWNER)` | Access control |
| Reentrancy safety | `G(in_external_call -> no_state_change_after)` | Safety |
| No permanent lock | `G(balance > 0 -> F withdrawn)` | Liveness |
| Approval respects allowance | `G(transferFrom -> approved_amount >= transfer_amount)` | Safety |
| Pausable enforcement | `G(paused -> no_transfers)` | Safety |

### 2.2 Property Derivation

Auto-generate properties from common patterns:
- `onlyOwner` functions → access control invariant
- `require(balance >= amount)` → balance conservation
- `ReentrancyGuard` modifier → reentrancy safety assertion
- `Pausable` → paused-state invariant

### 2.3 Assume-Guarantee for Multi-Contract

For composed contracts:
- Each contract's interface = uncontrollable from the other's perspective
- Assumptions about external contract behavior (e.g., "oracle always responds") as `PropertyRole::Assumption`
- Guarantees about own behavior as `PropertyRole::Guarantee`

---

## Phase 3: Controller Output — CLTS → Solidity — `NOT STARTED`

### 3.1 Output Format

Synthesized controller emitted as **Solidity modifier logic**:

```solidity
// Generated by Mununu — correct-by-construction guard
modifier mununu_guard() {
    require(
        // State conditions from synthesis
        (state == State.IDLE && msg.sender == owner) ||
        (state == State.ACTIVE && balance > 0),
        "Mununu: transition not allowed"
    );
    _;
}
```

Or as a **monitor contract** (compositional):

```solidity
contract MununuMonitor {
    enum State { SAFE, WARNING, VIOLATION }
    State public monitorState = State.SAFE;

    function beforeAction(bytes4 selector, address caller) external {
        // Transition logic from synthesized controller
        if (monitorState == State.SAFE && selector == bytes4(keccak256("withdraw(uint256)"))) {
            require(caller == owner, "Mununu: unauthorized");
        }
    }
}
```

### 3.2 Limitations

- Abstract controller must be re-interpreted in concrete Solidity (e.g., `POSITIVE` → `amount > 0`)
- Mapping indices are abstracted to roles — monitor must classify addresses at runtime
- Controller is conservative — may reject valid transactions that the abstraction can't distinguish

---

## Phase 4: Benchmarks — Non-Trivial, Verifiable — `NOT STARTED`

### Benchmark S1: ERC-20 Token Conservation

**Contract:** Standard ERC-20 with `mint`, `burn`, `transfer`, `approve`, `transferFrom`.

**Abstraction:**
- `balances[OWNER]`: {ZERO, POSITIVE}
- `balances[USER]`: {ZERO, POSITIVE}
- `totalSupply`: {ZERO, POSITIVE}
- `allowance[USER→OWNER]`: {ZERO, POSITIVE}

**Expected state count:** ~16-32 states (4 binary variables + function availability)

**Properties:**
| Name | Formula | Expected |
|------|---------|----------|
| `token_conservation` | `G(totalSupply_positive <-> some_balance_positive)` | Realizable |
| `no_unauthorized_mint` | `G(mint_called -> caller_is_OWNER)` | Realizable (onlyOwner) |
| `burn_reduces_supply` | `G(burn_called -> X(totalSupply_decreased_or_zero))` | Realizable |

**Expected realizability:** All REALIZABLE.

### Benchmark S2: Reentrancy-Vulnerable Vault (The DAO Pattern)

**Contract:**
```solidity
contract VulnerableVault {
    mapping(address => uint256) balances;
    function deposit() external payable { balances[msg.sender] += msg.value; }
    function withdraw() external {
        uint256 bal = balances[msg.sender];
        (bool ok,) = msg.sender.call{value: bal}(""); // VULNERABLE
        balances[msg.sender] = 0; // too late
    }
}
```

**Abstraction:**
- `balances[VICTIM]`: {ZERO, POSITIVE}
- `contract_eth`: {ZERO, POSITIVE}
- `in_withdraw`: bool (reentrancy state)

**Expected state count:** ~8-12 states

**Properties:**
| Name | Formula | Expected |
|------|---------|----------|
| `no_drain` | `G(contract_eth >= sum_balances)` | **UNREALIZABLE** — attacker re-enters |
| `withdraw_integrity` | `G(withdraw_called -> X(balance_zeroed))` | **UNREALIZABLE** — re-enter before zero |

**Expected realizability:** **UNREALIZABLE** for both. The counterexample trace should show the reentrancy attack path.

### Benchmark S3: Reentrancy-Safe Vault (CEI Pattern)

**Contract:** Same vault but with checks-effects-interactions:
```solidity
function withdraw() external {
    uint256 bal = balances[msg.sender];
    balances[msg.sender] = 0; // state update FIRST
    (bool ok,) = msg.sender.call{value: bal}("");
}
```

**Abstraction:** Same as S2.

**Properties:** Same as S2.

**Expected realizability:** REALIZABLE for both. The state update before external call prevents reentrancy exploitation.

**Cross-validation:** S2 vs S3 — identical contracts except for statement ordering. S2 unrealizable, S3 realizable. This is the key validation that the adapter correctly models reentrancy.

### Benchmark S4: Timelock Governor (Multi-Contract, Liveness)

**System:** Two contracts composed asynchronously:
- `Governor`: propose → vote → queue → execute (4 states)
- `Timelock`: queued → ready → executed (3 states, with delay abstracted as bounded counter 0-2)

**Abstraction:**
- `proposal_state`: {NONE, PROPOSED, VOTED, QUEUED, EXECUTED}
- `timelock_state`: {EMPTY, QUEUED, READY, EXECUTED}
- `delay_counter`: {0, 1, 2} (abstracted from actual block delay)

**Expected state count:** 5 × 4 × 3 = 60 product states

**Properties:**
| Name | Formula | Expected |
|------|---------|----------|
| `execute_requires_timelock` | `G(executed -> was_queued_and_delay_elapsed)` | Realizable |
| `proposal_eventually_resolves` | `G(proposed -> F(executed \|\| rejected))` | **UNREALIZABLE** if votes are uncontrollable |
| `no_skip_timelock` | `G(execute_called -> timelock_ready)` | Realizable |

### Benchmark S5: Access-Controlled Proxy (Upgradeable Pattern)

**Contract:** Proxy + implementation pattern with admin controls.
- `proxy_admin`: OWNER role controls upgrades
- `implementation`: any address can call logic functions
- Property: `G(upgrade_called -> caller_is_admin)`
- Simple 4-state model, fully realizable

**Expected state count:** ~8 states

---

## Phase 5: Test Plan — `NOT STARTED`

### Unit Tests (`src/adapter/solidity/`)

| Module | Tests |
|--------|-------|
| `parser.rs` | Parse solc AST (contract, functions, modifiers), parse annotations, handle missing fields |
| `abstraction.rs` | `abstract_uint_to_enum`, `abstract_address_to_role`, `abstract_mapping_to_per_role`, bound inference |
| `annotations.rs` | Parse `@mununu:bounds`, `@mununu:roles`, `@mununu:property`, `@mununu:access` |
| `mod.rs` | `to_ir_erc20`, `to_ir_vault`, `to_ir_multi_contract`, reentrancy encoding |

### Integration Tests (`tests/adapter_solidity.rs`)

| Test | What It Validates |
|------|-------------------|
| `sol_detect` | Content detection for `.sol` and `.sol.json` |
| `sol_erc20_roundtrip` | Full pipeline for ERC-20. Assert state count, automaton exists |
| `sol_vault_vulnerable_roundtrip` | Vulnerable vault translates to valid CTXDSL |
| `sol_vault_safe_roundtrip` | Safe vault translates to valid CTXDSL |
| `sol_multi_contract_composition` | Governor + Timelock asynchronous composition |
| `sol_annotation_parsing` | Annotations extracted and applied correctly |
| `sol_auto_detect` | `auto_translate()` for Solidity content |

### System Tests (`tests/adapter_solidity_system.rs`)

| Test | Benchmark | Asserts |
|------|-----------|---------|
| `sol_erc20_token_conservation` | S1 | Realizable, ~16-32 states |
| `sol_vault_vulnerable_unrealizable` | S2 | **UNREALIZABLE**, counterexample contains reentrancy |
| `sol_vault_safe_realizable` | S3 | REALIZABLE |
| `sol_vulnerable_vs_safe_cross` | S2 vs S3 | Opposite verdicts on identical properties |
| `sol_timelock_safety` | S4 | Realizable for safety properties |
| `sol_timelock_liveness_unrealizable` | S4 | Unrealizable for liveness (uncontrollable votes) |
| `sol_proxy_access_control` | S5 | Realizable |

### Criterion Benchmarks (`benches/solidity.rs`)

| Benchmark | What It Measures |
|-----------|-----------------|
| `sol_parse_erc20` | Parse + abstract time |
| `sol_full_pipeline_vault` | translate + realize + synthesize |
| `sol_multi_contract_composition` | Composition time for Governor + Timelock |

---

## Risk Register

| Risk | Likelihood | Impact | Mitigation |
|------|-----------|--------|------------|
| Abstraction too coarse — loses real vulnerabilities | Medium | High | Phase 0 validates on known-vulnerable contracts |
| Abstraction too fine — state explosion | Medium | Medium | User-controlled bounds, warn on >1000 states |
| `solc` dependency complicates setup | Low | Medium | Offer Option B (annotated JSON) as fallback |
| Reentrancy encoding doesn't capture all patterns | Medium | High | Test on multiple reentrancy variants (cross-function, read-only) |
| Users don't know what bounds to provide | Medium | Medium | Auto-inference heuristics + sensible defaults |

---

## Critical Files

| File | Role |
|------|------|
| [adapter/mod.rs](src/adapter/mod.rs) | FormatAdapter trait, registration |
| [adapter/ir.rs](src/adapter/ir.rs) | AdapterIR — may need extension for external-call interleaving |
| [adapter/emit.rs](src/adapter/emit.rs) | CTXDSL emitter — explicit-automaton path |
| [adapter/promela/mod.rs](src/adapter/promela/mod.rs) | Reference: variable automata, CFG extraction |
| [abstraction/](src/abstraction/) | Existing abstraction infra (intervals, symbol sets) |
| [examples/sterile_batch_release.ctxdsl](examples/sterile_batch_release.ctxdsl) | Reference: multi-automaton synchronous composition with safety properties |

---

## Progress Log

_Update this section at the end of each working session._

| Date | Session Summary |
|------|----------------|
| 2026-04-08 | Plan created. No implementation work started. |
| 2026-04-08 | **Phase 0 completed.** Created `tests/adapter_solidity_viability.rs` with 9 tests across 4 hand-translated contracts. Results: ERC-20 token (3 automata, 2×2×2 composed, REALIZABLE), Simple vault with lock (2 automata, REALIZABLE), Reentrancy-vulnerable vault — DAO pattern (**UNREALIZABLE** — attacker drains via reenter label), Reentrancy-safe vault — CEI pattern (REALIZABLE — balance zeroed before external call). Key finding: controllable labels must only be claimed by one automaton in composition (empty `controllable { }` blocks needed for non-claiming automata). Cross-validation test confirms opposite verdicts for vulnerable vs safe vaults. Viability gate: **PASSED** — abstraction to {ZERO, POSITIVE} is sufficient to detect reentrancy. 750 total tests. Next: Phase 1 (Solidity parser via solc AST or annotated JSON). |
