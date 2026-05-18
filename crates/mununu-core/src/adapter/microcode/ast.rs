//! Microcode JSON AST — strictly typed deserialisation surface.
//!
//! The microcode adapter ingests a JSON document with the shape laid
//! out in the plan's Part 5.5 (deep-dive on the microcode adapter):
//!
//! ```json
//! {
//!   "name": "store_then_fence_then_load",
//!   "description": "Core 0 store + rw-fence + core 1 load",
//!   "regs":    { "acc": { "width": 32 }, "ptr": { "width": 32 } },
//!   "mem":     { "x": { "kind": "shared", "attr": "cacheable" } },
//!   "interrupts": { "ext_7": { "maskable": true } },
//!   "steps": [
//!     { "id": "entry",
//!       "ops": [{ "op": "write_reg", "reg": "ptr", "value": "0x1000" }],
//!       "next": "issue_store" },
//!     { "id": "issue_store",
//!       "ops": [{ "op": "write_mem", "region": "x", "source_reg": "acc" }],
//!       "next": "sync_barrier" },
//!     { "id": "sync_barrier",
//!       "ops": [{ "op": "fence", "order": "rw" }],
//!       "next": "observe_done" },
//!     { "id": "observe_done",
//!       "ops": [{ "op": "read_mem", "region": "x", "into_reg": "acc" }],
//!       "next": "halt" },
//!     { "id": "halt", "ops": [] }
//!   ],
//!   "__mununu": { "controllable": [], "internal": [] }
//! }
//! ```
//!
//! Unknown fields are ignored via `#[serde(default)]` so authoring
//! tools can ship extensions without breaking the schema check.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Parsed microcode document — top-level shape.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Microcode {
    /// Microprogram name. Used as the emitted CTXDSL automaton name
    /// (sanitised). Required — every microcode source has a name.
    pub name: String,
    /// Optional human-readable description. Surfaced in the emitted
    /// CTXDSL metadata banner.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Declared register file. Keys are register identifiers; values
    /// carry width + optional attributes.
    #[serde(default)]
    pub regs: BTreeMap<String, RegDecl>,
    /// Declared memory regions. Keys are region identifiers; values
    /// carry the sharing tag + optional attributes. Sharing = "shared"
    /// generates rendezvous labels that pair with cache / memory
    /// automata; sharing = "private" stays internal to the microprogram.
    #[serde(default)]
    pub mem: BTreeMap<String, MemRegion>,
    /// Declared interrupt sources. Keys are interrupt identifiers;
    /// values carry the maskable bit + optional attributes.
    #[serde(default)]
    pub interrupts: BTreeMap<String, IrqDecl>,
    /// Ordered step list. The first step is the initial state.
    pub steps: Vec<Step>,
    /// Optional `__mununu` annotation block — overrides controllability
    /// classifications. Same convention as CrewAI / LangGraph adapters.
    #[serde(default, rename = "__mununu", skip_serializing_if = "Option::is_none")]
    pub mununu: Option<MununuAnnotations>,
    /// Extra labels the microcode source declares as controllable
    /// without emitting transitions for them. Useful when a single
    /// microcode source is composed with peers that reference
    /// rendezvous labels this microcode doesn't itself fire — e.g.
    /// a 2-core MESI cache composition where each core's L1 cache
    /// snoops the OTHER core's writes / reads. Without this, the
    /// realiser's legacy mode auto-infers those labels as
    /// controllable on each cache automaton that references them
    /// (the labels appear in transitions), producing a
    /// `DuplicateControllableAlphabet` error. Declaring them here
    /// gives the microcode source sole ownership and resolves the
    /// conflict.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub extra_controllable: Vec<String>,
}

/// Register declaration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegDecl {
    /// Width in bits. Today display-only; the abstraction recipe in
    /// Part 4 of the plan abstracts register values away unless
    /// explicitly tagged shared.
    pub width: u32,
    /// Optional `attr` field — display-only metadata.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attr: Option<String>,
    /// Sharing tag for register-level rendezvous. Default `"private"`.
    /// Shared registers emit rendezvous labels that other automata
    /// (e.g. a register-file model) can synchronise on.
    #[serde(default = "default_private")]
    pub kind: String,
}

/// Memory region declaration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemRegion {
    /// Sharing tag. `"shared"` (rendezvous with the memory automaton)
    /// or `"private"` (internal). Default `"shared"` — most memory
    /// regions in microcode workflows are shared with the bus.
    #[serde(default = "default_shared")]
    pub kind: String,
    /// Optional `attr` (e.g. `"cacheable"`, `"device"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attr: Option<String>,
}

/// Interrupt declaration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IrqDecl {
    /// Whether the source can be masked.
    #[serde(default = "default_true")]
    pub maskable: bool,
    /// Optional `attr`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attr: Option<String>,
}

/// One microcode step. Becomes a CLTS state; the `next` field becomes
/// the outgoing transition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Step {
    /// Step identifier. Must be unique within the microcode document.
    /// Becomes the CTXDSL state name (after sanitisation).
    pub id: String,
    /// Ordered list of effects this step performs. Today the
    /// discipline (Part 5) restricts a step to at most one effect;
    /// the AST allows more for forward compatibility. Each effect
    /// generates one transition.
    #[serde(default)]
    pub ops: Vec<Op>,
    /// Successor step id. `None` marks a terminal step.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next: Option<String>,
}

/// A single microcode side-effect. Tagged union over the five op
/// kinds the v1 adapter understands.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Op {
    /// Write a register. Emits `wr_reg_<reg>` (internal unless the
    /// register's `kind` is `"shared"`).
    WriteReg {
        reg: String,
        /// Display-only value (string so hex literals survive).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        value: Option<String>,
    },
    /// Write to a memory region. Emits `wr_mem_<region>` (or
    /// `wr_mem_<region>_<tag>` when `tag` is set) for shared regions
    /// and `wr_priv_<region>` for private regions.
    WriteMem {
        region: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        source_reg: Option<String>,
        /// Optional discriminator (e.g. core id) folded into the
        /// emitted label. Lets one peripheral / cache distinguish
        /// writes issued by different microcode instances. Ignored
        /// for `kind = "private"` regions (label stays `wr_priv_*`).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tag: Option<String>,
    },
    /// Read from a memory region. Emits `rd_mem_<region>` (or
    /// `rd_mem_<region>_<tag>` when `tag` is set) for shared regions
    /// and `rd_priv_<region>` for private regions.
    ReadMem {
        region: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        into_reg: Option<String>,
        /// Optional discriminator (e.g. core id). Same semantics as
        /// `WriteMem.tag`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tag: Option<String>,
    },
    /// Memory fence / barrier. Emits `fence_<order>` (always shared —
    /// fences are by definition global barriers).
    Fence {
        /// `acq` / `rel` / `rw` / etc. The string is preserved verbatim
        /// in the emitted label (sanitised).
        order: String,
    },
    /// Acknowledge an interrupt. Emits `irq_ack_<source>` (always
    /// shared).
    IrqAck { source: String },
}

/// `__mununu` annotation block — same convention as CrewAI / LangGraph.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MununuAnnotations {
    /// Labels to force into the controllable set.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub controllable: Vec<String>,
    /// Labels to force into the internal set.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub internal: Vec<String>,
    /// Labels to force into the uncontrollable set.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub uncontrollable: Vec<String>,
}

fn default_shared() -> String {
    "shared".to_string()
}

fn default_private() -> String {
    "private".to_string()
}

fn default_true() -> bool {
    true
}
