//! Isolated benches for `minimize_bisimulation`.
//!
//! Loads pre-built fixtures so partition refinement is the only
//! measurement. Anchors EXP-0001 baseline and EXP-0009 (Paige-Tarjan
//! rewrite). The current K-S implementation is naive O(k·m·n); benches
//! here will surface the asymptotic improvement once B1 lands.

use mununu_core::bench_support as common;

use std::time::Duration;

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use mununu_core::composition::minimize::minimize_bisimulation;
use std::hint::black_box;

fn bench_chain_minimal(c: &mut Criterion) {
    // Chains are already minimal under strong bisim; this measures the
    // partition-refinement cost when no merges happen — important for
    // ensuring the "fast path" doesn't regress.
    let mut group = c.benchmark_group("minimization_only/chain_minimal");
    group.measurement_time(Duration::from_secs(8));
    group.sample_size(30);
    let chain = common::fixtures::chain_1k();
    group.bench_function(BenchmarkId::from_parameter("chain_1k"), |b| {
        b.iter(|| {
            let r = minimize_bisimulation(&chain, None);
            let _ = black_box(r);
        });
    });
    group.finish();
}

fn bench_grid_minimal(c: &mut Criterion) {
    let mut group = c.benchmark_group("minimization_only/grid_minimal");
    group.measurement_time(Duration::from_secs(8));
    group.sample_size(20);
    let grid = common::fixtures::grid_32x32();
    group.bench_function(BenchmarkId::from_parameter("grid_32x32"), |b| {
        b.iter(|| {
            let r = minimize_bisimulation(&grid, None);
            let _ = black_box(r);
        });
    });
    group.finish();
}

fn bench_random_redundant(c: &mut Criterion) {
    // Random CLTS is unlikely to be minimal; this exercises the merge
    // path. Density 0.20 produces a graph where some bisim equivalences
    // typically exist.
    let mut group = c.benchmark_group("minimization_only/random_redundant");
    group.measurement_time(Duration::from_secs(8));
    group.sample_size(30);
    let r = common::fixtures::random_512_d20();
    group.bench_function(BenchmarkId::from_parameter("random_512_d20"), |b| {
        b.iter(|| {
            let m = minimize_bisimulation(&r, None);
            let _ = black_box(m);
        });
    });
    group.finish();
}

fn entry(c: &mut Criterion) {
    common::announce("minimization_only");
    bench_chain_minimal(c);
    bench_grid_minimal(c);
    bench_random_redundant(c);
}

criterion_group!(benches, entry);
criterion_main!(benches);
