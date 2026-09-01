//! Antecedent shadow-register synthesis for SVA `|=>` properties whose
//! antecedent reaches primary inputs (directly or transitively via combinational
//! logic). See [`docs/design/antecedent-shadow-synthesis.md`] for the full design
//! and soundness argument.
//!
//! # Why this exists
//!
//! The exact-symbolic engine leaves primary inputs FREE — they are quantified
//! out per modality step. Correct for state-only atoms; broken for `A |=> C`
//! whose antecedent `A` reads inputs (directly or through combinational logic):
//! `A@N` (antecedent evaluation) and `A@N+1` (the input driving the transition
//! into cycle N+1) share the same physical signal but are decoupled by the
//! per-step quantification, giving spurious verdicts.
//!
//! # The fix
//!
//! At SVA lift time, when `A |=> C` is being turned into a mu-calculus
//! formula, analyse `A`. If `A`'s combinational cone reaches primary inputs
//! (stopping at register boundaries — same walker as the [`super::parser::cone_inputs`]
//! helper), synthesise an **antecedent shadow register** in the BTOR2:
//!
//! - New state cell `_mununu_antshadow_<N>` (fresh unique per synthesised atom).
//! - Sort = A's sort (typically 1-bit Boolean).
//! - `init = 0` — the SVA `|=>` semantics say cycle 0 has no prior antecedent,
//!   so the obligation is trivially satisfied when the shadow reads false.
//! - `next = A_nid` — the shadow samples A each cycle.
//!
//! Rewrite the lifted mu-calculus formula so the antecedent atom references
//! `_mununu_antshadow_<N>` instead of `A`. Feed the augmented BTOR2 + rewritten
//! formula to the exact engine.
//!
//! # Non-goal
//!
//! **This rewrite is not sound as a general engine-internal transformation.**
//! Applying it to `mu Y. (A or <> Y)` (bare `EF A`) would shift semantics by
//! one cycle. The shadow is safe only for the `|=>` shape, which the SVA lift
//! knows about but the engine does not. Callers MUST only pass atoms they know
//! are `|=>` antecedents.

use std::collections::{BTreeMap, HashMap, HashSet};

use super::ast::{Btor2File, ConstValue, Line, Nid, Node, Operand, Sort};
use super::parser::{collect_symbols, cone_inputs, signal_reaches_anonymous_input};

/// Options for [`synthesize_shadows`].
#[derive(Debug, Clone)]
pub struct ShadowSynthOpts {
    /// When `false`, no shadows are synthesised — every input-derived antecedent
    /// atom is refused with [`RefusalReason::UserOptedOut`]. Used by the
    /// `--no-antecedent-shadow` CLI flag and the `antecedent_shadow: false` API
    /// field for debug / differential-oracle purposes.
    pub enabled: bool,
}

impl Default for ShadowSynthOpts {
    fn default() -> Self {
        Self { enabled: true }
    }
}

/// A successfully synthesised shadow — one entry per input-derived antecedent
/// atom that gets a shadow register. Surfaced in the verify-auto report so
/// downstream consumers can see the engine's rewrite.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShadowSynth {
    /// The original antecedent atom (as it appeared in the mu-calc formula).
    pub atom: String,
    /// The synthesised state cell name.
    pub shadow_name: String,
    /// The primary inputs whose combinational cone the atom reaches
    /// (sorted, deduped — determined by [`cone_inputs`]).
    pub source_inputs: Vec<String>,
}

/// An antecedent atom that could not be shadowed — falls through to the
/// engine's Phase A refusal for a definite skip.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AntecedentRefusal {
    pub atom: String,
    pub reason: RefusalReason,
}

