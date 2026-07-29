//! SPCR — Selective Prophecy-Cell Registerization (owned shot ①b of the array-νμ program).
//!
//! Removes the ∀-quantified array from the recoverability MUST-query by registerizing exactly
//! the property-ACCESSED array cells (one prophecy register per read index) and DROPPING the
//! array. The must-edge `∀ s ⊨ src. ∃ in, s'. …` then becomes pure QF_BV (decidable by
//! bit-blasting) at `O(#accessed-cells)` cost — instead of `AUFBV + ∀-over-an-array` (undecidable
//! → Z3 `Unknown` → no must-edges → the νµ abstains: the wall mechanized in
//! `recoverability::tests::p1a_array_gated_recovery_hits_must_edge_quantifier_wall`).
//! Plan: `.claude/plans/spcr-selective-prophecy-cell-registerization.md`.
//!
//! **Scope (SOUND, conservative — abstains (returns `None`) otherwise).** For EVERY array state in
//! the design, the next-state is a write to `waddr`/`wdata` (P-A1 an UNCONDITIONAL
//! `mem' = write(mem, waddr, wdata)`; P-A1b a write-ENABLE mux `ite(cond, write, mem)`, with a
//! tautological `cond` collapsing to unconditional), read only at bare-register indices. The frame
//!
//! ```text
//! pv' = ite(cond ∧ (waddr == idx'), wdata, pv)   (idx' = the index register's next-value)
//! ```
//!
//! reproduces `mem'[idx']` EXACTLY under two sound, checkable index disciplines:
//!   - **(U) unconditional / tautological write** — `idx'` may MOVE within `{idx, waddr}`: when
//!     `idx' = idx` the else-branch is `mem[idx] = pv`; when `idx' = waddr` the write always fires
//!     so the guard is true and it is `wdata = mem'[waddr]`.
//!   - **(S) conditional write** — `idx'` must be STABLE (`= idx` always): then `mem'[idx] =
//!     ite(cond ∧ waddr==idx, wdata, mem[idx]) = ite(cond ∧ waddr==idx, wdata, pv)`, exact for any
//!     `cond`. (A moving index under a conditional write is UNSOUND — the index could move to
//!     `waddr` while the write is disabled, leaving `mem[waddr] ≠ pv` — so it abstains.)
//!
//! So SPCR is a verdict-PRESERVING reformulation (not an abstraction): the KMTS of the SPCR'd design
//! has the same may/must edges as the original on the property-relevant projection ⇒ definite νµ
//! verdicts transfer at every alternation depth (Bruns–Godefroid). Residual (abstain): a
//! read-modify-write / input-indexed read (needs the array or a dead-code fold), a moving index
//! under a conditional write, write-chains, or a non-register index.

use crate::adapter::btor2::ast::{Btor2File, Line, Nid, Node, Op, Operand, Sort};
use crate::adapter::btor2::parser::find_next_value_operand;
use std::collections::{HashMap, HashSet};

/// A per-array elimination plan (the sound P-A1 shape, or `None` from [`plan_for_array`]).
struct ArrayPlan {
    array_nid: Nid,
    elem_sort: Nid,
    waddr: Operand,
    wdata: Operand,
    /// Broadcast init value (a const operand) if the array is initialised; else free at reset.
    init_value: Option<Operand>,
    /// `(read node nid, index register nid)` for every read of this array.
    reads: Vec<(Nid, Nid)>,
    /// distinct index register nid → its next-value operand (`None` = a never-written index).
    index_next: HashMap<Nid, Option<Operand>>,
    array_next_nid: Option<Nid>,
    array_init_nid: Option<Nid>,
    write_nid: Nid,
    /// The write-enable mux node `ite(cond, write, array)` (P-A1b), if the write is conditional.
    mux_nid: Option<Nid>,
    /// The write-enable condition (`None` = unconditional / a tautological enable). When `Some`,
    /// the write only fires under `cond`, so the frame guards `wdata` by `cond ∧ (waddr==idx')`
    /// AND the index is required to be STABLE (never moves) — see the soundness note in
    /// [`plan_for_array`].
    write_cond: Option<Operand>,
}

fn sort_of(file: &Btor2File, nid: Nid) -> Option<Sort> {
    match &file.lookup(nid)?.node {
        Node::Sort { sort } => Some(sort.clone()),
        _ => None,
    }
}

/// A syntactic tautology — a write-enable that always fires, so the mux is really an
/// unconditional write (`const 1`, `redor(nonzero const)`, `not(const 0)`). Yosys emits e.g.
/// `ite(redor(K), write, mem)` for a `for`-style always-write; treating it as unconditional
/// lets the moving-index (P-A1) frame apply.
fn is_tautology(file: &Btor2File, nid: Nid) -> bool {
    use crate::adapter::btor2::bit_blast::resolve_btor2_constant;
    if resolve_btor2_constant(file, nid).is_some_and(|v| v != 0) {
        return true;
    }
    match file.lookup(nid).map(|l| &l.node) {
        Some(Node::Op {
            op: Op::Redor,
            args,
            ..
        }) if args.len() == 1 => {
            resolve_btor2_constant(file, args[0].nid()).is_some_and(|v| v != 0)
        }
        Some(Node::Op {
            op: Op::Not, args, ..
        }) if args.len() == 1 => resolve_btor2_constant(file, args[0].nid()) == Some(0),
        _ => false,
    }
}

