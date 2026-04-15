//! Asynchronous CLTS fixtures drawn from classic DES and reactive-protocol texts.

use crate::clts::{Clts, DefaultLabelIdx, DefaultStateIdx};

/// Single-slot producer/consumer buffer with independent produce/consume events.
pub fn producer_consumer_buffer() -> Clts<DefaultStateIdx, DefaultLabelIdx> {
    let mut builder = Clts::builder();
    builder.state("empty").initial("empty");
    builder.state("full");

    let produce = builder.labels().intern(["produce"]).expect("label");
    let consume = builder.labels().intern(["consume"]).expect("label");

    builder.transition("empty", &[produce], "full");
    builder.transition("full", &[consume], "empty");

    builder.with_variables("full", ["buffer_full"]);

    builder.build().expect("buffer builds")
}

/// Two-phase handshake channel (req+, ack+, req-, ack-) as in asynchronous interface protocols.
pub fn two_phase_handshake() -> Clts<DefaultStateIdx, DefaultLabelIdx> {
    let mut builder = Clts::builder();
    builder.state("idle").initial("idle");
    builder.state("req_high");
    builder.state("ack_high");
    builder.state("returning");

    let req_plus = builder.labels().intern(["req+"]).expect("label");
    let ack_plus = builder.labels().intern(["ack+"]).expect("label");
    let req_low = builder.labels().intern(["req-"]).expect("label");
    let ack_low = builder.labels().intern(["ack-"]).expect("label");

    builder.transition("idle", &[req_plus], "req_high");
    builder.transition("req_high", &[ack_plus], "ack_high");
    builder.transition("ack_high", &[req_low], "returning");
    builder.transition("returning", &[ack_low], "idle");

    builder.build().expect("handshake builds")
}

/// Peterson's mutual exclusion for two processes expressed as asynchronous CLTS.
pub fn peterson_mutual_exclusion() -> Clts<DefaultStateIdx, DefaultLabelIdx> {
    let mut builder = Clts::builder();
    builder.state("idle").initial("idle");
    builder.state("p0_wait");
    builder.state("p1_wait");
    builder.state("critical");

    let p0_request = builder.labels().intern(["p0_request"]).expect("label");
    let p1_request = builder.labels().intern(["p1_request"]).expect("label");
    let grant0 = builder.labels().intern(["grant0"]).expect("label");
    let grant1 = builder.labels().intern(["grant1"]).expect("label");
    let release = builder.labels().intern(["release"]).expect("label");

    builder.transition("idle", &[p0_request], "p0_wait");
    builder.transition("idle", &[p1_request], "p1_wait");
    builder.transition("p0_wait", &[grant0], "critical");
    builder.transition("p1_wait", &[grant1], "critical");
    builder.transition("critical", &[release], "idle");

    builder.with_variables("critical", ["mutex_held"]);

    builder.build().expect("peterson builds")
}

/// Token ring with three nodes passing an asynchronous token.
pub fn token_ring_three() -> Clts<DefaultStateIdx, DefaultLabelIdx> {
    let mut builder = Clts::builder();
    builder.state("node0").initial("node0");
    builder.state("node1");
    builder.state("node2");

    let pass01 = builder.labels().intern(["pass_0_1"]).expect("label");
    let pass12 = builder.labels().intern(["pass_1_2"]).expect("label");
    let pass20 = builder.labels().intern(["pass_2_0"]).expect("label");

    builder.transition("node0", &[pass01], "node1");
    builder.transition("node1", &[pass12], "node2");
    builder.transition("node2", &[pass20], "node0");

    builder.build().expect("token ring builds")
}

/// Asynchronous producer with bounded buffer and explicit overflow handling.
pub fn bounded_buffer_overflow() -> Clts<DefaultStateIdx, DefaultLabelIdx> {
    let mut builder = Clts::builder();
    builder.state("empty").initial("empty");
    builder.state("one");
    builder.state("two");
    builder.state("overflow");

    let produce = builder.labels().intern(["produce"]).expect("label");
    let consume = builder.labels().intern(["consume"]).expect("label");
    let drop = builder.labels().intern(["drop"]).expect("label");

    builder.transition("empty", &[produce], "one");
    builder.transition("one", &[produce], "two");
    builder.transition("two", &[produce], "overflow");
    builder.transition("overflow", &[drop], "two");
    builder.transition("two", &[consume], "one");
    builder.transition("one", &[consume], "empty");

    builder.with_variables("overflow", ["data_lost"]);

    builder.build().expect("overflow buffer builds")
}