/// Why an antecedent atom did not receive a shadow.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RefusalReason {
    /// The atom's sort is wider than 1 bit. SVA `|=>` antecedents are Boolean;
    /// a wider atom is either language misuse or a multi-value pattern the
    /// shadow would misrepresent.
    NonBoolean { width: u32 },
    /// An array/memory sort appears in the atom's cone (BTOR2 memory reads
    /// havoc to free inputs — shadowing over a havoced array defeats the shadow).
    ArrayInCone,
    /// The atom is itself a primary input. Introducing a shadow that samples a
    /// bare input is technically sound but the resulting property is
    /// `AG(prev(A) → C)`, rarely what the author meant. Refuse and let the
    /// author confirm the shape.
    IsPrimaryInput,
    /// The atom's cone reaches an anonymous free input (partial-write havoc
    /// pattern — see [`signal_reaches_anonymous_input`]). Different soundness
    /// posture; shadow-synth does not fix it.
    ReachesAnonymousInput,
    /// The atom name is not present in the BTOR2 symbol tables — cannot be
    /// resolved to a NID for cone analysis.
    NotFound,
    /// The user disabled shadow-synth via `--no-antecedent-shadow` / API opt-out.
    UserOptedOut,
    /// The atom's cone does not reach any primary input — no shadow needed
    /// (a state-only atom, which the exact engine already handles correctly).
    /// Recorded as a refusal only so callers see we considered it; not an error.
    StateOnly,
}

/// The augmented BTOR2 file + the atom rename map + per-atom outcomes.
#[derive(Debug, Clone)]
pub struct ShadowSynthResult {
    /// BTOR2 file with `<state>` / `<init>` / `<next>` lines appended for each
    /// synthesised shadow. All existing NIDs are preserved so any cone / atom
    /// analysis computed on the original file remains valid.
    pub augmented: Btor2File,
    /// Original atom name → synthesised shadow name. Empty when no atom
    /// qualified for a shadow. Callers must apply this rename to the mu-calc
    /// formula before handing the pair to the engine.
    pub renames: BTreeMap<String, String>,
    /// One entry per successful shadow — for reporting.
    pub shadows: Vec<ShadowSynth>,
    /// One entry per atom that fell through (including `StateOnly` no-ops).
    pub refused: Vec<AntecedentRefusal>,
}

