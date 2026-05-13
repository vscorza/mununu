//! HITL stage-4 review surface — Document A §A7 / Document D §D.8.
//!
//! Stage 4 is the **human-in-the-loop** step between phase-2 discovery
//! and discharge: the user reviews the contract clauses mununu has
//! *proposed* from the available contract sources, then approves /
//! edits / rejects each before it goes into a `ContractSet`.
//!
//! This module ships the minimum viable slice: a `ReviewPackage`
//! that bundles the current discovery output with a flat list of
//! `ProposedClause` objects, one per proposal source. The CLI / HTTP /
//! UI layers consume this package and surface it to the user. The
//! approve/edit/reject *state* lives in the surface, not here — this
//! module is purely the *proposer*.
//!
//! ## Proposal sources
//!
//! Today only two sources are wired up:
//!
//! 1. **Source-comment annotations** — every `@mununu_assume` /
//!    `@mununu_guarantee` carrying a non-empty formula body becomes one
//!    `ProposedClause` with `provenance: source_comment`.
//! 2. **Corpus references** — every `Resolved` `CorpusResolution`
//!    becomes one `ProposedClause` with `provenance: corpus(id)` that
//!    names the entry + alternative + soundness flag. Today corpus
//!    entries are skeletons (parameters + alternatives + provenance,
//!    no concrete formulas), so the proposal carries a reference rather
//!    than a clause body. When entries grow concrete `contract`
//!    payloads, the reference unpacks into per-clause proposals.
//!
//! L\*-learned proposals (Document D §D.6) are reserved for a future
//! slice. The shape is stable so adding a third source is a one-place
//! change.

use crate::clts::LabelControllability;
use crate::contract::discover::{
    BlackBoxInterface, DiscoverOptions, InterfaceLabel, Phase1Output, ResolutionStatus,
    discover_phase1,
};
use crate::mununu_annotations::{MununuAnnotation, MununuTag};
use serde::{Deserialize, Serialize};

/// One proposed clause surfaced for HITL review.
///
/// Mirrors the field names of [`crate::contract::ContractClause`] so a
/// reviewer who accepts a proposal gets a `ContractClause`-shaped
/// object back. The `kind` field is split out from `ContractClause`'s
/// enum to a string so the HTTP wire format stays stable when new
/// clause kinds are added.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProposedClause {
    /// Stable identifier suggested for the clause. Reviewers may rename
    /// at accept-time; mununu pre-computes a sensible default
    /// (`<owner>__<provenance>__<index>`).
    pub id: String,
    /// One of `assumption | guarantee | invariant | reference`.
    /// `reference` is the special-case used for a `Resolved` corpus
    /// entry whose body is not yet unpacked into concrete clauses.
    pub kind: String,
    /// Owning module — the black-box module the clause sits on.
    pub owner: String,
    /// Free-form description (formula body for annotation proposals;
    /// corpus entry description for reference proposals).
    pub description: Option<String>,
    /// Where this proposal came from (`source_comment | corpus | …`).
    pub provenance: ProposalProvenance,
    /// One-line soundness consequence the reviewer should consider
    /// before accepting. For annotation proposals: the user-supplied
    /// formula is taken on trust, so this surfaces a generic
    /// "user-authored clause — verify externally" note. For corpus
    /// references: the corpus entry's `soundness_flag`.
    pub soundness_note: Option<String>,
}

/// Provenance of a `ProposedClause` — driven by the proposal source.
///
/// Wider than `crate::contract::ClauseProvenance` because review-stage
/// proposals carry extra context (e.g. the matched corpus alternative)
/// that the final `ContractClause` does not need.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "source", rename_all = "snake_case")]
pub enum ProposalProvenance {
    /// Proposal extracted from a `@mununu_assume` / `@mununu_guarantee`
    /// source-comment annotation. `tag` is the canonical tag name.
    SourceComment {
        tag: String,
        #[serde(default)]
        source_line: Option<u32>,
    },
    /// Proposal pointing at a corpus entry resolved via
    /// `@mununu_interface contract://`.
    Corpus {
        /// `<domain>/<name>@<version>` identifier.
        entry_id: String,
        /// Alternative selected via `?alt=…`, if any.
        #[serde(default)]
        alternative: Option<String>,
    },
}

