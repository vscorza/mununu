use crate::clts::{Clts, DefaultLabelIdx, DefaultStateIdx, LabelControllability};
use crate::composition::{self, CompositionOptions, CompositionSemantics};
use crate::context::{Context, ContextError};
use crate::mu_calculus::parser;
use crate::mu_calculus::{Environment, Formula};
use thiserror::Error;

const PIPELINE_NAME: &str = "pipeline";
const SHIP_REQUIRES_RELEASE: &str =
    "nu Safe. (((! < ( labels = {ship} ) > true) || < ( labels = {qp_sign} ) > true) && ([] Safe))";
const COMPLETION_LEADS_TO_DISPOSITION: &str = "nu Progress. (([ ( labels = {qc_start} ) ] (mu Resolve. ((< ( labels = {qp_sign} ) > true || < ( labels = {quarantine} ) > true || < ( labels = {cancel} ) > true) || (<> Resolve)))) && ([] Progress))";

#[derive(Debug, Error)]
pub enum SterileError {
    #[error(transparent)]
    Context(#[from] ContextError),
    #[error(transparent)]
    Compose(#[from] crate::clts::CltsError),
    #[error(transparent)]
    Formula(#[from] crate::mu_calculus::parser::ParseError),
}

pub struct SterileScenario {
    pub context: Context,
    pub pipeline: &'static str,
    pub ship_requires_release: Formula,
    pub completion_leads_to_disposition: Formula,
    pub environment: Environment,
}

pub fn sterile_scenario() -> Result<SterileScenario, SterileError> {
    let production = build_production()?;
    let quality = build_quality_control()?;
    let release = build_release_gateway()?;
    let logistics = build_logistics()?;

    let base_context = Context::builder()
        .register_clts("production", production)
        .register_clts("quality", quality)
        .register_clts("release", release)
        .register_clts("logistics", logistics)
        .finish();

    let options = CompositionOptions::new(CompositionSemantics::Synchronous);
    let prod_quality = base_context.compose_named("production", "quality", &options)?;
    let release_clts = base_context
        .clts("release")
        .ok_or_else(|| ContextError::UnknownClts("release".into()))?;
    let logistics_clts = base_context
        .clts("logistics")
        .ok_or_else(|| ContextError::UnknownClts("logistics".into()))?;

    let prod_quality_release = composition::compose(&prod_quality, release_clts, &options)?;
    let pipeline = composition::compose(&prod_quality_release, logistics_clts, &options)?;

    let pipeline_context = Context::builder()
        .register_clts(PIPELINE_NAME, pipeline)
        .finish_with_checks()?;

    let pipeline_ref = pipeline_context
        .clts(PIPELINE_NAME)
        .ok_or_else(|| ContextError::UnknownClts(PIPELINE_NAME.into()))?;
    let environment = Environment::new(pipeline_ref.state_count());

    let ship_requires_release = parser::parse(SHIP_REQUIRES_RELEASE)?;
    let completion_leads_to_disposition = parser::parse(COMPLETION_LEADS_TO_DISPOSITION)?;

    Ok(SterileScenario {
        context: pipeline_context,
        pipeline: PIPELINE_NAME,
        ship_requires_release,
        completion_leads_to_disposition,
        environment,
    })
}

fn build_production() -> Result<Clts<DefaultStateIdx, DefaultLabelIdx>, crate::clts::CltsError> {
    let mut builder = Clts::builder();
    builder.state("Idle");
    builder.state("Filling");
    builder.state("Complete");
    builder.state("Cancelled");

    let idle = builder.state_id_or_insert("Idle").unwrap();
    builder.initial_state_id(idle);

    let produce = builder.labels().intern(["produce"])?;
    let qc_start = builder.labels().intern(["qc_start"])?;
    let cancel = builder.labels().intern(["cancel"])?;

    let filling = builder.state_id_or_insert("Filling").unwrap();
    let complete = builder.state_id_or_insert("Complete").unwrap();
    let cancelled = builder.state_id_or_insert("Cancelled").unwrap();

    builder.transition_ids(idle, &[produce], filling);
    builder.set_label_controllability(qc_start, LabelControllability::Uncontrollable);
    builder.transition_ids(filling, &[qc_start], complete);
    builder.transition_ids(filling, &[cancel], cancelled);

    builder.build()
}

fn build_quality_control() -> Result<Clts<DefaultStateIdx, DefaultLabelIdx>, crate::clts::CltsError>
{
    let mut builder = Clts::builder();
    builder.state("Waiting");
    builder.state("Testing");
    builder.state("Approved");
    builder.state("Rejected");

    let waiting = builder.state_id_or_insert("Waiting").unwrap();
    builder.initial_state_id(waiting);

    let qc_start = builder.labels().intern(["qc_start"])?;
    let qc_pass = builder.labels().intern(["qc_pass"])?;
    let qc_fail = builder.labels().intern(["qc_fail"])?;
    let qp_sign = builder.labels().intern(["qp_sign"])?;
    let quarantine = builder.labels().intern(["quarantine"])?;

    let testing = builder.state_id_or_insert("Testing").unwrap();
    let approved = builder.state_id_or_insert("Approved").unwrap();
    let rejected = builder.state_id_or_insert("Rejected").unwrap();

    builder.set_label_controllability(qc_start, LabelControllability::Uncontrollable);
    builder.set_label_controllability(qc_pass, LabelControllability::Uncontrollable);
    builder.set_label_controllability(qc_fail, LabelControllability::Uncontrollable);
    builder.transition_ids(waiting, &[qc_start], testing);
    builder.transition_ids(testing, &[qc_pass], approved);
    builder.transition_ids(testing, &[qc_fail], rejected);
    builder.transition_ids(approved, &[qp_sign], waiting);
    builder.transition_ids(rejected, &[quarantine], waiting);

    builder.build()
}

fn build_release_gateway() -> Result<Clts<DefaultStateIdx, DefaultLabelIdx>, crate::clts::CltsError>
{
    let mut builder = Clts::builder();
    builder.state("Hold");
    builder.state("Released");
    builder.state("Quarantined");

    let hold = builder.state_id_or_insert("Hold").unwrap();
    builder.initial_state_id(hold);

    let qp_sign = builder.labels().intern(["qp_sign"])?;
    let quarantine = builder.labels().intern(["quarantine"])?;

    let released = builder.state_id_or_insert("Released").unwrap();
    let quarantined = builder.state_id_or_insert("Quarantined").unwrap();

    builder.transition_ids(hold, &[qp_sign], released);
    builder.transition_ids(hold, &[quarantine], quarantined);

    builder.build()
}

fn build_logistics() -> Result<Clts<DefaultStateIdx, DefaultLabelIdx>, crate::clts::CltsError> {
    let mut builder = Clts::builder();
    builder.state("Staging");
    builder.state("Packed");
    builder.state("Shipped");

    let staging = builder.state_id_or_insert("Staging").unwrap();
    builder.initial_state_id(staging);

    let qp_sign = builder.labels().intern(["qp_sign"])?;
    let ship = builder.labels().intern(["ship"])?;
    let cancel = builder.labels().intern(["cancel"])?;

    let packed = builder.state_id_or_insert("Packed").unwrap();
    let shipped = builder.state_id_or_insert("Shipped").unwrap();

    builder.transition_ids(staging, &[qp_sign], packed);
    builder.transition_ids(packed, &[ship], shipped);
    builder.transition_ids(shipped, &[cancel], staging);

    builder.build()
}