/// For each `atom` in `antecedent_atoms` whose combinational cone reaches
/// primary inputs, synthesise a shadow state cell in the returned BTOR2 file
/// and record the rename. Atoms that cannot be shadowed (see [`RefusalReason`])
/// appear in `refused` and are left unrewritten — the caller is expected to
/// let them fall through to the engine's Phase A refusal.
pub fn synthesize_shadows(
    file: &Btor2File,
    antecedent_atoms: &[String],
    opts: ShadowSynthOpts,
) -> ShadowSynthResult {
    // Fast path: opt-out.
    if !opts.enabled {
        return ShadowSynthResult {
            augmented: file.clone(),
            renames: BTreeMap::new(),
            shadows: vec![],
            refused: antecedent_atoms
                .iter()
                .map(|a| AntecedentRefusal {
                    atom: a.clone(),
                    reason: RefusalReason::UserOptedOut,
                })
                .collect(),
        };
    }

    // Name → NID resolution. `collect_symbols` covers Input, State, and
    // Op-symbol aliases that resolve to a state. We additionally walk
    // `Node::Output` symbols pointing at combinational signals (which
    // `collect_symbols` intentionally skips — see the discussion in
    // symbolic_bitblast.rs at the transitive-refusal guard).
    let mut name_to_nid: HashMap<String, Nid> = collect_symbols(file)
        .into_iter()
        .map(|(nid, name)| (name, nid))
        .collect();
    for line in &file.lines {
        if let Node::Output {
            symbol: Some(s),
            signal,
        } = &line.node
        {
            name_to_nid.entry(s.clone()).or_insert(signal.nid());
        }
    }

    // Primary-input name set for the exact-name refusal case.
    let input_names: HashSet<String> = file
        .inputs()
        .filter_map(|l| match &l.node {
            Node::Input {
                symbol: Some(s), ..
            } => Some(s.clone()),
            _ => None,
        })
        .collect();

    let mut shadows: Vec<ShadowSynth> = Vec::new();
    let mut refused: Vec<AntecedentRefusal> = Vec::new();
    let mut renames: BTreeMap<String, String> = BTreeMap::new();
    let mut new_lines: Vec<Line> = Vec::new();

    // Bookkeeping for NID allocation and shadow numbering.
    let mut next_nid: Nid = file.lines.iter().map(|l| l.nid).max().unwrap_or(0) + 1;
    let mut shadow_counter: u32 = 0;

    // A shared 1-bit `zero` const NID for shadow inits. Reused across all
    // shadows to keep the emitted BTOR2 minimal. Lazily allocated on first
    // shadow; None until we need one.
    let mut zero_const_nid: Option<(Nid, Nid)> = None; // (const_nid, sort_nid)

    for atom in antecedent_atoms {
        // Case: atom is itself a primary input (deliberate refusal — see
        // RefusalReason::IsPrimaryInput doc).
        if input_names.contains(atom) {
            refused.push(AntecedentRefusal {
                atom: atom.clone(),
                reason: RefusalReason::IsPrimaryInput,
            });
            continue;
        }
        // Case: name not resolvable.
        let Some(&atom_nid) = name_to_nid.get(atom) else {
            refused.push(AntecedentRefusal {
                atom: atom.clone(),
                reason: RefusalReason::NotFound,
            });
            continue;
        };
        // Case: cone reaches an anonymous free input (partial-write havoc).
        if signal_reaches_anonymous_input(file, atom) {
            refused.push(AntecedentRefusal {
                atom: atom.clone(),
                reason: RefusalReason::ReachesAnonymousInput,
            });
            continue;
        }
        // Cone analysis: which named primary inputs does the atom reach?
        let source_inputs = cone_inputs(file, atom_nid);
        if source_inputs.is_empty() {
            // State-only atom — no shadow needed.
            refused.push(AntecedentRefusal {
                atom: atom.clone(),
                reason: RefusalReason::StateOnly,
            });
            continue;
        }
        // Width / array check on the atom's sort.
        let sort_nid = match sort_of_signal(file, atom_nid) {
            Some(s) => s,
            None => {
                refused.push(AntecedentRefusal {
                    atom: atom.clone(),
                    reason: RefusalReason::NotFound,
                });
                continue;
            }
        };
        let sort_line = file.lookup(sort_nid);
        let Some(sort_line) = sort_line else {
            refused.push(AntecedentRefusal {
                atom: atom.clone(),
                reason: RefusalReason::NotFound,
            });
            continue;
        };
        match &sort_line.node {
            Node::Sort {
                sort: Sort::BitVec { width },
            } => {
                if *width != 1 {
                    refused.push(AntecedentRefusal {
                        atom: atom.clone(),
                        reason: RefusalReason::NonBoolean { width: *width },
                    });
                    continue;
                }
            }
            Node::Sort {
                sort: Sort::Array { .. },
            } => {
                refused.push(AntecedentRefusal {
                    atom: atom.clone(),
                    reason: RefusalReason::ArrayInCone,
                });
                continue;
            }
            _ => {
                refused.push(AntecedentRefusal {
                    atom: atom.clone(),
                    reason: RefusalReason::NotFound,
                });
                continue;
            }
        }
        // Cone-array check: does any node in the cone have an Array sort?
        // (BTOR2 Read/Write nodes over array sorts would have been havoced by
        // the caller if out-of-cone; an in-cone array is soundness-critical.)
        if cone_touches_array(file, atom_nid) {
            refused.push(AntecedentRefusal {
                atom: atom.clone(),
                reason: RefusalReason::ArrayInCone,
            });
            continue;
        }

        // All checks passed — synthesise the shadow.
        // Lazily allocate the 1-bit sort + `zero` const. Both are reused across
        // shadows within one synth pass. We ALSO scan the original file for a
        // pre-existing 1-bit sort and a pre-existing 1-bit zero const so we do
        // not append duplicates (yosys typically emits exactly one of each,
        // and downstream passes prefer minimal BTOR2).
        let (zero_nid, zero_sort_nid) = match zero_const_nid {
            Some(z) => z,
            None => {
                let sort_nid_1bit = existing_1bit_sort_nid(file).unwrap_or_else(|| {
                    let n = next_nid;
                    next_nid += 1;
                    new_lines.push(Line {
                        nid: n,
                        node: Node::Sort {
                            sort: Sort::BitVec { width: 1 },
                        },
                        immediates: vec![],
                        source_line: 0,
                    });
                    n
                });
                let const_nid =
                    existing_1bit_zero_const_nid(file, sort_nid_1bit).unwrap_or_else(|| {
                        let c = next_nid;
                        next_nid += 1;
                        new_lines.push(Line {
                            nid: c,
                            node: Node::Const {
                                sort: sort_nid_1bit,
                                value: ConstValue::Zero,
                            },
                            immediates: vec![],
                            source_line: 0,
                        });
                        c
                    });
                zero_const_nid = Some((const_nid, sort_nid_1bit));
                (const_nid, sort_nid_1bit)
            }
        };
        // The shadow's sort MUST be 1-bit (we width-checked above). Prefer to
        // reuse the atom's own sort NID when it's already a 1-bit sort — this
        // keeps sort-NID reuse tight and matches what yosys typically emits.
        let shadow_sort_nid = if sort_nid == zero_sort_nid {
            sort_nid
        } else {
            zero_sort_nid
        };

        let shadow_name = format!("_mununu_antshadow_{}", shadow_counter);
        shadow_counter += 1;

        // state <sort> <shadow_name>
        let state_nid = next_nid;
        next_nid += 1;
        new_lines.push(Line {
            nid: state_nid,
            node: Node::State {
                sort: shadow_sort_nid,
                symbol: Some(shadow_name.clone()),
            },
            immediates: vec![],
            source_line: 0,
        });
        // init <sort> <state> <zero>
        let init_line_nid = next_nid;
        next_nid += 1;
        new_lines.push(Line {
            nid: init_line_nid,
            node: Node::Init {
                sort: shadow_sort_nid,
                state: state_nid,
                value: Operand(zero_nid),
            },
            immediates: vec![],
            source_line: 0,
        });
        // next <sort> <state> <atom_nid>
        let next_line_nid = next_nid;
        next_nid += 1;
        new_lines.push(Line {
            nid: next_line_nid,
            node: Node::Next {
                sort: shadow_sort_nid,
                state: state_nid,
                value: Operand(atom_nid),
            },
            immediates: vec![],
            source_line: 0,
        });

        renames.insert(atom.clone(), shadow_name.clone());
        shadows.push(ShadowSynth {
            atom: atom.clone(),
            shadow_name,
            source_inputs,
        });
    }

    // Assemble the augmented file. Preserves all original NIDs and appends the
    // new lines; the `by_nid` index is rebuilt.
    let mut augmented_lines = file.lines.clone();
    augmented_lines.extend(new_lines);
    let by_nid: HashMap<Nid, usize> = augmented_lines
        .iter()
        .enumerate()
        .map(|(i, l)| (l.nid, i))
        .collect();
    let augmented = Btor2File {
        lines: augmented_lines,
        by_nid,
    };

    ShadowSynthResult {
        augmented,
        renames,
        shadows,
        refused,
    }
}

