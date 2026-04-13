use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};

use mununu::adapter::xstate::XStateAdapter;
use mununu::adapter::{AdapterOptions, FormatAdapter};
use mununu::context_dsl;

const TRAFFIC_LIGHT: &str = r#"{
    "id": "traffic_light",
    "initial": "green_ns",
    "states": {
        "green_ns": { "on": { "TIMER": "yellow_ns", "PED_REQUEST": "green_ns_ped_waiting" } },
        "green_ns_ped_waiting": { "on": { "TIMER": "yellow_ns" } },
        "yellow_ns": { "on": { "TIMER": "red_ns" } },
        "red_ns": { "on": { "TIMER": "green_ew" } },
        "green_ew": { "on": { "TIMER": "yellow_ew" } },
        "yellow_ew": { "on": { "TIMER": "red_ew" } },
        "red_ew": { "on": { "TIMER": "green_ns" } }
    },
    "__mununu": {
        "controllable": ["TIMER"],
        "uncontrollable": ["PED_REQUEST"],
        "properties": [{ "name": "safety", "formula": "nu X. ([] X)", "role": "guarantee" }]
    }
}"#;

const AUTH_FLOW: &str = r#"{
    "id": "auth_flow",
    "initial": "main",
    "states": {
        "main": {
            "type": "parallel",
            "states": {
                "auth": {
                    "initial": "idle",
                    "states": {
                        "idle": { "on": { "LOGIN": "logging_in" } },
                        "logging_in": { "on": { "MFA_REQUIRED": "mfa_pending", "DENY": "failed" } },
                        "mfa_pending": { "on": { "MFA_CODE": "verifying" } },
                        "verifying": { "on": { "VERIFY": "authed", "DENY": "failed" } },
                        "authed": { "on": { "LOGOUT": "idle" } },
                        "failed": { "on": { "RETRY": "idle" } }
                    }
                },
                "session": {
                    "initial": "none",
                    "states": {
                        "none": { "on": { "START": "active" } },
                        "active": { "on": { "EXPIRE": "expired", "LOGOUT": "none" } },
                        "expired": { "on": { "START": "active" } }
                    }
                }
            }
        }
    },
    "__mununu": {
        "controllable": ["VERIFY", "DENY", "START", "MFA_REQUIRED", "RETRY"],
        "uncontrollable": ["LOGIN", "MFA_CODE", "EXPIRE", "LOGOUT"],
        "properties": [{ "name": "safety", "formula": "nu X. ([] X)", "role": "guarantee" }]
    }
}"#;

fn bench_translate(c: &mut Criterion) {
    let options = AdapterOptions::default();
    let mut group = c.benchmark_group("xstate_translate");

    group.bench_function("traffic_light_7states", |b| {
        b.iter(|| XStateAdapter::translate(TRAFFIC_LIGHT, &options).unwrap())
    });

    group.bench_function("auth_flow_parallel", |b| {
        b.iter(|| XStateAdapter::translate(AUTH_FLOW, &options).unwrap())
    });

    group.finish();
}

fn bench_full_pipeline(c: &mut Criterion) {
    let options = AdapterOptions::default();
    let mut group = c.benchmark_group("xstate_full_pipeline");

    for (name, json) in [("traffic_light", TRAFFIC_LIGHT), ("auth_flow", AUTH_FLOW)] {
        group.bench_with_input(BenchmarkId::from_parameter(name), &json, |b, &json| {
            b.iter(|| {
                let output = XStateAdapter::translate(json, &options).unwrap();
                let doc = context_dsl::parse(&output.ctxdsl).unwrap();
                let realized = context_dsl::realize_context(&doc, &[]).unwrap();

                // Evaluate the first formula on the first automaton
                if let Some(formula) = realized.formulas.values().next() {
                    let names = realized.context.clts_names();
                    if let Some(target) = names.first() {
                        let env = realized.environment_for(target);
                        let _ = realized
                            .context
                            .evaluate_mu(target, &formula.formula, &env, None);
                    }
                }
            })
        });
    }

    group.finish();
}

criterion_group!(benches, bench_translate, bench_full_pipeline);
criterion_main!(benches);