fn node_refs_array(node: &Node, array_nid: Nid) -> bool {
    match node {
        Node::Op { args, .. } => args.iter().any(|a| a.nid() == array_nid),
        Node::Next { state, value, .. } | Node::Init { state, value, .. } => {
            *state == array_nid || value.nid() == array_nid
        }
        Node::Bad { signal }
        | Node::Constraint { signal }
        | Node::Fair { signal }
        | Node::Output { signal, .. } => signal.nid() == array_nid,
        Node::Justice { signals } => signals.iter().any(|s| s.nid() == array_nid),
        _ => false,
    }
}

/// Detect the sound P-A1 SPCR shape for one array state. `None` ⇒ abstain (leave the array).
fn plan_for_array(file: &Btor2File, array_nid: Nid) -> Option<ArrayPlan> {
    let array_sort_nid = match &file.lookup(array_nid)?.node {
        Node::State { sort, .. } => *sort,
        _ => return None,
    };
    let Sort::Array { element, .. } = sort_of(file, array_sort_nid)? else {
        return None;
    };
    let elem_sort = element;

    // The array's next is either a single unconditional `Write(array, waddr, wdata)` (P-A1) or a
    // write-ENABLE mux `ite(cond, write(array, …), array)` (P-A1b). A tautological `cond` collapses
    // to unconditional. Anything else (write-chain, write in the else-branch, nested mux) abstains.
    let next_op = find_next_value_operand(file, array_nid)?;
    let array_next_nid = file.lines.iter().find_map(|l| match &l.node {
        Node::Next { state, .. } if *state == array_nid => Some(l.nid),
        _ => None,
    });
    let next_line = file.lookup(next_op.nid())?;
    let (write_nid, mux_nid, write_cond): (Nid, Option<Nid>, Option<Operand>) =
        match &next_line.node {
            Node::Op { op: Op::Write, .. } => (next_op.nid(), None, None),
            Node::Op {
                op: Op::Ite, args, ..
            } if args.len() == 3 && args[2].nid() == array_nid => {
                // ite(cond, write(array,…), array) — write in the THEN branch, hold in the else.
                let cond = args[0];
                let eff_cond = if is_tautology(file, cond.nid()) {
                    None
                } else {
                    Some(cond)
                };
                (args[1].nid(), Some(next_op.nid()), eff_cond)
            }
            _ => return None,
        };
    let write_line = file.lookup(write_nid)?;
    let (waddr, wdata) = match &write_line.node {
        Node::Op {
            op: Op::Write,
            args,
            ..
        } if args.len() == 3 && args[0].nid() == array_nid => (args[1], args[2]),
        _ => return None,
    };
    // SOUNDNESS: the frame `pv' = ite(cond ∧ waddr==idx', wdata, pv)` is exact iff `idx' ≠ idx ⟹
    // cond` (the index moves to `waddr` only when the write fires). Two sound, checkable cases:
    //   (U) `write_cond == None` (unconditional / tautological) ⇒ the move-to-`waddr` guard is
    //       trivially satisfied ⇒ a MOVING index (`idx' ∈ {idx, waddr}`) is fine.
    //   (S) `write_cond == Some` (a real enable) ⇒ require a STABLE index (never moves), so the
    //       `idx' = waddr` case never arises and the frame is exact for any `cond`.
    let allow_waddr = write_cond.is_none();

    // Broadcast init (a const operand) if present.
    let (init_value, array_init_nid) = file
        .lines
        .iter()
        .find_map(|l| match &l.node {
            Node::Init { state, value, .. } if *state == array_nid => {
                Some((Some(*value), Some(l.nid)))
            }
            _ => None,
        })
        .unwrap_or((None, None));

    // Reads of the array; each index must be a bare state register.
    let mut reads: Vec<(Nid, Nid)> = Vec::new();
    for l in &file.lines {
        if let Node::Op {
            op: Op::Read, args, ..
        } = &l.node
            && args.len() == 2
            && args[0].nid() == array_nid
        {
            let idx = args[1].nid();
            if !matches!(file.lookup(idx)?.node, Node::State { .. }) {
                return None; // non-register index → P-A1 abstains
            }
            reads.push((l.nid, idx));
        }
    }
    if reads.is_empty() {
        return None;
    }
    // The write-enable condition must not itself BE an array read (else the frame's guard would
    // dangle when the read is dropped). A cond that merely CONTAINS a read via an op node is fine
    // — that op is kept and remapped to `pv`. Direct-read-as-enable is bizarre; abstain.
    if let Some(cond) = write_cond
        && reads.iter().any(|&(r, _)| r == cond.nid())
    {
        return None;
    }

    // Stability: each index register's next-value moves only within {itself, waddr}.
    let mut index_next: HashMap<Nid, Option<Operand>> = HashMap::new();
    for &(_, idx) in &reads {
        if index_next.contains_key(&idx) {
            continue;
        }
        let nx = find_next_value_operand(file, idx);
        if let Some(nxop) = nx {
            let mut vis: HashSet<Nid> = HashSet::new();
            let mut st = vec![nxop.nid()];
            while let Some(n) = st.pop() {
                if !vis.insert(n) {
                    continue;
                }
                if n == idx {
                    continue; // held — always allowed
                }
                if n == waddr.nid() {
                    // move-to-write-address — allowed ONLY for an unconditional/tautological write
                    // (case U); a conditional write requires a stable index (case S).
                    if allow_waddr {
                        continue;
                    }
                    return None;
                }
                match &file.lookup(n)?.node {
                    Node::Op {
                        op: Op::Ite, args, ..
                    } if args.len() == 3 => {
                        st.push(args[1].nid());
                        st.push(args[2].nid());
                    }
                    // a leaf other than {idx, waddr} ⇒ the index moves to an arbitrary cell
                    // whose old content `pv` does not hold ⇒ frame is not exact ⇒ abstain.
                    _ => return None,
                }
            }
        }
        index_next.insert(idx, nx);
    }

    // No OTHER reference to the array beyond the reads + the write + the enable-mux + next + init.
    let mut expected: HashSet<Nid> = reads.iter().map(|&(r, _)| r).collect();
    expected.insert(write_nid);
    if let Some(n) = mux_nid {
        expected.insert(n);
    }
    if let Some(n) = array_next_nid {
        expected.insert(n);
    }
    if let Some(n) = array_init_nid {
        expected.insert(n);
    }
    for l in &file.lines {
        if !expected.contains(&l.nid) && node_refs_array(&l.node, array_nid) {
            return None; // an unexpected use of the array ⇒ cannot eliminate soundly
        }
    }

    Some(ArrayPlan {
        array_nid,
        elem_sort,
        waddr,
        wdata,
        init_value,
        reads,
        index_next,
        array_next_nid,
        array_init_nid,
        write_nid,
        mux_nid,
        write_cond,
    })
}

