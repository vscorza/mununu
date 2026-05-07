# EXP-NNNN: <short title>

**One-line summary.** What this experiment measured and the headline number.

## Motivation

Why does this matter? Cite the inventory `file:line` showing the cost. Cite prior work (Paige-Tarjan 1987, Tarjan 1972, Knaster-Tarski, Bruns-Godefroid CONCUR 2000, etc.).

## Hypothesis

Quantified, pre-registered, testable. Example: "≥3× speedup on chain CLTSs of 10k states with no regression on grid CLTSs of equivalent edge count."

## Headline result

`<baseline median> → <candidate median>`, speedup `<X.YY×>` [95% CI: lo, hi].

Memory (if applicable): peak `<N>` MB → `<M>` MB; allocations `<K>` → `<K'>`.

Tests: ✓ green / ✗ red (link).

## How to replay

```bash
make replay EXP=NNNN
```

Full provenance: `manifest.json`, hardware: `hw-fingerprint.txt`, raw output: `criterion-archive.tar.zst`, lab notebook: `log.md`, observations: `notes.md`.

## Files in this archive

- `README.md` — this file.
- `manifest.json` — provenance: commit SHA, container digest, build flags, env, command, timestamps.
- `command.txt` — exact replay command.
- `hw-fingerprint.txt` — output of `scripts/capture_hw.sh` at run time.
- `criterion-archive.tar.zst` — `target/criterion/` archive (raw Criterion JSON).
- `dhat-archive.tar.zst` — memory profile archive (if applicable).
- `log.md` — dated lab-notebook entry (motivation, hypothesis, method, results, interpretation, dead-ends, followups).
- `notes.md` — free-form observations.

## Status

`open` | `closed` | `superseded by EXP-NNNN`
