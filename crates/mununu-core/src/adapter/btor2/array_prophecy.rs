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

/// P-A1d — an index LEAF: a state cell (a moving index, tracked by its current value) or a
/// constant index. SPCR registerizes one prophecy register `pv = mem[Cell]` per distinct cell.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Cell {
    State(Nid),
    Const(u64),
}

/// P-A1d — the recursive array-elimination context for ONE array. Registerizes `pv_L = mem[L]`
/// for every index leaf `L` reachable from a read index or (transitively) the next-value of a
/// registerized index-state. A read `mem[E]` is reconstructed as [`Elim::read_at`] (the index
/// `ite`-tree rebuilt over the `pv`s); each frame `pv_L' = mem'[L']` is built by
/// [`Elim::mem_next_at`], substituting `mem'[leaf]` recursively. `ok` is cleared (⇒ SPCR abstains)
/// on any leaf that is not a state / const / (the write address under an UNCONDITIONAL write) —
/// e.g. an input-indexed read or an RMW read in a next-value. This subsumes P-A1/A1b/A1c (the
/// single-state-leaf special case) and adds reset-mux `ite(rst, key, 0)` indices (P-A1d).
struct Elim<'a> {
    file: &'a Btor2File,
    array_nid: Nid,
    elem_sort: Nid,
    bool_sort: Nid,
    waddr: Operand,
    wdata: Operand,
    /// `None` = unconditional write (so `mem'[waddr] = wdata`); `Some(cond)` = write-enable
    /// (guards each cell frame, and forbids a `waddr` leaf in a next-value — that would need the
    /// un-registerizable `mem[waddr]`).
    write_cond: Option<Operand>,
    /// Broadcast init value (a const operand) if the array is initialised; else pv is free at reset.
    init_value: Option<Operand>,
    next_nid: &'a mut Nid,
    new_nodes: &'a mut Vec<Line>,
    const_of: &'a mut HashMap<(u64, Nid), Nid>,
    pv_of: HashMap<Cell, Nid>,
    frame_done: HashSet<Cell>,
    ok: bool,
    depth: u32,
}

