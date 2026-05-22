use bitvec::prelude::*;
use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use std::hint::black_box;

use mununu_core::clts::{Clts, DefaultLabelIdx, DefaultStateIdx};
use mununu_core::mu_calculus::{
    Environment, EvaluationOptions, evaluate_tri_with_options, evaluate_with_options, parser,
};

const FIXPOINT_FORMULA: &str = "nu Goal. mu Safe. ([ ( labels = {tick}, ctrl = controllable ) ] (Safe and < ( labels = {sync}, req_next = {active} ) > Goal) and safe_pred)";

fn build_eval_fixture(state_count: usize, fanout: usize) -> Clts<DefaultStateIdx, DefaultLabelIdx> {
    let mut builder = Clts::builder();
    let mut state_names = Vec::with_capacity(state_count);

    for idx in 0..state_count {
        let name = format!("s{idx}");
        builder.state(name.clone());
        state_names.push(name);
    }

    let initial_state = builder
        .state_id_or_insert(&state_names[0])
        .expect("initial state registered");
    builder.initial_state_id(initial_state);

    let tick = builder.labels().intern(["tick"]).expect("tick label");
    let sync = builder.labels().intern(["sync"]).expect("sync label");
    let ack = builder.labels().intern(["ack"]).expect("ack label");

    for (idx, name) in state_names.iter().enumerate() {
        let state_id = builder
            .state_id_or_insert(name)
            .expect("state registered before transitions");

        if idx % 2 == 0 {
            builder.with_variables_for_state(state_id, ["active"]);
        } else {
            builder.with_variables_for_state(state_id, ["inactive"]);
        }

        for step in 1..=fanout {
            let target_idx = (idx + step) % state_count;
            let target_id = builder
                .state_id_or_insert(&state_names[target_idx])
                .expect("target state registered");

            let labels = if step % 2 == 0 {
                [sync, ack]
            } else {
                [tick, ack]
            };
            builder.transition_ids(state_id, &labels, target_id);
        }
    }

    builder.build().expect("benchmark CLTS builds")
}

fn build_environment(clts: &Clts<DefaultStateIdx, DefaultLabelIdx>) -> Environment {
    let count = clts.state_count();
    let mut safe = BitVec::<usize, Lsb0>::repeat(false, count);
    for idx in 0..count {
        if idx % 5 != 0 {
            safe.set(idx, true);
        }
    }

    Environment::new(count).with_predicate("safe_pred", safe)
}

fn mu_calculus_benchmarks(c: &mut Criterion) {
    let mut group = c.benchmark_group("mu_calculus_evaluate");
    let base_formula = parser::parse(FIXPOINT_FORMULA).expect("benchmark formula parses");
    let options = EvaluationOptions::default();

    for &(state_count, fanout) in &[(2048usize, 4usize), (8192usize, 6usize)] {
        let clts = build_eval_fixture(state_count, fanout);
        let env = build_environment(&clts);
        let formula = base_formula.clone();
        let options = options.clone();

        group.throughput(Throughput::Elements(state_count as u64));
        group.bench_function(
            BenchmarkId::from_parameter(format!("states_{state_count}_fanout_{fanout}")),
            move |b| {
                b.iter(|| {
                    let result = evaluate_with_options(&formula, &clts, &env, &options)
                        .expect("μ-calculus evaluation succeeded");
                    black_box(result);
                });
            },
        );
    }

    group.finish();
}

/// R-A1 — measure 3-valued (trit) evaluator performance with and
/// without sub-expression sharing. Per the §Phase 6 §6.7 anchor for
/// R-A1, the pass-bar is:
///
/// - shared-subexpr fixture: ≥ 30% faster post-memoisation
/// - no-shared fixture:      ≤ 5% regression
///
/// Both formulas use `evaluate_tri_with_options` (the KleeneDomain
/// path); same CLTS + environment as the 2-valued bench above so
/// fixture-size effects are comparable.
const TRIT_SHARED_SUBEXPR_FORMULA: &str =
    // (safe_pred ∧ <tick> active) appears 3 times — pure memo win.
    "((safe_pred and < ( labels = {tick} ) > active) or \
      (safe_pred and < ( labels = {tick} ) > active) or \
      (safe_pred and < ( labels = {tick} ) > active))";

const TRIT_NO_SHARED_FORMULA: &str =
    // Three structurally distinct conjuncts; no shared subterm.
    "((safe_pred and < ( labels = {tick} ) > active) or \
      (inactive and [ ( labels = {sync} ) ] safe_pred) or \
      (active and [ ( labels = {ack} ) ] inactive))";

fn trit_subexpr_sharing_benchmarks(c: &mut Criterion) {
    let mut group = c.benchmark_group("trit_eval_shared_subexpr");
    let options = EvaluationOptions::default();

    let shared = parser::parse(TRIT_SHARED_SUBEXPR_FORMULA).expect("shared formula parses");
    let no_shared = parser::parse(TRIT_NO_SHARED_FORMULA).expect("no-shared formula parses");

    // M.1 / M.2 KMTS-shape proxies — 64 / 512 states with moderate fanout.
    for &(state_count, fanout) in &[(64usize, 3usize), (512usize, 4usize)] {
        let clts = build_eval_fixture(state_count, fanout);
        let env = build_environment(&clts);
        group.throughput(Throughput::Elements(state_count as u64));

        // shared: pre-memo would re-evaluate the same subtree 3×;
        // post-memo evaluates once and reuses.
        {
            let f = shared.clone();
            let opts = options.clone();
            let clts_ref = &clts;
            let env_ref = &env;
            group.bench_function(
                BenchmarkId::new("shared", format!("states_{state_count}_fanout_{fanout}")),
                move |b| {
                    b.iter(|| {
                        let result = evaluate_tri_with_options(&f, clts_ref, env_ref, &opts)
                            .expect("trit eval succeeded");
                        black_box(result);
                    });
                },
            );
        }
        // no_shared: memo cost without payoff — must not regress > 5%.
        {
            let f = no_shared.clone();
            let opts = options.clone();
            let clts_ref = &clts;
            let env_ref = &env;
            group.bench_function(
                BenchmarkId::new("no_shared", format!("states_{state_count}_fanout_{fanout}")),
                move |b| {
                    b.iter(|| {
                        let result = evaluate_tri_with_options(&f, clts_ref, env_ref, &opts)
                            .expect("trit eval succeeded");
                        black_box(result);
                    });
                },
            );
        }
    }

    group.finish();
}

criterion_group!(
    benches,
    mu_calculus_benchmarks,
    trit_subexpr_sharing_benchmarks
);
criterion_main!(benches);