/// The full HITL stage-4 package for one black-box interface.
///
/// Wraps the current phase-2 [`Phase1Output`] (so the reviewer sees
/// the alphabet, controllability, and gap markers in the same screen)
/// and adds a flat ordered list of [`ProposedClause`] objects.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewPackage {
    /// Module name — duplicated from `phase1.module` for convenience.
    pub module: String,
    /// Current phase-2 discovery output.
    pub phase1: Phase1Output,
    /// Ordered list of proposed clauses, in discovery order.
    pub proposed_clauses: Vec<ProposedClause>,
}

impl ReviewPackage {
    /// Number of accepted-by-default proposals — every proposal in
    /// today's slice is human-approved at the surface layer, so this
    /// always returns `0`. Surfaces a stable field for downstream UX.
    pub fn pre_accepted_count(&self) -> usize {
        0
    }

    /// Total number of proposals across all sources.
    pub fn total_proposals(&self) -> usize {
        self.proposed_clauses.len()
    }
}

/// Build a `ReviewPackage` for a single black-box interface.
///
/// Runs phase-2 discovery, then walks (a) the interface's annotations
/// and (b) the resulting `corpus_resolutions` to produce one or more
/// `ProposedClause` records per source.
///
/// **Soundness of the proposal layer:** every clause produced here is
/// a *suggestion*. None of them affect the verifier's verdict until
/// the user accepts them and they flow into a `ContractSet`. The
/// approve/edit/reject UX lives in the CLI / HTTP / UI surfaces.
pub fn build_review_package(
    interface: &BlackBoxInterface,
    options: &DiscoverOptions<'_>,
) -> ReviewPackage {
    let phase1 = discover_phase1(interface, options);
    let mut proposals = Vec::new();

    // 1. Source-comment annotations.
    proposals.extend(propose_from_annotations(interface, &phase1.labels));

    // 2. Corpus references.
    proposals.extend(propose_from_corpus(&phase1, &interface.name));

    ReviewPackage {
        module: phase1.module.clone(),
        phase1,
        proposed_clauses: proposals,
    }
}

fn propose_from_annotations(
    interface: &BlackBoxInterface,
    _labels: &[InterfaceLabel],
) -> Vec<ProposedClause> {
    let mut out = Vec::new();
    let mut idx_per_kind = std::collections::HashMap::<&'static str, usize>::new();
    for ann in &interface.annotations {
        let kind = clause_kind_for_tag(ann.tag);
        let Some(kind) = kind else { continue };
        if ann.value.trim().is_empty() {
            continue;
        }
        let counter = idx_per_kind.entry(kind).or_insert(0);
        *counter += 1;
        let proposal_id = format!("{}__sc_{}__{}", interface.name, kind, *counter);
        out.push(ProposedClause {
            id: proposal_id,
            kind: kind.to_string(),
            owner: interface.name.clone(),
            description: Some(ann.value.clone()),
            provenance: ProposalProvenance::SourceComment {
                tag: ann.tag.name().to_string(),
                source_line: ann.source_line,
            },
            soundness_note: Some(soundness_note_for_annotation(ann)),
        });
    }
    out
}

fn propose_from_corpus(phase1: &Phase1Output, owner: &str) -> Vec<ProposedClause> {
    let mut out = Vec::new();
    for (idx, resolution) in phase1.corpus_resolutions.iter().enumerate() {
        if !matches!(resolution.status, ResolutionStatus::Resolved) {
            continue;
        }
        let Some(matched) = resolution.matched_ids.first() else {
            continue;
        };
        let proposal_id = format!("{owner}__corpus_{idx}");
        let description = format!(
            "Corpus reference: {matched}{alt}. Resolver chose this entry from {n} candidate(s).",
            alt = resolution
                .parsed
                .alternative
                .as_ref()
                .map(|a| format!(" (alt: {a})"))
                .unwrap_or_default(),
            n = resolution.matched_ids.len(),
        );
        out.push(ProposedClause {
            id: proposal_id,
            kind: "reference".to_string(),
            owner: owner.to_string(),
            description: Some(description),
            provenance: ProposalProvenance::Corpus {
                entry_id: matched.clone(),
                alternative: resolution.parsed.alternative.clone(),
            },
            soundness_note: Some(format!(
                "Corpus entry's soundness flag applies; verify the alternative ({}) matches your design.",
                resolution
                    .parsed
                    .alternative
                    .as_deref()
                    .unwrap_or("<none requested>")
            )),
        });
    }
    out
}

