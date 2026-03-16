use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use std::hint::black_box;

use mununu::clts::{Clts, DefaultLabelIdx, DefaultStateIdx};
use mununu::context::{Context, ContextError};
use mununu::mu_calculus::{Environment, parser};

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

fn build_context(
    state_count: usize,
) -> Result<(Context, Environment, mununu::mu_calculus::Formula), ContextError> {
    let plant = build_line_plant(state_count);
    let context = Context::builder()
        .register_clts("plant", plant)
        .finish_with_checks()?;

    let formula = parser::parse("true").expect("formula parses");
    let env = Environment::new(state_count);
    Ok((context, env, formula))
}

fn controller_benchmarks(c: &mut Criterion) {
    let mut group = c.benchmark_group("controller_synthesis");

    for &(label, states) in &[("small", 64usize), ("large", 4096usize)] {
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
