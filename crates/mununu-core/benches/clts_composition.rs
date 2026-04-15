use std::time::Duration;

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use itoa::Buffer;
use mununu_core::clts::{Clts, DefaultLabelIdx, DefaultStateIdx};
use mununu_core::composition::{CompositionOptions, CompositionSemantics, compose};
use std::hint::black_box;

const LABELS: &[&str] = &["alpha", "beta", "gamma", "delta"];
const VARIABLES: &[&str] = &["x", "y", "z"];
const HEAVY_INTERNAL_LABELS: usize = 10;

/// Builds a cyclic CLTS used by the lighter composition benchmarks.
fn build_chain(
    states: usize,
    fanout: usize,
    labels_per_edge: usize,
) -> Clts<DefaultStateIdx, DefaultLabelIdx> {
    let mut builder = Clts::builder();
    builder
        .reserve_states(states)
        .reserve_transitions(states * fanout);
    let mut state_ids = Vec::with_capacity(states);
    let mut num_buf = Buffer::new();
    debug_assert!(
        VARIABLES.len() >= 3,
        "VARIABLES requires at least 3 entries"
    );
    let variable_sets: [&[&str]; 3] = [&VARIABLES[..1], &VARIABLES[..2], &VARIABLES[..3]];

    for idx in 0..states {
        let digits = num_buf.format(idx);
        let mut name = String::with_capacity(1 + digits.len());
        name.push('s');
        name.push_str(digits);
        let state_id = builder
            .state_with_name(name)
            .expect("state index within id range");
        if idx == 0 {
            builder.initial_state_id(state_id);
        }

        let vars = variable_sets[idx % variable_sets.len()];
        builder.with_variables_for_state(state_id, vars.iter().copied());
        state_ids.push(state_id);
    }

    let label_slice = &LABELS[..labels_per_edge.min(LABELS.len())];
    let edge_label = builder
        .labels()
        .intern(label_slice.iter().copied())
        .expect("label intern succeeds");
    let edge_labels = [edge_label];

    for idx in 0..states {
        let from_id = state_ids[idx];
        for offset in 1..=fanout {
            let to_idx = (idx + offset) % states;
            let to_id = state_ids[to_idx];
            builder.transition_ids(from_id, &edge_labels, to_id);
        }
    }

    builder.build().expect("fixture builds")
}

/// Builds a CLTS with a synchronising label and a series of internal jumps used
/// to stress-test the builder and composition routines.
fn build_heavy_chain(states: usize, fanout: usize) -> Clts<DefaultStateIdx, DefaultLabelIdx> {
    let mut builder = Clts::builder();
    builder
        .reserve_states(states)
        .reserve_transitions(states * fanout);
    let mut state_ids = Vec::with_capacity(states);
    let mut num_buf = Buffer::new();
    debug_assert!(
        VARIABLES.len() >= 3,
        "VARIABLES requires at least 3 entries"
    );
    let variable_sets: [&[&str]; 3] = [&VARIABLES[..1], &VARIABLES[..2], &VARIABLES[..3]];

    for idx in 0..states {
        let digits = num_buf.format(idx);
        let mut name = String::with_capacity(1 + digits.len());
        name.push('q');
        name.push_str(digits);
        let state_id = builder
            .state_with_name(name)
            .expect("state index within id range");
        if idx == 0 {
            builder.initial_state_id(state_id);
        }
        let vars = variable_sets[idx % variable_sets.len()];
        builder.with_variables_for_state(state_id, vars.iter().copied());
        state_ids.push(state_id);
    }

    let sync_label = builder.labels().intern(["sync"]).expect("sync label");
    let mut internal_labels = Vec::new();
    let mut label_buf = Buffer::new();
    for offset in 2..=fanout {
        let digits = label_buf.format(offset);
        let mut label_name = String::with_capacity("internal_".len() + digits.len());
        label_name.push_str("internal_");
        label_name.push_str(digits);
        let id = builder
            .labels()
            .intern([label_name.as_str()])
            .expect("internal label");
        internal_labels.push((offset, id));
    }

    for idx in 0..states {
        let from_id = state_ids[idx];
        let sync_to = state_ids[(idx + 1) % states];
        builder.transition_ids(from_id, &[sync_label], sync_to);
        for &(offset, label_id) in &internal_labels {
            let to_id = state_ids[(idx + offset) % states];
            builder.transition_ids(from_id, &[label_id], to_id);
        }
    }

    builder.build().expect("heavy chain builds")
}