fn clause_kind_for_tag(tag: MununuTag) -> Option<&'static str> {
    match tag {
        MununuTag::Assume => Some("assumption"),
        MununuTag::Guarantee => Some("guarantee"),
        // Other tags (Blackbox / Interface / Controllable /
        // Uncontrollable) are not clauses — they shape discovery, not
        // the contract body.
        _ => None,
    }
}

fn soundness_note_for_annotation(ann: &MununuAnnotation) -> String {
    match ann.tag {
        MununuTag::Assume => {
            "Environment assumption — vendor-supplied. Reviewer must verify against the deployed environment."
                .to_string()
        }
        MununuTag::Guarantee => {
            "Module guarantee — vendor-supplied. Reviewer must verify against the silicon or the spec."
                .to_string()
        }
        _ => "Annotation surfaced for review; verify externally.".to_string(),
    }
}

/// Convenience: how many proposals come from each source. Used by the
/// CLI / UI for the summary header.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProposalCounts {
    pub source_comment_assumptions: usize,
    pub source_comment_guarantees: usize,
    pub corpus_references: usize,
}

impl ProposalCounts {
    pub fn from_package(pkg: &ReviewPackage) -> Self {
        let mut s = Self::default();
        for p in &pkg.proposed_clauses {
            match (&p.provenance, p.kind.as_str()) {
                (ProposalProvenance::SourceComment { .. }, "assumption") => {
                    s.source_comment_assumptions += 1;
                }
                (ProposalProvenance::SourceComment { .. }, "guarantee") => {
                    s.source_comment_guarantees += 1;
                }
                (ProposalProvenance::Corpus { .. }, _) => {
                    s.corpus_references += 1;
                }
                _ => {}
            }
        }
        s
    }

    pub fn total(&self) -> usize {
        self.source_comment_assumptions + self.source_comment_guarantees + self.corpus_references
    }
}

