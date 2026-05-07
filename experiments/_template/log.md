# EXP-NNNN: <short title>

**Author:** Mariano Cerrutti (vscorza@gmail.com)
**Date opened:** YYYY-MM-DD
**Date closed:** YYYY-MM-DD
**Commit baseline:** <sha>
**Commit candidate:** <sha>
**Container digest:** <sha256>
**Hardware:** <one-line summary; full manifest in hw-fingerprint.txt>

## Motivation

<Why this matters. Cite the inventory file:line that shows the cost. Cite prior work where relevant: Paige-Tarjan 1987, Tarjan 1972, Knaster-Tarski, Bruns-Godefroid CONCUR 2000 for the OOB sink, etc.>

## Hypothesis

<Testable, quantified. "We expect ≥3× speedup on chain CLTSs of 10k states with no regression on grid CLTSs." Pre-registered before the run.>

## Method

- Inputs: <fixture name + seed>
- Bench: <bench name + criterion config>
- Test: <property test or differential test that must remain green>
- Statistical test: paired t-test on Criterion samples; significance at p<0.01.

## Results

- Median: <baseline> → <candidate>
- 95% CI: <[lo, hi]> → <[lo, hi]>
- Speedup (Kalibera-Jones bootstrap): X.YYx [CI lo, CI hi]
- Memory (dhat): peak <N> MB → <M> MB; allocations <K> → <K'>
- Tests: <green / red, with link>

## Interpretation

<Did the hypothesis hold? Anything surprising?>

## Dead-ends

<Approaches tried that didn't work. Each with one paragraph and a date.>

## Followups

<Concrete EXP-IDs to open next.>

## Artifacts

- criterion-archive.tar.zst (sha256: ...)
- dhat-archive.tar.zst (sha256: ...) (if applicable)
- raw stdout/stderr (manifest.json links)
