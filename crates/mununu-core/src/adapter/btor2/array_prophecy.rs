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
//! **P-A1 scope (SOUND, conservative — abstains (returns `None`) otherwise).** For EVERY array
//! state in the design: a single UNCONDITIONAL full-width write `mem' = write(mem, waddr, wdata)`,
//! read only at bare-register indices whose next-value moves only within `{itself, waddr}`
//! (stable / move-to-write-address). Under that shape the frame
//!
//! ```text
//! pv' = ite(waddr == idx', wdata, pv)          (idx' = the index register's next-value)
//! ```
//!
//! reproduces `mem'[idx']` EXACTLY: when `idx' = idx` (held) the else-branch is `mem[idx] = pv`;
//! when `idx' = waddr` (moved to the written cell) the `waddr == idx'` guard is true so it is
//! `wdata = mem'[waddr]`. So SPCR is a verdict-PRESERVING reformulation (not an abstraction): the
//! KMTS of the SPCR'd design has the same may/must edges as the original on the property-relevant
//! projection ⇒ definite νµ verdicts transfer at every alternation depth (Bruns–Godefroid).

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
}

fn sort_of(file: &Btor2File, nid: Nid) -> Option<Sort> {
    match &file.lookup(nid)?.node {
        Node::Sort { sort } => Some(sort.clone()),
        _ => None,
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

    // The array's next MUST be a single unconditional Write(array, waddr, wdata).
    let next_op = find_next_value_operand(file, array_nid)?;
    let array_next_nid = file.lines.iter().find_map(|l| match &l.node {
        Node::Next { state, .. } if *state == array_nid => Some(l.nid),
        _ => None,
    });
    let write_line = file.lookup(next_op.nid())?;
    let (waddr, wdata, write_nid) = match &write_line.node {
        Node::Op {
            op: Op::Write,
            args,
            ..
        } if args.len() == 3 && args[0].nid() == array_nid => (args[1], args[2], write_line.nid),
        // write-mux / chain / non-write next → P-A1 abstains.
        _ => return None,
    };

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
                if n == idx || n == waddr.nid() {
                    continue; // an allowed leaf (hold / move-to-write-address)
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

    // No OTHER reference to the array beyond the reads + the write + next-array + init-array.
    let mut expected: HashSet<Nid> = reads.iter().map(|&(r, _)| r).collect();
    expected.insert(write_nid);
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

/// SPCR entry: if EVERY in-cone array matches the sound P-A1 shape, return the array-FREE
/// equivalent design (prophecy registers + exact frames, arrays dropped); else `None`
/// (the caller keeps the original and falls through to its honest abstain).
///
/// **Engine:** owned BTOR2→BTOR2 rewrite (no solver). The result is consumed by the array-free
/// deciders — `exact-symbolic` ROBDD (small cone) or the `symbolic` predicate-cube (QF_BV must).
pub(crate) fn spcr(file: &Btor2File) -> Option<Btor2File> {
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
            // frame: pv' = ite(eq(waddr, idx'), wdata, pv). A never-written index holds (idx' = idx).
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
            let ite = alloc();
            tail.push(Line {
                nid: ite,
                node: Node::Op {
                    sort: plan.elem_sort,
                    op: Op::Ite,
                    args: vec![Operand(eq), plan.wdata, Operand(pv)],
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
