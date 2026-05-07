//! Isolated benches for CLTS construction (`CltsBuilder`).
//!
//! Measures the builder's allocation / population / finalize cost on
//! deterministic templates, with no composition or evaluation
//! downstream. Anchors EXP-0001 (baseline freeze) and the planned
//! EXP-0004 (drop 20% growth + with_capacity) and EXP-0004cont (CSR
//! adjacency).
//!
//! Fixtures rebuild every iteration (no caching) — the construction
//! itself is the measurement.

use mununu_core::bench_support as common;

use std::time::Duration;

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use mununu_core::test_support::{self, RandomClts};
use std::hint::black_box;

const CHAIN_SIZES: &[usize] = &[1_000, 10_000, 100_000];
const GRID_SIZES: &[(usize, usize)] = &[(32, 32), (64, 64)];
const RANDOM_SIZES: &[usize] = &[256, 1_024];

fn bench_chain(c: &mut Criterion) {
    let mut group = c.benchmark_group("clts_construction/chain");
    group.measurement_time(Duration::from_secs(8));
    group.sample_size(40);
    for &n in CHAIN_SIZES {
        group.throughput(Throughput::Elements(n as u64));
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, &n| {
            b.iter(|| {
                let c = test_support::chain(n, 4);
                black_box(c);
            });
        });
    }
    group.finish();
}

fn bench_grid(c: &mut Criterion) {
    let mut group = c.benchmark_group("clts_construction/grid");
    group.measurement_time(Duration::from_secs(8));
    group.sample_size(40);
    for &(w, h) in GRID_SIZES {
        let states = (w * h) as u64;
        group.throughput(Throughput::Elements(states));
        group.bench_with_input(
            BenchmarkId::from_parameter(format!("{w}x{h}")),
            &(w, h),
            |b, &(w, h)| {
                b.iter(|| {
                    let c = test_support::grid(w, h);
                    black_box(c);
                });
            },
        );
    }
    group.finish();
}

fn bench_random(c: &mut Criterion) {
    let mut group = c.benchmark_group("clts_construction/random_seeded");
    group.measurement_time(Duration::from_secs(8));
    group.sample_size(40);
    for &n in RANDOM_SIZES {
        group.throughput(Throughput::Elements(n as u64));
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, &n| {
            b.iter(|| {
                let c = RandomClts::new(0xC0FFEE)
                    .with_states(n)
                    .with_density(0.10)
                    .with_alphabet(4)
                    .build();
                black_box(c);
            });
        });
    }
    group.finish();
}

fn entry(c: &mut Criterion) {
    common::announce("clts_construction");
    bench_chain(c);
    bench_grid(c);
    bench_random(c);
}

criterion_group!(benches, entry);
criterion_main!(benches);
