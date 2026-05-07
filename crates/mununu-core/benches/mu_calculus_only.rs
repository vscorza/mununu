//! Isolated benches for mu-calculus evaluation.
//!
//! Loads pre-built CLTS fixtures and exercises three formula classes:
//!   1. Propositional: bitwise AND/OR/NOT only.
//!   2. Reachability (least fixpoint with diamond): `mu X. (target or <> X)`.
//!   3. Invariance (greatest fixpoint with box): `nu X. (safe and [] X)`.
//!
//! Anchors EXP-0001 baseline and the planned EXP-0002 (iter-rank SoA),
//! EXP-0007 (predicate interning + Vec bindings), EXP-0012 (changed-flag
//! termination), EXP-0014 (modal pre-image CSR), EXP-0015 (parallel
//! modal eval).

use mununu_core::bench_support as common;

use std::time::Duration;

use bitvec::prelude::*;
use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use mununu_core::clts::{Clts, DefaultLabelIdx, DefaultStateIdx};
use mununu_core::context::{Context, ControllerMode, ControllerSynthesisOptions};
use mununu_core::mu_calculus::{Environment, EvaluationOptions, evaluate_with_options, parser};
use std::hint::black_box;

/// Build an environment with two boolean predicates derived deterministically
/// from state index. `safe` true on indices not divisible by 7; `target` true
/// on indices divisible by 13. Different primes so the predicates are not
/// trivially related.
fn env_for(clts: &Clts<DefaultStateIdx, DefaultLabelIdx>) -> Environment {
    let n = clts.state_count();
    let mut safe = BitVec::<usize, Lsb0>::repeat(false, n);
    let mut target = BitVec::<usize, Lsb0>::repeat(false, n);
    for i in 0..n {
        if i % 7 != 0 {
            safe.set(i, true);
        }
        if i % 13 == 0 {
            target.set(i, true);
        }
    }
    Environment::new(n)
        .with_predicate("safe", safe)
        .with_predicate("target", target)
}

fn bench_propositional(c: &mut Criterion) {
    let mut group = c.benchmark_group("mu_calculus_only/propositional");
    group.measurement_time(Duration::from_secs(8));
    group.sample_size(40);
    let formula = parser::parse("safe and (not target)").expect("formula parses");
    let opts = EvaluationOptions::default();
    for (label, clts) in [
        ("chain_1k", common::fixtures::chain_1k()),
        ("grid_32x32", common::fixtures::grid_32x32()),
    ] {
        let env = env_for(&clts);
        group.throughput(Throughput::Elements(clts.state_count() as u64));
        group.bench_function(BenchmarkId::from_parameter(label), |b| {
            b.iter(|| {
                let r = evaluate_with_options(&formula, &clts, &env, &opts).expect("eval");
                let _ = black_box(r);
            });
        });
    }
    group.finish();
}

fn bench_reachability(c: &mut Criterion) {
    let mut group = c.benchmark_group("mu_calculus_only/reachability_mu");
    group.measurement_time(Duration::from_secs(10));
    group.sample_size(30);
    let formula = parser::parse("mu X. (target or <> X)").expect("formula parses");
    let opts = EvaluationOptions::default();
    for (label, clts) in [
        ("chain_1k", common::fixtures::chain_1k()),
        ("ring_1k", common::fixtures::ring_1k()),
        ("grid_32x32", common::fixtures::grid_32x32()),
    ] {
        let env = env_for(&clts);
        group.throughput(Throughput::Elements(clts.state_count() as u64));
        group.bench_function(BenchmarkId::from_parameter(label), |b| {
            b.iter(|| {
                let r = evaluate_with_options(&formula, &clts, &env, &opts).expect("eval");
                let _ = black_box(r);
            });
        });
    }
    group.finish();
}

fn bench_invariance(c: &mut Criterion) {
    let mut group = c.benchmark_group("mu_calculus_only/invariance_nu");
    group.measurement_time(Duration::from_secs(10));
    group.sample_size(30);
    let formula = parser::parse("nu X. (safe and [] X)").expect("formula parses");
    let opts = EvaluationOptions::default();
    for (label, clts) in [
        ("chain_1k", common::fixtures::chain_1k()),
        ("ring_1k", common::fixtures::ring_1k()),
        ("grid_32x32", common::fixtures::grid_32x32()),
    ] {
        let env = env_for(&clts);
        group.throughput(Throughput::Elements(clts.state_count() as u64));
        group.bench_function(BenchmarkId::from_parameter(label), |b| {
            b.iter(|| {
                let r = evaluate_with_options(&formula, &clts, &env, &opts).expect("eval");
                let _ = black_box(r);
            });
        });
    }
    group.finish();
}

/// Synthesis-bound bench. Builds a `Context` with the fixture registered as
/// "M", then calls `synthesise_controller_with_options` with
/// `ControllerMode::ProductGame` against an alternation-2 formula. This is
/// the workload that actually exercises `IterationRanks::record()` (during
/// witness-guided fixpoint evaluation) and `IterationRanks::get_rank()`
/// (during ProductGame controller construction at `context/mod.rs:2034`).
///
/// EXP-0002b uses this bench to test the original ≥2× hypothesis from
/// EXP-0002. EXP-0002a falsified the hypothesis on workloads that don't
/// touch iteration_ranks at all; this bench is the right test target.
fn bench_synthesis(c: &mut Criterion) {
    let mut group = c.benchmark_group("mu_calculus_only/synthesis_product_game");
    // Alternation-2 synthesis is expensive (chain_1k took 13s per call in
    // smoke testing, dominated by linear-chain modal pre-image walks).
    // Limit fixtures to ring_1k + grid_32x32 which converge fast enough
    // for 30+ Criterion samples in a few minutes per side.
    group.measurement_time(Duration::from_secs(30));
    group.sample_size(15);
    // Alternation-2 GR(1)-style formula with two mu-obligations nested
    // inside a nu-invariant. Exercises the rank-record inner loop on every
    // mu-obligation entry per state.
    let formula =
        parser::parse("nu X. ((mu Y1. (target or <> Y1)) and (mu Y2. (safe or <> Y2)) and [] X)")
            .expect("formula parses");
    for (label, clts) in [
        ("ring_1k", common::fixtures::ring_1k()),
        ("grid_32x32", common::fixtures::grid_32x32()),
    ] {
        let env = env_for(&clts);
        let ctx = Context::builder().register_clts("M", clts.clone()).finish();
        group.throughput(Throughput::Elements(clts.state_count() as u64));
        group.bench_function(BenchmarkId::from_parameter(label), |b| {
            b.iter(|| {
                let opts = ControllerSynthesisOptions {
                    evaluation: None,
                    diagnostics: None,
                    minimize: false,
                    extract_strategy: false,
                    mode: ControllerMode::ProductGame,
                };
                let r = ctx.synthesise_controller_with_options("M", &formula, &env, opts);
                let _ = black_box(r);
            });
        });
    }
    group.finish();
}

fn entry(c: &mut Criterion) {
    common::announce("mu_calculus_only");
    bench_propositional(c);
    bench_reachability(c);
    bench_invariance(c);
    bench_synthesis(c);
}

criterion_group!(benches, entry);
criterion_main!(benches);
