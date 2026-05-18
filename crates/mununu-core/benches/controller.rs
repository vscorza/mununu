use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use std::hint::black_box;

use mununu_core::clts::{Clts, DefaultLabelIdx, DefaultStateIdx};
use mununu_core::context::{Context, ContextError};
use mununu_core::mu_calculus::{Environment, parser};

fn build_line_plant(state_count: usize) -> Clts<DefaultStateIdx, DefaultLabelIdx> {
    let mut builder = Clts::builder();
    let label = builder
        .labels()
        .intern(["tick"])
        .expect("label intern succeeds");

    for idx in 0..state_count {
        let name = format!("s{idx}");
        builder.state(name.clone());
        if idx == 0 {
            let id = builder
                .state_id_or_insert(&name)
                .expect("initial state exists");
            builder.initial_state_id(id);
        }
    }

    for idx in 0..state_count {
        let from_name = format!("s{idx}");
        let to_name = format!("s{}", (idx + 1) % state_count);
        let from = builder
            .state_id_or_insert(&from_name)
            .expect("from state exists");
        let to = builder
            .state_id_or_insert(&to_name)
            .expect("to state exists");
        builder.transition_ids(from, &[label], to);
    }

    builder.build().expect("controller benchmark plant builds")
}

/// Build a GR(1)-style formula: `¬GF(Req) ∨ GF(Grant)` — the controller must
/// grant infinitely often whenever requested.  This exercises the full
/// alternating fixpoint evaluation instead of the trivial `true` formula.
fn gr1_formula() -> mununu_core::mu_calculus::Formula {
    parser::parse(
        "((! (nu NuA. ((mu MuA. (Req || <> MuA)) && ([] NuA)))) \
          || (nu NuG. ((mu MuG. (Grant || <> MuG)) && ([] NuG))))",
    )
    .expect("GR(1) formula parses")
}

fn build_context(
    state_count: usize,
) -> Result<(Context, Environment, mununu_core::mu_calculus::Formula), ContextError> {
    let plant = build_line_plant(state_count);
    let n = plant.state_count();

    // Mark even-indexed states as Req, odd-indexed as Grant so both predicates
    // are non-trivial and the fixpoint computation does real work.
    let mut req_bits = bitvec::bitvec![usize, bitvec::order::Lsb0; 0; n];
    let mut grant_bits = bitvec::bitvec![usize, bitvec::order::Lsb0; 0; n];
    for i in 0..n {
        if i % 2 == 0 {
            req_bits.set(i, true);
        } else {
            grant_bits.set(i, true);
        }
    }

    let context = Context::builder()
        .register_clts("plant", plant)
        .finish_with_checks()?;

    let env = Environment::new(n)
        .with_predicate("Req", req_bits)
        .with_predicate("Grant", grant_bits);

    let formula = gr1_formula();
    Ok((context, env, formula))
}

fn controller_benchmarks(c: &mut Criterion) {
    let mut group = c.benchmark_group("controller_synthesis");

    for &(label, states) in &[("small", 1_000usize), ("large", 64_000usize)] {
        let (context, env, formula) = build_context(states).expect("context builds");
        group.throughput(Throughput::Elements(states as u64));
        group.bench_function(BenchmarkId::new(label, states), |b| {
            b.iter(|| {
                let synthesis = context
                    .synthesise_controller("plant", &formula, &env, None)
                    .expect("controller synthesis succeeds");
                black_box(synthesis.controller.state_count());
            });
        });
    }

    group.finish();
}

criterion_group!(benches, controller_benchmarks);
criterion_main!(benches);