fn remap_node(node: &Node, f: &impl Fn(Operand) -> Operand) -> Node {
    match node {
        Node::Op {
            sort,
            op,
            args,
            symbol,
        } => Node::Op {
            sort: *sort,
            op: *op,
            args: args.iter().map(|a| f(*a)).collect(),
            symbol: symbol.clone(),
        },
        Node::Next { sort, state, value } => Node::Next {
            sort: *sort,
            state: *state,
            value: f(*value),
        },
        Node::Init { sort, state, value } => Node::Init {
            sort: *sort,
            state: *state,
            value: f(*value),
        },
        Node::Bad { signal } => Node::Bad { signal: f(*signal) },
        Node::Constraint { signal } => Node::Constraint { signal: f(*signal) },
        Node::Fair { signal } => Node::Fair { signal: f(*signal) },
        Node::Output { signal, symbol } => Node::Output {
            signal: f(*signal),
            symbol: symbol.clone(),
        },
        Node::Justice { signals } => Node::Justice {
            signals: signals.iter().map(|s| f(*s)).collect(),
        },
        other => other.clone(),
    }
}

/// P-A1c — a small SOUND constant-fold + dead-code-elimination pass, run BEFORE SPCR. Its purpose
/// is to collapse the yosys per-bit-write-ENABLE modeling of a plain full write:
///
/// ```text
/// mem' = write(mem, a, (wdata & mask) | (mem[a] & ~mask))     with mask = all-ones
/// ```
///
/// With `mask` all-ones, `mem[a] & ~mask = mem[a] & 0 = 0`, so the read-modify-write read `mem[a]`
/// is DEAD. Folding `not(allones)→0`, `and(x,0)→0`, `or(x,0)→x`, `and(x,allones)→x` collapses the
/// value to `wdata` and DCE removes the dead read — so SPCR (`plan_for_array`) then sees a single
/// clean `write(mem, a, wdata)`. Every fold is width-aware and semantics-preserving; DCE removes
/// only nodes unreachable from the roots (next/init/bad/constraint/fair/justice/output). Nodes
/// wider than 64 bits are left un-folded (conservative — SPCR then simply abstains).
fn fold_and_dce(file: &Btor2File) -> Btor2File {
    use crate::adapter::btor2::ast::ConstValue;
    use crate::adapter::btor2::bit_blast::resolve_btor2_constant;
    use crate::adapter::btor2::parser::bv_width;

    // Phase 1 — constant values (u64 fixpoint over not/and/or/xor + and-0 / or-allones).
    let mut cval: HashMap<Nid, u64> = HashMap::new();
    for l in &file.lines {
        if let Some(v) = resolve_btor2_constant(file, l.nid) {
            cval.insert(l.nid, v);
        }
    }
    loop {
        let mut changed = false;
        for l in &file.lines {
            let Node::Op { op, args, sort, .. } = &l.node else {
                continue;
            };
            if cval.contains_key(&l.nid) {
                continue;
            }
            let Some(w) = bv_width(file, *sort) else {
                continue;
            };
            if w > 64 {
                continue;
            }
            let m: u64 = if w >= 64 { u64::MAX } else { (1u64 << w) - 1 };
            let a = args.first().and_then(|o| cval.get(&o.nid()).copied());
            let b = args.get(1).and_then(|o| cval.get(&o.nid()).copied());
            let r = match op {
                Op::Not => a.map(|x| !x & m),
                Op::And => {
                    if a == Some(0) || b == Some(0) {
                        Some(0)
                    } else if let (Some(x), Some(y)) = (a, b) {
                        Some(x & y)
                    } else {
                        None
                    }
                }
                Op::Or => {
                    if a == Some(m) || b == Some(m) {
                        Some(m)
                    } else if let (Some(x), Some(y)) = (a, b) {
                        Some(x | y)
                    } else {
                        None
                    }
                }
                Op::Xor => match (a, b) {
                    (Some(x), Some(y)) => Some(x ^ y),
                    _ => None,
                },
                _ => None,
            };
            if let Some(v) = r {
                cval.insert(l.nid, v);
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }

    // Phase 2 — replacement operands: a const-folded op → a materialized const; a structural
    // identity `and(x, allones)→x` / `or(x, 0)→x` → the surviving operand.
    let mut next_nid: Nid = file.lines.iter().map(|l| l.nid).max().unwrap_or(0) + 1;
    let mut extra_consts: Vec<Line> = Vec::new();
    let mut const_of: HashMap<(u64, Nid), Nid> = HashMap::new();
    let mut repl: HashMap<Nid, Operand> = HashMap::new();
    for l in &file.lines {
        let Node::Op { op, args, sort, .. } = &l.node else {
            continue;
        };
        // A non-Const op that resolved to a constant → materialize / reuse a const node.
        if let Some(&v) = cval.get(&l.nid) {
            let nid = *const_of.entry((v, *sort)).or_insert_with(|| {
                let n = next_nid;
                next_nid += 1;
                extra_consts.push(Line {
                    nid: n,
                    node: Node::Const {
                        sort: *sort,
                        value: ConstValue::Dec(v as i128),
                    },
                    immediates: Vec::new(),
                    source_line: 0,
                });
                n
            });
            repl.insert(l.nid, Operand(nid));
            continue;
        }
        let Some(w) = bv_width(file, *sort) else {
            continue;
        };
        if w > 64 || args.len() != 2 {
            continue;
        }
        let m: u64 = if w >= 64 { u64::MAX } else { (1u64 << w) - 1 };
        let (av, bv) = (
            cval.get(&args[0].nid()).copied(),
            cval.get(&args[1].nid()).copied(),
        );
        let pass = match op {
            Op::And => {
                if av == Some(m) {
                    Some(args[1])
                } else if bv == Some(m) {
                    Some(args[0])
                } else {
                    None
                }
            }
            Op::Or => {
                if av == Some(0) {
                    Some(args[1])
                } else if bv == Some(0) {
                    Some(args[0])
                } else {
                    None
                }
            }
            _ => None,
        };
        if let Some(o) = pass {
            repl.insert(l.nid, o);
        }
    }

    // Resolve a (non-negated) operand through the replacement chain. Negated operands are left
    // untouched (yosys emits explicit `not` nodes, not operand negation — conservative + sound).
    let resolve = |o: Operand| -> Operand {
        if o.0 < 0 {
            return o;
        }
        let mut cur = o;
        for _ in 0..=file.lines.len() {
            match repl.get(&cur.nid()) {
                Some(&r) if r.0 >= 0 => cur = r,
                _ => break,
            }
        }
        cur
    };

    // Rewrite every operand through `resolve`, then append the fresh const nodes.
    let mut rewritten: Vec<Line> = file
        .lines
        .iter()
        .map(|l| Line {
            nid: l.nid,
            node: remap_node(&l.node, &resolve),
            immediates: l.immediates.clone(),
            source_line: l.source_line,
        })
        .collect();
    rewritten.extend(extra_consts);

    // Phase 3 — DCE. Keep sorts/states/inputs + the root lines always; keep Op/Const iff reachable
    // from a root (next/init/bad/constraint/fair/justice/output) through op args.
    let by_nid: HashMap<Nid, usize> = rewritten
        .iter()
        .enumerate()
        .map(|(i, l)| (l.nid, i))
        .collect();
    let mut reachable: HashSet<Nid> = HashSet::new();
    let mut stack: Vec<Nid> = Vec::new();
    for l in &rewritten {
        match &l.node {
            Node::Next { state, value, .. } | Node::Init { state, value, .. } => {
                stack.push(*state);
                stack.push(value.nid());
            }
            Node::Bad { signal }
            | Node::Constraint { signal }
            | Node::Fair { signal }
            | Node::Output { signal, .. } => stack.push(signal.nid()),
            Node::Justice { signals } => stack.extend(signals.iter().map(|s| s.nid())),
            _ => {}
        }
    }
    while let Some(n) = stack.pop() {
        if !reachable.insert(n) {
            continue;
        }
        if let Some(&i) = by_nid.get(&n)
            && let Node::Op { args, .. } = &rewritten[i].node
        {
            stack.extend(args.iter().map(|a| a.nid()));
        }
    }
    let out: Vec<Line> = rewritten
        .into_iter()
        .filter(|l| match &l.node {
            Node::Op { .. } | Node::Const { .. } => reachable.contains(&l.nid),
            _ => true, // sorts, states, inputs, and root lines are always kept
        })
        .collect();
    let by_nid = out.iter().enumerate().map(|(i, l)| (l.nid, i)).collect();
    Btor2File { lines: out, by_nid }
}

/// SPCR entry: if EVERY in-cone array matches the sound P-A1 shape (after the P-A1c fold), return
/// the array-FREE equivalent design (prophecy registers + exact frames, arrays dropped); else
/// `None` (the caller keeps the original and falls through to its honest abstain).
///
/// **Engine:** owned BTOR2→BTOR2 rewrite (no solver). The result is consumed by the array-free
/// deciders — `exact-symbolic` ROBDD (small cone) or the `symbolic` predicate-cube (QF_BV must).
pub(crate) fn spcr(file: &Btor2File) -> Option<Btor2File> {
    // Cheap guard: SPCR (and the fold) are only for array-bearing designs.
    if !file.lines.iter().any(|l| {
        matches!(&l.node, Node::State { sort, .. } if matches!(sort_of(file, *sort), Some(Sort::Array { .. })))
    }) {
        return None;
    }
    // P-A1c: fold the yosys write-mask RMW into a clean write so `plan_for_array` can apply.
    let folded = fold_and_dce(file);
    let file = &folded;
    let array_states: Vec<Nid> = file
        .lines
        .iter()
        .filter_map(|l| match &l.node {
            Node::State { sort, .. }
                if matches!(sort_of(file, *sort), Some(Sort::Array { .. })) =>
            {
                Some(l.nid)
            }
            _ => None,
        })
        .collect();
    if array_states.is_empty() {
        return None; // no arrays ⇒ nothing for SPCR to do (caller uses the normal path)
    }
    // P-A1: every array must be soundly eliminable, else abstain wholesale.
    let plans: Vec<ArrayPlan> = array_states
        .iter()
        .map(|&a| plan_for_array(file, a))
        .collect::<Option<Vec<_>>>()?;

    let mut next_nid: Nid = file.lines.iter().map(|l| l.nid).max().unwrap_or(0) + 1;
    let mut alloc = || {
        let n = next_nid;
        next_nid += 1;
        n
    };

    // A 1-bit sort for the `eq(waddr, idx')` guards (reuse an existing one, else create it).
    let mut prelude: Vec<Line> = Vec::new();
    let bool_sort = file
        .lines
        .iter()
        .find_map(|l| match &l.node {
            Node::Sort {
                sort: Sort::BitVec { width: 1 },
            } => Some(l.nid),
            _ => None,
        })
        .unwrap_or_else(|| {
            let n = alloc();
            prelude.push(Line {
                nid: n,
                node: Node::Sort {
                    sort: Sort::BitVec { width: 1 },
                },
                immediates: Vec::new(),
                source_line: 0,
            });
            n
        });

    let mut tail: Vec<Line> = Vec::new();
    let mut read_to_pv: HashMap<Nid, Nid> = HashMap::new();
    let mut drop_nodes: HashSet<Nid> = HashSet::new();

    for plan in &plans {
        drop_nodes.insert(plan.array_nid);
        drop_nodes.insert(plan.write_nid);
        if let Some(n) = plan.mux_nid {
            drop_nodes.insert(n);
        }
        if let Some(n) = plan.array_next_nid {
            drop_nodes.insert(n);
        }
        if let Some(n) = plan.array_init_nid {
            drop_nodes.insert(n);
        }
        for &(r, _) in &plan.reads {
            drop_nodes.insert(r);
        }

        let mut idx_to_pv: HashMap<Nid, Nid> = HashMap::new();
        for (&idx, idx_next) in &plan.index_next {
            let pv = alloc();
            idx_to_pv.insert(idx, pv);
            // pv state (element sort).
            prelude.push(Line {
                nid: pv,
                node: Node::State {
                    sort: plan.elem_sort,
                    symbol: Some(format!("spcr_pv_{}_{}", plan.array_nid, idx)),
                },
                immediates: Vec::new(),
                source_line: 0,
            });
            // pv init (broadcast) if the array is initialised.
            if let Some(iv) = plan.init_value {
                tail.push(Line {
                    nid: alloc(),
                    node: Node::Init {
                        sort: plan.elem_sort,
                        state: pv,
                        value: iv,
                    },
                    immediates: Vec::new(),
                    source_line: 0,
                });
            }
            // frame: pv' = ite(GUARD, wdata, pv), GUARD = (waddr == idx') [∧ write_cond].
            // Unconditional/tautological write ⇒ GUARD = eq (index may move). Conditional write ⇒
            // GUARD = write_cond ∧ eq, and the index is stable (idx' = idx) by the plan gate.
            let idx_next_op = idx_next.unwrap_or(Operand(idx));
            let eq = alloc();
            tail.push(Line {
                nid: eq,
                node: Node::Op {
                    sort: bool_sort,
                    op: Op::Eq,
                    args: vec![plan.waddr, idx_next_op],
                    symbol: None,
                },
                immediates: Vec::new(),
                source_line: 0,
            });
            let guard = match plan.write_cond {
                None => Operand(eq),
                Some(cond) => {
                    let and = alloc();
                    tail.push(Line {
                        nid: and,
                        node: Node::Op {
                            sort: bool_sort,
                            op: Op::And,
                            args: vec![cond, Operand(eq)],
                            symbol: None,
                        },
                        immediates: Vec::new(),
                        source_line: 0,
                    });
                    Operand(and)
                }
            };
            let ite = alloc();
            tail.push(Line {
                nid: ite,
                node: Node::Op {
                    sort: plan.elem_sort,
                    op: Op::Ite,
                    args: vec![guard, plan.wdata, Operand(pv)],
                    symbol: None,
                },
                immediates: Vec::new(),
                source_line: 0,
            });
            tail.push(Line {
                nid: alloc(),
                node: Node::Next {
                    sort: plan.elem_sort,
                    state: pv,
                    value: Operand(ite),
                },
                immediates: Vec::new(),
                source_line: 0,
            });
        }
        for &(r, idx) in &plan.reads {
            read_to_pv.insert(r, idx_to_pv[&idx]);
        }
    }

    // Rewrite: references to a dropped read node → its prophecy register (sign-preserving).
    let remap = |o: Operand| -> Operand {
        match read_to_pv.get(&o.nid()) {
            Some(&pv) => Operand(if o.0 < 0 { -pv } else { pv }),
            None => o,
        }
    };
    let mut out: Vec<Line> = Vec::with_capacity(file.lines.len() + prelude.len() + tail.len());
    out.extend(prelude); // pv states (+ maybe the bool sort) — only forward-ref a SORT (nid-resolved)
    for line in &file.lines {
        if drop_nodes.contains(&line.nid) {
            continue;
        }
        out.push(Line {
            nid: line.nid,
            node: remap_node(&line.node, &remap),
            immediates: line.immediates.clone(),
            source_line: line.source_line,
        });
    }
    out.extend(tail); // eq / ite / next-pv / init — all backward refs
    let by_nid = out.iter().enumerate().map(|(i, l)| (l.nid, i)).collect();
    Some(Btor2File { lines: out, by_nid })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Clean P-A1 fixture: full-width write `mem[waddr]=wdata`, latched index
    /// `key' ∈ {key, waddr}`, recovery gated on `mem[key]==3`; `AG EF(busy==0)` HOLDS.
    const AGR_SPCR: &str = "\
1 sort bitvec 1
2 sort bitvec 2
3 sort array 2 2
4 input 1 start
5 input 2 waddr
6 input 2 wdata
7 state 1 busy
8 state 2 key
9 state 3 mem
10 const 1 0
11 init 1 7 10
12 const 2 00
13 init 2 8 12
14 const 2 11
15 read 2 9 8
16 eq 1 15 14
17 not 1 7
18 and 1 4 17
19 const 1 1
20 and 1 7 16
21 ite 1 20 10 7
22 ite 1 18 19 21
23 next 1 7 22
24 ite 2 18 5 8
25 next 2 8 24
26 write 3 9 5 6
27 next 3 9 26
";

    fn has_array(file: &Btor2File) -> bool {
        file.lines.iter().any(|l| {
            matches!(
                &l.node,
                Node::State { sort, .. } if matches!(sort_of(file, *sort), Some(Sort::Array { .. }))
            )
        })
    }

    #[test]
    fn spcr_eliminates_the_array_on_the_clean_shape() {
        let file = crate::adapter::btor2::parser::parse(AGR_SPCR).expect("parse");
        assert!(has_array(&file), "fixture must have an array");
        let out = super::spcr(&file).expect("SPCR applies to the clean shape");
        assert!(!has_array(&out), "SPCR output must be array-free");
        // The read `mem[key]` must be gone; a prophecy register must exist.
        assert!(
            !out.lines
                .iter()
                .any(|l| matches!(&l.node, Node::Op { op: Op::Read, .. })),
            "no array reads should remain"
        );
        assert!(
            out.lines.iter().any(|l| matches!(&l.node, Node::State { symbol: Some(s), .. } if s.starts_with("spcr_pv_"))),
            "a prophecy register must be synthesized"
        );
    }

    #[test]
    fn spcr_decides_array_gated_recovery_holds_via_exact() {
        use crate::adapter::btor2::symbolic_bitblast::{ExactVerdict, exact_symbolic_verdict};
        use crate::mu_calculus::parser as mu_parser;
        let file = crate::adapter::btor2::parser::parse(AGR_SPCR).expect("parse");
        let f = mu_parser::parse("nu Y. ((mu X. ((busy == 0) || <> X)) && [] Y)").unwrap();
        // Guard: the ORIGINAL (array-bearing) design makes the exact ROBDD SKIP (in-cone array).
        assert!(
            exact_symbolic_verdict(AGR_SPCR, &f).is_err(),
            "original must SKIP on the in-cone array"
        );
        // After SPCR the design is array-free ⇒ the exact ROBDD decides HOLDS.
        let out = super::spcr(&file).expect("SPCR applies");
        let src = crate::adapter::btor2::emit::emit_btor2(&out);
        assert_eq!(
            exact_symbolic_verdict(&src, &f).expect("array-free ⇒ exact decides"),
            ExactVerdict::Holds,
            "SPCR'd design must decide AG EF(busy==0) = Holds (matches registerized oracle)"
        );
    }

    /// 8-bit twin (AW=8/DW=8, 256-cell array): SPCR must still yield exactly ONE prophecy
    /// register (O(#accessed-cells), array-size-INDEPENDENT). Whole-array registerization would be
    /// 256·8 = 2048 array bits (over the exact cap → SKIP); SPCR's `busy(1)+key(8)+pv(8)=17` bits
    /// bit-blast, so the exact ROBDD decides — the scaling that beats registerization.
    const AGR_SPCR_LARGE: &str = "\
1 sort bitvec 1
2 sort bitvec 8
3 sort array 2 2
4 input 1 start
5 input 2 waddr
6 input 2 wdata
7 state 1 busy
8 state 2 key
9 state 3 mem
10 const 1 0
11 init 1 7 10
12 const 2 00000000
13 init 2 8 12
14 const 2 11111111
15 read 2 9 8
16 eq 1 15 14
17 not 1 7
18 and 1 4 17
19 const 1 1
20 and 1 7 16
21 ite 1 20 10 7
22 ite 1 18 19 21
23 next 1 7 22
24 ite 2 18 5 8
25 next 2 8 24
26 write 3 9 5 6
27 next 3 9 26
";

    #[test]
    fn spcr_scales_array_size_independent_one_pv_and_exact_decides() {
        use crate::adapter::btor2::symbolic_bitblast::{ExactVerdict, exact_symbolic_verdict};
        use crate::mu_calculus::parser as mu_parser;
        let file = crate::adapter::btor2::parser::parse(AGR_SPCR_LARGE).expect("parse");
        let out = super::spcr(&file).expect("SPCR applies to the 8-bit clean shape");
        // O(#accessed-cells): exactly ONE prophecy register regardless of the 256-cell array.
        let pvs = out
            .lines
            .iter()
            .filter(|l| matches!(&l.node, Node::State { symbol: Some(s), .. } if s.starts_with("spcr_pv_")))
            .count();
        assert_eq!(
            pvs, 1,
            "one prophecy register per read index, array-size-independent"
        );
        assert!(!has_array(&out), "SPCR output must be array-free");
        let f = mu_parser::parse("nu Y. ((mu X. ((busy == 0) || <> X)) && [] Y)").unwrap();
        // Whole-array registerization (256·8 bits) would exceed the exact cap; SPCR's 17 state
        // bits bit-blast → the exact ROBDD decides Holds.
        let src = crate::adapter::btor2::emit::emit_btor2(&out);
        assert_eq!(
            exact_symbolic_verdict(&src, &f).expect("array-free 17-bit cone ⇒ exact decides"),
            ExactVerdict::Holds,
        );
    }

    /// P-A1b — write-ENABLE mux `mem' = ite(we, write(mem,waddr,wdata), mem)` with a STABLE
    /// (never-moving) config index `key`. Recovery via the env asserting `we ∧ waddr==key ∧
    /// wdata==3`; `AG EF(busy==0)` HOLDS. SPCR frame = `pv' = ite(we ∧ waddr==key, wdata, pv)`.
    const AGR_SPCR_WE: &str = "\
1 sort bitvec 1
2 sort bitvec 2
3 sort array 2 2
4 input 1 start
5 input 2 waddr
6 input 2 wdata
7 input 1 we
8 state 1 busy
9 state 2 key
10 state 3 mem
11 const 1 0
12 init 1 8 11
13 const 2 00
14 init 2 9 13
15 const 2 11
16 read 2 10 9
17 eq 1 16 15
18 not 1 8
19 and 1 4 18
20 const 1 1
21 and 1 8 17
22 ite 1 21 11 8
23 ite 1 19 20 22
24 next 1 8 23
25 write 3 10 5 6
26 ite 3 7 25 10
27 next 3 10 26
";

    #[test]
    fn spcr_pa1b_write_enable_mux_stable_index_decides_holds() {
        use crate::adapter::btor2::symbolic_bitblast::{ExactVerdict, exact_symbolic_verdict};
        use crate::mu_calculus::parser as mu_parser;
        let file = crate::adapter::btor2::parser::parse(AGR_SPCR_WE).expect("parse");
        let out =
            super::spcr(&file).expect("SPCR applies to the write-enable + stable-index shape");
        assert!(!has_array(&out), "SPCR output must be array-free");
        let f = mu_parser::parse("nu Y. ((mu X. ((busy == 0) || <> X)) && [] Y)").unwrap();
        assert!(
            exact_symbolic_verdict(AGR_SPCR_WE, &f).is_err(),
            "the array-bearing design must SKIP on the exact ROBDD"
        );
        let src = crate::adapter::btor2::emit::emit_btor2(&out);
        assert_eq!(
            exact_symbolic_verdict(&src, &f).expect("array-free ⇒ exact decides"),
            ExactVerdict::Holds,
            "write-enable SPCR (guard we ∧ waddr==key) must decide AG EF(busy==0) = Holds"
        );
    }

    /// SOUNDNESS gate: a MOVING index (`key' = ite(arm, waddr, key)`) under a CONDITIONAL write
    /// must ABSTAIN — the index could move to `waddr` while `we` is low, leaving `mem[waddr] ≠ pv`
    /// (the frame would be unsound). `AGR_SPCR_WE` + a move-to-waddr `next` for `key`.
    const AGR_SPCR_WE_MOVING: &str = "\
1 sort bitvec 1
2 sort bitvec 2
3 sort array 2 2
4 input 1 start
5 input 2 waddr
6 input 2 wdata
7 input 1 we
8 state 1 busy
9 state 2 key
10 state 3 mem
11 const 1 0
12 init 1 8 11
13 const 2 00
14 init 2 9 13
15 const 2 11
16 read 2 10 9
17 eq 1 16 15
18 not 1 8
19 and 1 4 18
20 const 1 1
21 and 1 8 17
22 ite 1 21 11 8
23 ite 1 19 20 22
24 next 1 8 23
25 write 3 10 5 6
26 ite 3 7 25 10
27 next 3 10 26
28 ite 2 19 5 9
29 next 2 9 28
";

    #[test]
    fn spcr_pa1b_conditional_write_moving_index_abstains_soundly() {
        let file = crate::adapter::btor2::parser::parse(AGR_SPCR_WE_MOVING).expect("parse");
        assert!(
            super::spcr(&file).is_none(),
            "moving index under a conditional write is unsound to registerize ⇒ SPCR must abstain"
        );
    }

    /// P-A1c — the yosys per-bit-write-ENABLE modeling of a plain full write `mem[waddr] <= wdata`:
    /// `write(mem, waddr, (wdata & allones) | (mem[waddr] & ~allones))`. The `mem[waddr]` read is
    /// DEAD (`& ~allones = & 0`). Without the fold, SPCR abstains (the RMW read's index is the INPUT
    /// `waddr`, not a register). `fold_and_dce` collapses it to a clean `write(mem, waddr, wdata)`
    /// and DCE removes the dead read, so SPCR applies and decides HOLDS.
    const AGR_MASKED: &str = "\
1 sort bitvec 1
2 sort bitvec 2
3 sort array 2 2
4 input 1 start
5 input 2 waddr
6 input 2 wdata
7 state 1 busy
8 state 2 key
9 state 3 mem
10 const 1 0
11 init 1 7 10
12 const 2 00
13 init 2 8 12
14 const 2 11
15 read 2 9 8
16 eq 1 15 14
17 not 1 7
18 and 1 4 17
19 const 1 1
20 and 1 7 16
21 ite 1 20 10 7
22 ite 1 18 19 21
23 next 1 7 22
24 ite 2 18 5 8
25 next 2 8 24
26 read 2 9 5
27 not 2 14
28 and 2 26 27
29 and 2 6 14
30 or 2 29 28
31 write 3 9 5 30
32 next 3 9 31
";

    #[test]
    fn spcr_pa1c_fold_removes_dead_rmw_read_then_decides_holds() {
        use crate::adapter::btor2::symbolic_bitblast::{ExactVerdict, exact_symbolic_verdict};
        use crate::mu_calculus::parser as mu_parser;
        let file = crate::adapter::btor2::parser::parse(AGR_MASKED).expect("parse");
        // Without folding, the raw shape has the masked-RMW read at an INPUT index → SPCR would
        // see a non-register read index. `fold_and_dce` inside `spcr` collapses it first.
        let out = super::spcr(&file).expect("SPCR applies after the P-A1c fold");
        assert!(!has_array(&out), "SPCR output must be array-free");
        assert!(
            !out.lines
                .iter()
                .any(|l| matches!(&l.node, Node::Op { op: Op::Read, .. })),
            "the dead RMW read must be folded + DCE'd away"
        );
        let f = mu_parser::parse("nu Y. ((mu X. ((busy == 0) || <> X)) && [] Y)").unwrap();
        let src = crate::adapter::btor2::emit::emit_btor2(&out);
        assert_eq!(
            exact_symbolic_verdict(&src, &f).expect("array-free ⇒ exact decides"),
            ExactVerdict::Holds,
        );
    }

    #[test]
    fn spcr_pa1c_fold_is_verdict_preserving_on_masked_vs_clean() {
        // The masked-write fixture and the clean-write fixture (`AGR_SPCR`) are the SAME design —
        // SPCR must give the same array-free structure (both decide Holds via exact).
        use crate::adapter::btor2::symbolic_bitblast::{ExactVerdict, exact_symbolic_verdict};
        use crate::mu_calculus::parser as mu_parser;
        let f = mu_parser::parse("nu Y. ((mu X. ((busy == 0) || <> X)) && [] Y)").unwrap();
        for src in [AGR_MASKED, AGR_SPCR] {
            let file = crate::adapter::btor2::parser::parse(src).expect("parse");
            let out = super::spcr(&file).expect("SPCR applies");
            let emitted = crate::adapter::btor2::emit::emit_btor2(&out);
            assert_eq!(
                exact_symbolic_verdict(&emitted, &f).expect("decides"),
                ExactVerdict::Holds,
            );
        }
    }

    #[test]
    fn spcr_abstains_when_no_array() {
        // A BV-only design: SPCR is a no-op (None) so the caller keeps the original.
        let bv_only = "\
1 sort bitvec 1
2 state 1 busy
3 const 1 0
4 init 1 2 3
5 next 1 2 2
";
        let file = crate::adapter::btor2::parser::parse(bv_only).expect("parse");
        assert!(super::spcr(&file).is_none(), "no array ⇒ SPCR is a no-op");
    }
}
