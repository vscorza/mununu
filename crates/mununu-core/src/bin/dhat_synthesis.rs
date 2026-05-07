//! dhat-instrumented replay of the EXP-0002b synthesis workload.
//!
//! Builds a `Context` with the EXP-0002b grid_32x32 fixture, runs
//! `Context::synthesise_controller_with_options(... ProductGame ...)`
//! against the alternation-2 GR(1)-style formula a small number of
//! times (default 3), and writes a `dhat-heap.json` profile to the
//! current directory.
//!
//! Build with both feature flags. Required-features in Cargo.toml
//! gate the bin so a stray `cargo build --bins` doesn't try to
//! compile it without the necessary deps.
//!
//! Usage:
//!   cargo run --release --features test_support,dhat --bin dhat_synthesis
//!   # produces dhat-heap.json in cwd; archive into experiments/<EXP-ID>/.
//!
//! Two-call comparison (EXP-0002b-mem):
//!   1. revert SoA → build → run → save as dhat-heap.A.json (HashMap)
//!   2. apply SoA → build → run → save as dhat-heap.B.json (SoA)
//!   3. compare peak_bytes, alloc_count, bytes_read/written across
//!      the two profiles via `dh_view` (online viewer) or by parsing
//!      the JSON directly.
//!
//! Determinism: the fixture is built from `RandomClts::new(seed=0xC0FFEE)`
//! per `bench_support::fixtures::grid_32x32` (which is `test_support::grid(32, 32)`).
//! Re-runs are byte-identical modulo HashMap iteration order, which
//! affects allocation timing but not totals.

use bitvec::prelude::*;

use mununu_core::bench_support;
use mununu_core::context::{Context, ControllerMode, ControllerSynthesisOptions};
use mununu_core::mu_calculus::{Environment, parser};

#[global_allocator]
static ALLOC: dhat::Alloc = dhat::Alloc;

fn env_for(state_count: usize) -> Environment {
    let mut safe = BitVec::<usize, Lsb0>::repeat(false, state_count);
    let mut target = BitVec::<usize, Lsb0>::repeat(false, state_count);
    for i in 0..state_count {
        if i % 7 != 0 {
            safe.set(i, true);
        }
        if i % 13 == 0 {
            target.set(i, true);
        }
    }
    Environment::new(state_count)
        .with_predicate("safe", safe)
        .with_predicate("target", target)
}

fn main() {
    // Profiler must outlive everything below; on Drop it writes
    // `dhat-heap.json` to the current directory.
    let _profiler = dhat::Profiler::new_heap();

    // Load (or build) the EXP-0002b grid_32x32 fixture deterministically.
    // bench_support uses test_support's seeded generators.
    let clts = bench_support::fixtures::grid_32x32();
    let env = env_for(clts.state_count());
    let formula =
        parser::parse("nu X. ((mu Y1. (target or <> Y1)) and (mu Y2. (safe or <> Y2)) and [] X)")
            .expect("formula parses");

    let ctx = Context::builder().register_clts("M", clts.clone()).finish();

    // Run synthesis a few times so the dhat output reflects the
    // steady-state heap profile, not just one-shot startup costs.
    // Three iterations is enough to amortize the fixture-build cost
    // and stabilize the per-call shape.
    for _ in 0..3 {
        let opts = ControllerSynthesisOptions {
            evaluation: None,
            diagnostics: None,
            minimize: false,
            extract_strategy: false,
            mode: ControllerMode::ProductGame,
        };
        let _ = ctx
            .synthesise_controller_with_options("M", &formula, &env, opts)
            .expect("synthesis succeeds");
    }

    // _profiler drops here, writes dhat-heap.json.
}