/// Produces a small CLTS that synchronises on `sync` and idles on `local`, used
/// as the partner automaton in heavy/light composition benchmarks.
fn build_small_sync_partner() -> Clts<DefaultStateIdx, DefaultLabelIdx> {
    let mut builder = Clts::builder();
    builder.reserve_states(10).reserve_transitions(20);
    let sync = builder.labels().intern(["sync"]).expect("sync label");
    let local = builder.labels().intern(["local"]).expect("local label");
    let mut state_ids = Vec::with_capacity(10);
    let mut num_buf = Buffer::new();

    for idx in 0..10 {
        let digits = num_buf.format(idx);
        let mut name = String::with_capacity(1 + digits.len());
        name.push('p');
        name.push_str(digits);
        let state_id = builder
            .state_with_name(name)
            .expect("state index within id range");
        if idx == 0 {
            builder.initial_state_id(state_id);
        }
        builder.with_variables_for_state(state_id, ["controller"]);
        state_ids.push(state_id);
    }

    for idx in 0..10 {
        let from_id = state_ids[idx];
        let next = state_ids[(idx + 1) % 10];
        builder.transition_ids(from_id, &[sync], next);
        builder.transition_ids(from_id, &[local], from_id);
    }

    builder.build().expect("small sync partner builds")
}

/// Benchmarks small-to-medium CLTS builds with varying state counts.
fn bench_clts_build(c: &mut Criterion) {
    let mut group = c.benchmark_group("clts_build");
    for &size in &[128_usize, 256, 512] {
        group.throughput(Throughput::Elements(size as u64));
        group.bench_with_input(BenchmarkId::from_parameter(size), &size, |b, &n| {
            b.iter(|| {
                black_box(build_chain(n, 3, 3));
            });
        });
    }
    group.finish();
}

/// Benchmarks construction of a large uniform graph (100k states, fanout 10).
fn bench_clts_build_large(c: &mut Criterion) {
    let mut group = c.benchmark_group("clts_build_large");
    group.sample_size(10);
    group.measurement_time(Duration::from_secs(25));
    let states = 100_000_usize;
    group.throughput(Throughput::Elements(states as u64));
    group.bench_function("states_100k_fanout10", |b| {
        b.iter(|| {
            black_box(build_chain(states, 10, LABELS.len()));
        });
    });
    group.finish();
}

/// Benchmarks the heavier chain builder with synchronising and internal labels.
fn bench_clts_build_heavy(c: &mut Criterion) {
    let mut group = c.benchmark_group("clts_build_heavy");
    let states = 20_000_usize;
    group.sample_size(10);
    group.measurement_time(Duration::from_secs(30));
    group.throughput(Throughput::Elements(states as u64));
    group.bench_function("states_20k_fanout10", |b| {
        b.iter(|| {
            black_box(build_heavy_chain(states, HEAVY_INTERNAL_LABELS));
        });
    });
    group.finish();
}

/// Convenience helper that returns two identically shaped chains for
/// composition benchmarks.
fn compose_fixture(
    size: usize,
) -> (
    Clts<DefaultStateIdx, DefaultLabelIdx>,
    Clts<DefaultStateIdx, DefaultLabelIdx>,
) {
    let left = build_chain(size, 2, 2);
    let right = build_chain(size, 2, 2);
    (left, right)
}

/// Benchmarks composition on smaller fixtures across all supported semantics.
fn bench_composition(c: &mut Criterion) {
    let mut group = c.benchmark_group("clts_composition");
    for &size in &[32_usize, 48, 64] {
        let (left, right) = compose_fixture(size);
        for semantics in [
            CompositionSemantics::Synchronous,
            CompositionSemantics::Asynchronous,
            CompositionSemantics::Superset,
        ] {
            let label = format!("{}-{:?}", size, semantics);
            group.throughput(Throughput::Elements(size as u64));
            group.bench_function(BenchmarkId::new("compose", &label), |b| {
                b.iter(|| {
                    let options = CompositionOptions::new(semantics);
                    let product = compose(&left, &right, &options).expect("composition succeeds");
                    black_box(product);
                });
            });
        }
    }
    group.finish();
}

/// Benchmarks composition between a heavy CLTS and a small synchronous partner.
fn bench_heavy_light_composition(c: &mut Criterion) {
    let mut group = c.benchmark_group("clts_heavy_light_composition");
    group.sample_size(10);
    let large = build_heavy_chain(20_000, HEAVY_INTERNAL_LABELS);
    let small = build_small_sync_partner();

    for semantics in [
        CompositionSemantics::Synchronous,
        CompositionSemantics::Superset,
    ] {
        let label = format!("{:?}", semantics);
        let options = CompositionOptions::new(semantics);
        group.bench_function(BenchmarkId::new("compose", &label), |b| {
            b.iter(|| {
                let product = compose(&large, &small, &options).expect("composition succeeds");
                black_box(product);
            });
        });
    }

    group.finish();
}

criterion_group!(
    benches,
    bench_clts_build,
    bench_clts_build_large,
    bench_clts_build_heavy,
    bench_composition,
    bench_heavy_light_composition
);
criterion_main!(benches);