impl Elim<'_> {
    /// A hard recursion bound (index `ite`-trees are shallow; a runaway ⇒ abstain).
    const DEPTH_CAP: u32 = 64;

    fn alloc(&mut self) -> Nid {
        let n = *self.next_nid;
        *self.next_nid += 1;
        n
    }

    fn push(&mut self, node: Node) -> Nid {
        let n = self.alloc();
        self.new_nodes.push(Line {
            nid: n,
            node,
            immediates: Vec::new(),
            source_line: 0,
        });
        n
    }

    fn const_node(&mut self, v: u64) -> Nid {
        if let Some(&n) = self.const_of.get(&(v, self.elem_sort)) {
            return n;
        }
        let n = self.push(Node::Const {
            sort: self.elem_sort,
            value: crate::adapter::btor2::ast::ConstValue::Dec(v as i128),
        });
        self.const_of.insert((v, self.elem_sort), n);
        n
    }

    /// The prophecy register for a cell (lazily created, with the array's broadcast init if any).
    fn pv_for(&mut self, cell: Cell) -> Nid {
        if let Some(&n) = self.pv_of.get(&cell) {
            return n;
        }
        let symbol = match cell {
            Cell::State(s) => format!("spcr_pv_{}_{}", self.array_nid, s),
            Cell::Const(c) => format!("spcr_pv_{}_c{}", self.array_nid, c),
        };
        let pv = self.push(Node::State {
            sort: self.elem_sort,
            symbol: Some(symbol),
        });
        self.pv_of.insert(cell, pv);
        if let Some(iv) = self.init_value {
            self.push(Node::Init {
                sort: self.elem_sort,
                state: pv,
                value: iv,
            });
        }
        pv
    }

    /// The index VALUE operand of a cell (for `waddr == cell`): a state's current value, or a const.
    fn cell_value(&mut self, cell: Cell) -> Operand {
        match cell {
            Cell::State(s) => Operand(s),
            Cell::Const(c) => Operand(self.const_node(c)),
        }
    }

    /// The write guard for a cell: `(waddr == cell)` [∧ write_cond].
    fn write_guard(&mut self, cell: Cell) -> Operand {
        let idxval = self.cell_value(cell);
        let eq = self.push(Node::Op {
            sort: self.bool_sort,
            op: Op::Eq,
            args: vec![self.waddr, idxval],
            symbol: None,
        });
        match self.write_cond {
            None => Operand(eq),
            Some(cond) => Operand(self.push(Node::Op {
                sort: self.bool_sort,
                op: Op::And,
                args: vec![cond, Operand(eq)],
                symbol: None,
            })),
        }
    }

    /// `mem'[cell]` = `ite((waddr == cell)[∧cond], wdata, pv_cell)`.
    fn leaf_frame(&mut self, cell: Cell) -> Operand {
        let g = self.write_guard(cell);
        let pv = self.pv_for(cell);
        Operand(self.push(Node::Op {
            sort: self.elem_sort,
            op: Op::Ite,
            args: vec![g, self.wdata, Operand(pv)],
            symbol: None,
        }))
    }

    /// See through a width-preserving `uext`/`sext(x, 0)` alias to the underlying node (yosys puts
    /// the visible name on such a copy — e.g. `u.key`).
    fn see_through(&self, nid: Nid) -> Nid {
        if let Some(l) = self.file.lookup(nid)
            && let Node::Op { op, args, .. } = &l.node
            && matches!(op, Op::Uext | Op::Sext)
            && l.immediates.first().copied() == Some(0)
            && let Some(a) = args.first()
        {
            return self.see_through(a.nid());
        }
        nid
    }

    /// The CURRENT-cycle value read at index expression `E`, rebuilt over the `pv`s. Clears `ok`
    /// on a leaf that cannot be registerized (an input index / a complex op).
    fn read_at(&mut self, nid: Nid) -> Operand {
        if !self.ok || self.depth > Self::DEPTH_CAP {
            self.ok = false;
            return Operand(0);
        }
        let nid = self.see_through(nid);
        if let Some(c) = crate::adapter::btor2::bit_blast::resolve_btor2_constant(self.file, nid) {
            return Operand(self.pv_for(Cell::Const(c)));
        }
        match self.file.lookup(nid).map(|l| &l.node) {
            Some(Node::State { .. }) => Operand(self.pv_for(Cell::State(nid))),
            Some(Node::Op {
                op: Op::Ite, args, ..
            }) if args.len() == 3 => {
                let (c, a, b) = (args[0], args[1].nid(), args[2].nid());
                self.depth += 1;
                let ra = self.read_at(a);
                let rb = self.read_at(b);
                self.depth -= 1;
                Operand(self.push(Node::Op {
                    sort: self.elem_sort,
                    op: Op::Ite,
                    args: vec![c, ra, rb],
                    symbol: None,
                }))
            }
            _ => {
                self.ok = false;
                Operand(0)
            }
        }
    }

    /// `mem'[value-of(E)]` — the NEXT-cycle memory content at index expression `E`. Substitutes
    /// `mem'[leaf]` recursively: a state/const leaf → [`Elim::leaf_frame`]; the write address (under
    /// an unconditional write) → `wdata`; an `ite` → the reconstructed `ite`. Clears `ok` otherwise.
    fn mem_next_at(&mut self, nid: Nid) -> Operand {
        if !self.ok || self.depth > Self::DEPTH_CAP {
            self.ok = false;
            return Operand(0);
        }
        let nid = self.see_through(nid);
        if nid == self.waddr.nid() {
            if self.write_cond.is_none() {
                return self.wdata;
            }
            self.ok = false;
            return Operand(0);
        }
        if let Some(c) = crate::adapter::btor2::bit_blast::resolve_btor2_constant(self.file, nid) {
            return self.leaf_frame(Cell::Const(c));
        }
        match self.file.lookup(nid).map(|l| &l.node) {
            Some(Node::State { .. }) => self.leaf_frame(Cell::State(nid)),
            Some(Node::Op {
                op: Op::Ite, args, ..
            }) if args.len() == 3 => {
                let (c, a, b) = (args[0], args[1].nid(), args[2].nid());
                self.depth += 1;
                let ma = self.mem_next_at(a);
                let mb = self.mem_next_at(b);
                self.depth -= 1;
                Operand(self.push(Node::Op {
                    sort: self.elem_sort,
                    op: Op::Ite,
                    args: vec![c, ma, mb],
                    symbol: None,
                }))
            }
            // an RMW read in a next-value, or any other op ⇒ cannot registerize soundly.
            _ => {
                self.ok = false;
                Operand(0)
            }
        }
    }
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