/// Look up the sort NID of the signal at `nid`. Follows one level of `Output`
/// indirection (an Output's sort is its signal's sort, not the Output line's).
fn sort_of_signal(file: &Btor2File, nid: Nid) -> Option<Nid> {
    let line = file.lookup(nid)?;
    match &line.node {
        Node::Input { sort, .. }
        | Node::State { sort, .. }
        | Node::Const { sort, .. }
        | Node::Op { sort, .. } => Some(*sort),
        Node::Output { signal, .. } => sort_of_signal(file, signal.nid()),
        _ => None,
    }
}

/// Return the NID of an existing `sort bitvec 1` declaration, if any.
fn existing_1bit_sort_nid(file: &Btor2File) -> Option<Nid> {
    for line in &file.lines {
        if let Node::Sort {
            sort: Sort::BitVec { width: 1 },
        } = &line.node
        {
            return Some(line.nid);
        }
    }
    None
}

/// Return the NID of an existing 1-bit const whose value is 0 (either the
/// `zero` keyword or a numeric literal that parses as zero). Lets shadow-synth
/// reuse an already-present zero const instead of appending a duplicate.
fn existing_1bit_zero_const_nid(file: &Btor2File, sort_nid_1bit: Nid) -> Option<Nid> {
    for line in &file.lines {
        if let Node::Const { sort, value } = &line.node
            && *sort == sort_nid_1bit
            && is_zero_const_value(value)
        {
            return Some(line.nid);
        }
    }
    None
}

