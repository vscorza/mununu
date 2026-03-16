# Case Studies

This directory contains documented case studies of property violations discovered during formal verification of public specifications.

## Structure

Each case study is a markdown document following the template in `docs/case_study_template.md`.

## Categories

Case studies are organized by:
- **Domain**: BPMN (Business Process)
- **Category**: Real Defect, Ambiguity, or Semantic Mismatch
- **Status**: Open, Resolved, or Under Review

## Adding a Case Study

1. Use the template from `docs/case_study_template.md`
2. Fill in all relevant sections
3. Include reproducible artifacts (commit hashes, file SHAs, counterexample traces)
4. Document assumptions and semantic choices clearly
5. Generate issue templates using the disclosure workflow

## Disclosure Process

1. **Triage**: Categorize the finding (Real Defect / Ambiguity / Semantic Mismatch)
2. **Document**: Create case study document
3. **Artifact**: Generate reproducible artifact with all necessary information
4. **Template**: Generate issue/PR template using disclosure workflow
5. **Review**: Review case study for accuracy and completeness
6. **Disclose**: File issue/PR with upstream project (if appropriate)

## Current Case Studies

### BPMN

- [test_bpmn_order_approval.md](test_bpmn_order_approval.md) - Test case validating mining workflow infrastructure