/// P-A1d — eliminate ONE array via the recursive [`Elim`], accumulating the fresh prophecy nodes
/// (`new_nodes`), the `read_nid → reconstruction` map, and the drop set. Returns `false` (⇒ SPCR
/// abstains) on any unsound shape: a write-chain / write in the else-branch, an unexpected
/// reference to the array, a direct-read write-enable, or an un-registerizable index leaf.
#[allow(clippy::too_many_arguments)]
fn eliminate_array(
    file: &Btor2File,
    array_nid: Nid,
    next_nid: &mut Nid,
    new_nodes: &mut Vec<Line>,
    const_of: &mut HashMap<(u64, Nid), Nid>,
    bool_sort: Nid,
    read_repl: &mut HashMap<Nid, Operand>,
    drops: &mut HashSet<Nid>,
) -> bool {
    let array_sort_nid = match file.lookup(array_nid).map(|l| &l.node) {
        Some(Node::State { sort, .. }) => *sort,
        _ => return false,
    };
    let Some(Sort::Array { element, .. }) = sort_of(file, array_sort_nid) else {
        return false;
    };
    let elem_sort = element;

    // Write shape: unconditional `Write(array, waddr, wdata)` (P-A1) or a write-ENABLE mux
    // `ite(cond, write, array)` (P-A1b; tautological `cond` collapses to unconditional).
    let Some(next_op) = find_next_value_operand(file, array_nid) else {
        return false;
    };
    let array_next_nid = file.lines.iter().find_map(|l| match &l.node {
        Node::Next { state, .. } if *state == array_nid => Some(l.nid),
        _ => None,
    });
    let (write_nid, mux_nid, write_cond): (Nid, Option<Nid>, Option<Operand>) =
        match file.lookup(next_op.nid()).map(|l| &l.node) {
            Some(Node::Op { op: Op::Write, .. }) => (next_op.nid(), None, None),
            Some(Node::Op {
                op: Op::Ite, args, ..
            }) if args.len() == 3 && args[2].nid() == array_nid => {
                let cond = args[0];
                let eff = if is_tautology(file, cond.nid()) {
                    None
                } else {
                    Some(cond)
                };
                (args[1].nid(), Some(next_op.nid()), eff)
            }
            _ => return false,
        };
    let (waddr, wdata) = match file.lookup(write_nid).map(|l| &l.node) {
        Some(Node::Op {
            op: Op::Write,
            args,
            ..
        }) if args.len() == 3 && args[0].nid() == array_nid => (args[1], args[2]),
        _ => return false,
    };
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

    let reads: Vec<(Nid, Nid)> = file
        .lines
        .iter()
        .filter_map(|l| match &l.node {
            Node::Op {
                op: Op::Read, args, ..
            } if args.len() == 2 && args[0].nid() == array_nid => Some((l.nid, args[1].nid())),
            _ => None,
        })
        .collect();
    if reads.is_empty() {
        return false;
    }
    // A write-enable that is itself an array read would dangle when the read is dropped; abstain.
    if let Some(cond) = write_cond
        && reads.iter().any(|&(r, _)| r == cond.nid())
    {
        return false;
    }
    // No OTHER reference to the array beyond the reads + the write + the enable-mux + next + init.
    let mut expected: HashSet<Nid> = reads.iter().map(|&(r, _)| r).collect();
    expected.insert(write_nid);
    for n in [mux_nid, array_next_nid, array_init_nid]
        .into_iter()
        .flatten()
    {
        expected.insert(n);
    }
    for l in &file.lines {
        if !expected.contains(&l.nid) && node_refs_array(&l.node, array_nid) {
            return false;
        }
    }

    let mut elim = Elim {
        file,
        array_nid,
        elem_sort,
        bool_sort,
        waddr,
        wdata,
        write_cond,
        init_value,
        next_nid,
        new_nodes,
        const_of,
        pv_of: HashMap::new(),
        frame_done: HashSet::new(),
        ok: true,
        depth: 0,
    };
    // Reconstruct each read `mem[E]` as the index `ite`-tree over the prophecy registers.
    for &(read_nid, index_nid) in &reads {
        let recon = elim.read_at(index_nid);
        read_repl.insert(read_nid, recon);
    }
    // Build each prophecy register's frame `pv_L' = mem'[L']` (fixpoint — a frame may pull in more
    // cells via `mem_next_at`).
    let mut worklist: Vec<Cell> = elim.pv_of.keys().copied().collect();
    while let Some(cell) = worklist.pop() {
        if !elim.frame_done.insert(cell) {
            continue;
        }
        let frame = match cell {
            // A const index never moves: pv_c' = mem'[c].
            Cell::Const(_) => elim.leaf_frame(cell),
            // A state index: pv_s' = mem'[s'] where s' is the state's next-value (self if none).
            Cell::State(s) => {
                let nn = find_next_value_operand(file, s)
                    .map(|o| o.nid())
                    .unwrap_or(s);
                elim.mem_next_at(nn)
            }
        };
        let pv = elim.pv_of[&cell];
        let n = elim.alloc();
        elim.new_nodes.push(Line {
            nid: n,
            node: Node::Next {
                sort: elem_sort,
                state: pv,
                value: frame,
            },
            immediates: Vec::new(),
            source_line: 0,
        });
        for c in elim.pv_of.keys().copied().collect::<Vec<_>>() {
            if !elim.frame_done.contains(&c) {
                worklist.push(c);
            }
        }
    }
    if !elim.ok {
        return false;
    }

    drops.insert(array_nid);
    drops.insert(write_nid);
    for n in [mux_nid, array_next_nid, array_init_nid]
        .into_iter()
        .flatten()
    {
        drops.insert(n);
    }
    for &(r, _) in &reads {
        drops.insert(r);
    }
    true
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
    let mut next_nid: Nid = file.lines.iter().map(|l| l.nid).max().unwrap_or(0) + 1;
    let mut new_nodes: Vec<Line> = Vec::new();
    let mut const_of: HashMap<(u64, Nid), Nid> = HashMap::new();
    let mut read_repl: HashMap<Nid, Operand> = HashMap::new();
    let mut drops: HashSet<Nid> = HashSet::new();

    // A 1-bit sort for the `eq(waddr, idx)` guards (reuse an existing one, else create it).
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
            let n = next_nid;
            next_nid += 1;
            new_nodes.push(Line {
                nid: n,
                node: Node::Sort {
                    sort: Sort::BitVec { width: 1 },
                },
                immediates: Vec::new(),
                source_line: 0,
            });
            n
        });

    // Every array must be soundly eliminable (P-A1d recursive prophecy), else abstain wholesale.
    for &a in &array_states {
        if !eliminate_array(
            file,
            a,
            &mut next_nid,
            &mut new_nodes,
            &mut const_of,
            bool_sort,
            &mut read_repl,
            &mut drops,
        ) {
            return None;
        }
    }

    // Rewrite: references to a dropped read node → its reconstruction (sign-combining).
    let remap = |o: Operand| -> Operand {
        match read_repl.get(&o.nid()) {
            Some(&r) => {
                if (o.0 < 0) ^ (r.0 < 0) {
                    Operand(-r.nid())
                } else {
                    Operand(r.nid())
                }
            }
            None => o,
        }
    };
    // Topological layout: fresh DECLARATIONS (sorts / prophecy states / consts — they reference at
    // most a sort) first, then the original lines (minus drops, remapped), then the fresh
    // COMBINATIONAL/next nodes (in push order = dependency order). This keeps every data operand a
    // backward reference (only sort references may be forward — resolved by NID).
    let (decls, rest): (Vec<Line>, Vec<Line>) = new_nodes.into_iter().partition(|l| {
        matches!(
            l.node,
            Node::Sort { .. } | Node::State { .. } | Node::Const { .. }
        )
    });
    let mut out: Vec<Line> = Vec::with_capacity(file.lines.len() + decls.len() + rest.len());
    out.extend(decls);
    for line in &file.lines {
        if drops.contains(&line.nid) {
            continue;
        }
        out.push(Line {
            nid: line.nid,
            node: remap_node(&line.node, &remap),
            immediates: line.immediates.clone(),
            source_line: line.source_line,
        });
    }
    out.extend(rest);
    let by_nid = out.iter().enumerate().map(|(i, l)| (l.nid, i)).collect();
    // Observability (attribution): report that SPCR fired + its O(#accessed-cells) footprint.
    let pvs = out
        .iter()
        .filter(
            |l| matches!(&l.node, Node::State { symbol: Some(s), .. } if s.starts_with("spcr_pv_")),
        )
        .count();
    tracing::info!(
        arrays = array_states.len(),
        prophecy_registers = pvs,
        "SPCR: eliminated array(s) via selective prophecy-cell registerization"
    );
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

    /// P-A1d — the REAL yosys async-reset lift of `array_gates_recovery.sv` (module _small): the
    /// read index is a reset-mux `ite(rst_n, key, 0)` (node 16), `key`'s next carries reset-value
    /// `const 0` leaves (nodes 35/36), and the write is the masked RMW (nodes 38-45, P-A1c folds
    /// it). The recursive prophecy registerizes BOTH `mem[key]` and `mem[0]` and reconstructs the
    /// read as `ite(rst_n, pv_key, pv_0)`. SPCR must produce an array-free design.
    const AGR_RESETMUX_LIFT: &str = "\
1 sort bitvec 1
2 input 1 clk
3 input 1 rst_n
4 input 1 start
5 sort bitvec 2
6 input 5 waddr
7 input 5 wdata
8 const 1 0
9 state 1
10 ite 1 3 9 8
11 output 10 busy
12 uext 1 10 0 u.busy
13 uext 1 2 0 u.clk
14 const 5 00
15 state 5
16 ite 5 3 15 14
17 uext 5 16 0 u.key
18 uext 1 3 0 u.rst_n
19 uext 1 4 0 u.start
20 uext 5 6 0 u.waddr
21 uext 5 7 0 u.wdata
22 sort array 5 5
23 state 22 u.mem
24 read 5 23 16
25 const 5 11
26 eq 1 24 25
27 ite 1 26 8 10
28 ite 1 10 27 10
29 const 1 1
30 not 1 10
31 and 1 4 30
32 ite 1 31 29 28
33 ite 1 3 32 8
34 next 1 9 33
35 ite 5 31 6 16
36 ite 5 3 35 14
37 next 5 15 36
38 read 5 23 6
39 not 5 25
40 and 5 38 39
41 and 5 7 25
42 or 5 41 40
43 write 22 23 6 42
44 redor 1 25
45 ite 22 44 43 23
46 next 22 23 45
";

    #[test]
    fn spcr_pa1d_real_async_reset_lift_decides_holds_e2e() {
        // End-to-end through the wired recoverability path (which runs SPCR): the REAL async-reset
        // yosys lift decides Holds — the DIFFERENTIAL ORACLE (matches the whole-array-registerized
        // ROBDD, which independently gives Holds). Was `unknown` before P-A1d.
        use crate::verdict::PropertyVerdict;
        assert_eq!(
            crate::adapter::recoverability::verify_recoverability_scalable(
                AGR_RESETMUX_LIFT,
                "busy == 0",
                &[],
            )
            .expect("decides"),
            PropertyVerdict::Holds,
        );
    }

    #[test]
    fn spcr_pa1d_resetmux_lift_eliminates_array_two_cells() {
        let file = crate::adapter::btor2::parser::parse(AGR_RESETMUX_LIFT).expect("parse");
        let out = super::spcr(&file).expect("SPCR (P-A1c fold + P-A1d recursive prophecy) applies");
        assert!(
            !has_array(&out),
            "the async-reset lift must become array-free"
        );
        assert!(
            !out.lines
                .iter()
                .any(|l| matches!(&l.node, Node::Op { op: Op::Read, .. })),
            "no array reads remain (reconstructed over prophecy registers)"
        );
        // The reset-mux index `ite(rst_n, key, 0)` has TWO leaves ⇒ two prophecy cells (mem[key], mem[0]).
        let pvs = out
            .lines
            .iter()
            .filter(|l| matches!(&l.node, Node::State { symbol: Some(s), .. } if s.starts_with("spcr_pv_")))
            .count();
        assert_eq!(pvs, 2, "reset-mux index registerizes mem[key] AND mem[0]");
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
