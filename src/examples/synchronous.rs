//! Synchronous CLTS fixtures inspired by classic controller literature.
//!
//! Each helper returns an immutable [`Clts`](crate::clts::Clts) snapshot that can be
//! composed, analysed, or used in examples/tests.

use crate::clts::{Clts, DefaultLabelIdx, DefaultStateIdx};

/// Toggle controller that flips between `off` and `on` on every clock tick.
///
/// Matches the textbook synchronous Mealy machine used to introduce clocked
/// finite-state control.
pub fn clocked_toggle() -> Clts<DefaultStateIdx, DefaultLabelIdx> {
    let mut builder = Clts::builder();
    builder.state("off").initial("off");
    builder.state("on");

    let tick = builder.labels().intern(["tick"]).expect("label intern");
    builder.transition("off", &[tick], "on");
    builder.transition("on", &[tick], "off");

    builder.build().expect("toggle builds")
}

/// Three-phase traffic light where all lanes advance synchronously on a timer.
///
/// States cycle RED → GREEN → YELLOW → RED as in synchronous signal control.
pub fn traffic_light_controller() -> Clts<DefaultStateIdx, DefaultLabelIdx> {
    let mut builder = Clts::builder();
    builder.state("red").initial("red");
    builder.state("green");
    builder.state("yellow");

    let timer = builder.labels().intern(["timer"]).expect("label");
    builder.transition("red", &[timer], "green");
    builder.transition("green", &[timer], "yellow");
    builder.transition("yellow", &[timer], "red");

    builder.with_variables("red", ["stop"]);
    builder.with_variables("green", ["go"]);
    builder.with_variables("yellow", ["prepare"]);

    builder.build().expect("traffic light builds")
}

/// Elevator controller following synchronous reactive design: `call` and `arrive`
/// events fire atomically within the same control cycle.
pub fn elevator_controller() -> Clts<DefaultStateIdx, DefaultLabelIdx> {
    let mut builder = Clts::builder();
    builder.state("idle").initial("idle");
    builder.state("moving");
    builder.state("door_open");

    let dispatch = builder
        .labels()
        .intern(["call", "dispatch"])
        .expect("label");
    let arrive = builder.labels().intern(["arrive"]).expect("label");
    let dwell = builder.labels().intern(["dwell"]).expect("label");

    builder.transition("idle", &[dispatch], "moving");
    builder.transition("moving", &[arrive], "door_open");
    builder.transition("door_open", &[dwell], "idle");

    builder.with_variables("door_open", ["doors_open"]);
    builder.with_variables("moving", ["in_motion"]);

    builder.build().expect("elevator builds")
}

/// Synchronous bus arbiter granting exclusive access when request and tick co-occur.
pub fn synchronous_bus_arbiter() -> Clts<DefaultStateIdx, DefaultLabelIdx> {
    let mut builder = Clts::builder();
    builder.state("idle").initial("idle");
    builder.state("grant");

    let req_tick = builder.labels().intern(["req", "tick"]).expect("label");
    let release = builder.labels().intern(["release", "tick"]).expect("label");

    builder.transition("idle", &[req_tick], "grant");
    builder.transition("grant", &[release], "idle");

    builder.with_variables("grant", ["bus_busy"]);

    builder.build().expect("arbiter builds")
}

/// Two-stage synchronous pipeline: stage A produces data, stage B consumes in lockstep.
///
/// The label set `{produce, consume}` models a lock-step barrier common in synchronous
/// dataflow pipelines.
pub fn synchronous_pipeline() -> Clts<DefaultStateIdx, DefaultLabelIdx> {
    let mut builder = Clts::builder();
    builder.state("empty").initial("empty");
    builder.state("full");

    let fill = builder
        .labels()
        .intern(["produce", "consume"])
        .expect("label");
    let flush = builder.labels().intern(["flush"]).expect("label");

    builder.transition("empty", &[fill], "full");
    builder.transition("full", &[flush], "empty");

    builder.with_variables("full", ["stage_loaded"]);

    builder.build().expect("pipeline builds")
}