pub(crate) fn is_zero_const_value(v: &ConstValue) -> bool {
    match v {
        ConstValue::Zero => true,
        ConstValue::One | ConstValue::Ones => false,
        ConstValue::Bin(s) | ConstValue::Hex(s) => s.trim().chars().all(|c| c == '0'),
        ConstValue::Dec(n) => *n == 0,
    }
}

/// Does the combinational cone of `start` touch any node with an Array sort?
/// Uses the same stop-at-register discipline as [`cone_inputs`].
fn cone_touches_array(file: &Btor2File, start: Nid) -> bool {
    let mut seen: HashSet<Nid> = HashSet::new();
    let mut work: Vec<Nid> = vec![start];
    while let Some(nid) = work.pop() {
        if !seen.insert(nid) {
            continue;
        }
        let Some(line) = file.lookup(nid) else {
            continue;
        };
        // Check the node's own sort for arrays.
        if let Some(sort_nid) = sort_of_signal(file, nid)
            && let Some(sort_line) = file.lookup(sort_nid)
            && matches!(
                sort_line.node,
                Node::Sort {
                    sort: Sort::Array { .. }
                }
            )
        {
            return true;
        }
        match &line.node {
            Node::State { .. } | Node::Input { .. } | Node::Const { .. } | Node::Sort { .. } => {}
            Node::Op { args, .. } => {
                for a in args {
                    work.push(a.nid());
                }
            }
            Node::Init { value, .. } | Node::Next { value, .. } => work.push(value.nid()),
            Node::Bad { signal }
            | Node::Constraint { signal }
            | Node::Fair { signal }
            | Node::Output { signal, .. } => work.push(signal.nid()),
            Node::Justice { signals } => {
                for s in signals {
                    work.push(s.nid());
                }
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::super::parser::parse as parse_btor2;
    use super::*;

    /// Positive: an atom `mem_rvalid_mine = mem_rvalid && got` — cone reaches
    /// two primary inputs, no state, 1 bit. Should synthesise a shadow and
    /// return a rename.
    #[test]
    fn synth_shadows_boolean_of_two_inputs() {
        const BTOR2: &str = r#"
1 sort bitvec 1
2 input 1 mem_rvalid
3 input 1 got
4 state 1 q
5 const 1 0
6 init 1 4 5
7 and 1 2 3
8 output 7 mem_rvalid_mine
9 next 1 4 2
"#;
        let file = parse_btor2(BTOR2).expect("parse");
        let result = synthesize_shadows(
            &file,
            &["mem_rvalid_mine".to_string()],
            ShadowSynthOpts::default(),
        );
        assert_eq!(result.shadows.len(), 1, "one shadow synthesised");
        let shadow = &result.shadows[0];
        assert_eq!(shadow.atom, "mem_rvalid_mine");
        assert_eq!(shadow.shadow_name, "_mununu_antshadow_0");
        assert_eq!(shadow.source_inputs, vec!["got", "mem_rvalid"]);
        assert_eq!(
            result.renames.get("mem_rvalid_mine").map(String::as_str),
            Some("_mununu_antshadow_0"),
        );
        assert!(
            result.refused.is_empty(),
            "no fallback: {:?}",
            result.refused
        );
        // Structural: the augmented file has original 9 lines + 3 shadow lines
        // (state / init / next; the 1-bit const and sort are reused). Total 12.
        assert_eq!(result.augmented.lines.len(), 12);
        // The shadow state is a new Node::State with the shadow name.
        let shadow_state = result
            .augmented
            .lines
            .iter()
            .find(|l| matches!(&l.node, Node::State { symbol: Some(s), .. } if s == "_mununu_antshadow_0"))
            .expect("shadow state cell added");
        // Its init is the `zero` const (NID 5).
        let shadow_init = result
            .augmented
            .lines
            .iter()
            .find(|l| matches!(&l.node, Node::Init { state, .. } if *state == shadow_state.nid))
            .expect("shadow init cell added");
        if let Node::Init { value, .. } = &shadow_init.node {
            let init_target = result
                .augmented
                .lookup(value.nid())
                .expect("init target line");
            match &init_target.node {
                Node::Const { value, .. } => assert!(
                    is_zero_const_value(value),
                    "shadow init should point at a zero-valued const, got {:?}",
                    value,
                ),
                other => panic!("shadow init should point at a Const, got {:?}", other),
            }
        }
        // Its next samples the original atom NID (the `and` at NID 7).
        let shadow_next = result
            .augmented
            .lines
            .iter()
            .find(|l| matches!(&l.node, Node::Next { state, .. } if *state == shadow_state.nid))
            .expect("shadow next cell added");
        if let Node::Next { value, .. } = &shadow_next.node {
            assert_eq!(value.nid(), 7, "shadow next samples the `and` at NID 7");
        }
    }

    /// State-only atom (`o = p + 1`): cone reaches no inputs, no shadow needed.
    #[test]
    fn synth_no_shadow_for_state_only_atom() {
        const BTOR2: &str = r#"
1 sort bitvec 4
2 state 1 p
3 one 1
4 add 1 2 3
5 output 4 o
6 next 1 2 4
7 const 1 0000
8 init 1 2 7
"#;
        let file = parse_btor2(BTOR2).expect("parse");
        let result = synthesize_shadows(&file, &["o".to_string()], ShadowSynthOpts::default());
        assert!(result.shadows.is_empty(), "no shadow for state-only atom");
        assert_eq!(result.renames.len(), 0);
        assert_eq!(
            result.refused,
            vec![AntecedentRefusal {
                atom: "o".to_string(),
                reason: RefusalReason::StateOnly,
            }],
        );
        // No mutation of the file.
        assert_eq!(result.augmented.lines.len(), file.lines.len());
    }

    /// Atom that IS a primary input — refuse (author-confirmation case).
    #[test]
    fn synth_refuses_bare_primary_input_atom() {
        const BTOR2: &str = r#"
1 sort bitvec 1
2 input 1 clr
3 state 1 q
4 const 1 0
5 init 1 3 4
6 next 1 3 2
"#;
        let file = parse_btor2(BTOR2).expect("parse");
        let result = synthesize_shadows(&file, &["clr".to_string()], ShadowSynthOpts::default());
        assert!(result.shadows.is_empty());
        assert_eq!(
            result.refused,
            vec![AntecedentRefusal {
                atom: "clr".to_string(),
                reason: RefusalReason::IsPrimaryInput,
            }],
        );
    }

    /// User opt-out: no synthesis regardless of atom eligibility.
    #[test]
    fn synth_opt_out_refuses_all() {
        const BTOR2: &str = r#"
1 sort bitvec 1
2 input 1 mem_rvalid
3 input 1 got
4 state 1 q
5 const 1 0
6 init 1 4 5
7 and 1 2 3
8 output 7 mem_rvalid_mine
9 next 1 4 2
"#;
        let file = parse_btor2(BTOR2).expect("parse");
        let result = synthesize_shadows(
            &file,
            &["mem_rvalid_mine".to_string()],
            ShadowSynthOpts { enabled: false },
        );
        assert!(result.shadows.is_empty());
        assert_eq!(result.refused.len(), 1);
        assert_eq!(result.refused[0].reason, RefusalReason::UserOptedOut);
        assert_eq!(result.augmented.lines.len(), file.lines.len());
    }

    /// Unknown atom name — NotFound refusal.
    #[test]
    fn synth_refuses_unknown_atom() {
        const BTOR2: &str = r#"
1 sort bitvec 1
2 input 1 clr
3 state 1 q
4 const 1 0
5 init 1 3 4
6 next 1 3 2
"#;
        let file = parse_btor2(BTOR2).expect("parse");
        let result = synthesize_shadows(
            &file,
            &["not_a_signal".to_string()],
            ShadowSynthOpts::default(),
        );
        assert!(result.shadows.is_empty());
        assert_eq!(
            result.refused,
            vec![AntecedentRefusal {
                atom: "not_a_signal".to_string(),
                reason: RefusalReason::NotFound,
            }],
        );
    }
}