/// Helper: derive `LabelControllability` consequence text for the UI.
/// Surfaces a one-liner like "AES_CTR_v1.start is Controllable from
/// the host's perspective" for each label, so the reviewer can sanity-
/// check the proposed contract against the alphabet without flipping
/// tabs.
pub fn controllability_summary(label: &InterfaceLabel) -> String {
    let class = match label.controllability {
        LabelControllability::Controllable => "Controllable",
        LabelControllability::Uncontrollable => "Uncontrollable",
        LabelControllability::Internal => "Internal",
    };
    format!("{}: {class}", label.name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::discover::{BlackBoxInterface, PortDescriptor};
    use crate::controllability::BoundaryDirection;

    fn iface_with_annotations() -> BlackBoxInterface {
        BlackBoxInterface {
            name: "AES_CTR_v1".to_string(),
            ports: vec![
                PortDescriptor {
                    name: "start".to_string(),
                    direction: BoundaryDirection::Input,
                    description: None,
                },
                PortDescriptor {
                    name: "done".to_string(),
                    direction: BoundaryDirection::Output,
                    description: None,
                },
            ],
            source_file: None,
            source_line: None,
            annotations: vec![
                MununuAnnotation::new(MununuTag::Blackbox, ""),
                MununuAnnotation::new(MununuTag::Guarantee, "G(start -> eventually done)"),
                MununuAnnotation::new(MununuTag::Assume, "G(start -> !reset)"),
            ],
        }
    }

    #[test]
    fn extracts_one_proposal_per_assume_and_guarantee() {
        let iface = iface_with_annotations();
        let pkg = build_review_package(&iface, &DiscoverOptions::default());
        assert_eq!(pkg.proposed_clauses.len(), 2);
        let kinds: Vec<&str> = pkg
            .proposed_clauses
            .iter()
            .map(|c| c.kind.as_str())
            .collect();
        assert!(kinds.contains(&"assumption"));
        assert!(kinds.contains(&"guarantee"));
    }

    #[test]
    fn skips_annotations_without_a_clause_body() {
        let iface = iface_with_annotations();
        let pkg = build_review_package(&iface, &DiscoverOptions::default());
        // The Blackbox annotation has an empty value → not a clause.
        for c in &pkg.proposed_clauses {
            assert!(c.description.as_deref().is_some_and(|s| !s.is_empty()));
        }
    }

    #[test]
    fn source_comment_proposals_carry_tag_provenance() {
        let iface = iface_with_annotations();
        let pkg = build_review_package(&iface, &DiscoverOptions::default());
        let guarantee = pkg
            .proposed_clauses
            .iter()
            .find(|c| c.kind == "guarantee")
            .expect("guarantee proposal");
        match &guarantee.provenance {
            ProposalProvenance::SourceComment { tag, .. } => {
                assert_eq!(tag, "guarantee");
            }
            other => panic!("expected SourceComment provenance, got {other:?}"),
        }
        assert_eq!(
            guarantee.description.as_deref(),
            Some("G(start -> eventually done)")
        );
    }

    #[test]
    fn proposal_counts_track_each_source() {
        let iface = iface_with_annotations();
        let pkg = build_review_package(&iface, &DiscoverOptions::default());
        let counts = ProposalCounts::from_package(&pkg);
        assert_eq!(counts.source_comment_assumptions, 1);
        assert_eq!(counts.source_comment_guarantees, 1);
        assert_eq!(counts.corpus_references, 0);
        assert_eq!(counts.total(), 2);
    }

    #[test]
    fn corpus_resolution_becomes_a_reference_proposal() {
        use crate::corpus::{ContractEntry, Corpus, Provenance};
        let corpus = Corpus::from_entries(vec![ContractEntry {
            id: "rtl_crypto/aes_ctr".to_string(),
            version: "1.0.0".to_string(),
            domain: "rtl_crypto".to_string(),
            name: "aes_ctr".to_string(),
            description: None,
            parameters: Default::default(),
            contract: None,
            alternatives: Vec::new(),
            provenance: Provenance::MununuVerified {
                verified_against: None,
            },
            soundness_flag: None,
        }]);
        let iface = BlackBoxInterface {
            name: "AES_CTR_v1".to_string(),
            ports: vec![PortDescriptor {
                name: "done".to_string(),
                direction: BoundaryDirection::Output,
                description: None,
            }],
            source_file: None,
            source_line: None,
            annotations: vec![MununuAnnotation::new(
                MununuTag::Interface,
                "contract://rtl_crypto/aes_ctr@1.0.0?alt=strict_iv",
            )],
        };
        let opts = DiscoverOptions {
            corpus: Some(&corpus),
            ..DiscoverOptions::default()
        };
        let pkg = build_review_package(&iface, &opts);
        let counts = ProposalCounts::from_package(&pkg);
        assert_eq!(counts.corpus_references, 1);
        let proposal = pkg
            .proposed_clauses
            .iter()
            .find(|c| c.kind == "reference")
            .expect("reference proposal");
        match &proposal.provenance {
            ProposalProvenance::Corpus {
                entry_id,
                alternative,
            } => {
                assert_eq!(entry_id, "rtl_crypto/aes_ctr@1.0.0");
                assert_eq!(alternative.as_deref(), Some("strict_iv"));
            }
            other => panic!("expected Corpus provenance, got {other:?}"),
        }
    }

    #[test]
    fn proposal_ids_are_stable_and_unique_per_module() {
        let iface = iface_with_annotations();
        let pkg = build_review_package(&iface, &DiscoverOptions::default());
        let ids: std::collections::HashSet<&str> =
            pkg.proposed_clauses.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids.len(), pkg.proposed_clauses.len(), "ids must be unique");
        for c in &pkg.proposed_clauses {
            assert!(
                c.id.starts_with("AES_CTR_v1__"),
                "id `{}` should be prefixed with owner",
                c.id
            );
        }
    }
}
