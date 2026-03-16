use crate::clts::{Clts, CltsError, DefaultLabelIdx, DefaultStateIdx};

pub fn safety_gate() -> Result<Clts<DefaultStateIdx, DefaultLabelIdx>, CltsError> {
    let mut builder = Clts::builder();
    builder.state("Open");
    builder.state("Closed");

    let open = builder.state_id_or_insert("Open").unwrap();
    let closed = builder.state_id_or_insert("Closed").unwrap();
    builder.initial_state_id(open);

    let gate_close = builder.labels().intern(["gate_closed"])?;
    let gate_open = builder.labels().intern(["gate_open"])?;

    builder.transition_ids(open, &[gate_close], closed);
    builder.transition_ids(closed, &[gate_open], open);

    builder.build()
}

pub fn operator_panel() -> Result<Clts<DefaultStateIdx, DefaultLabelIdx>, CltsError> {
    let mut builder = Clts::builder();
    builder.state("Idle");
    builder.state("Ready");

    let idle = builder.state_id_or_insert("Idle").unwrap();
    let ready = builder.state_id_or_insert("Ready").unwrap();
    builder.initial_state_id(idle);

    let operator_ready = builder.labels().intern(["operator_ready"])?;
    let operator_reset = builder.labels().intern(["operator_reset"])?;

    builder.transition_ids(idle, &[operator_ready], ready);
    builder.transition_ids(ready, &[operator_reset], idle);

    builder.build()
}

pub fn assembly_machine() -> Result<Clts<DefaultStateIdx, DefaultLabelIdx>, CltsError> {
    let mut builder = Clts::builder();
    builder.state("Idle");
    builder.state("Running");

    let idle = builder.state_id_or_insert("Idle").unwrap();
    let running = builder.state_id_or_insert("Running").unwrap();
    builder.initial_state_id(idle);

    let stop = builder.labels().intern(["stop"])?;
    let start_combo = builder.labels().intern(["gate_closed", "operator_ready"])?;

    builder.transition_ids(idle, &[start_combo], running);
    builder.transition_ids(running, &[stop], idle);

    builder.build()
}
