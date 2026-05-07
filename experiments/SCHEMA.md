# Experiment manifest schema

`experiments/EXP-NNNN-<slug>/manifest.json` is the machine-readable provenance record for an experiment archive. The schema is versioned so that the scaffold can evolve without invalidating older archives.

## Versioning policy

- **Required fields** define the contract. Adding or renaming a required field bumps `schema_version`.
- **Optional fields** can be added freely without a bump; consumers must tolerate their absence.
- `scripts/check_repro.sh` validates **per-version requirements**, not the union — an EXP-0001 archive at `schema_version: 1` does not need fields introduced in `schema_version: 2`.
- Old archives are never rewritten in-place. If a version bump fixes a bug, archives are re-recorded under a new EXP-NNNN with a `supersedes: EXP-NNNN` field; the old archive stays as historical evidence.

## v1 (2026-05-06, current)

```json
{
  "schema_version": 1,
  "exp_id": "EXP-NNNN-slug",                    // required
  "commit": "<git sha>",                         // required
  "branch": "<git branch>",                      // required
  "git_dirty": "yes|no",                         // required
  "host": "<hostname>",                          // required
  "container": "yes|no",                         // required
  "command": "cargo bench --bench ...",          // required: exact replay command
  "started_at": "<ISO 8601 UTC>",                // required
  "ended_at": "<ISO 8601 UTC>",                  // required
  "exit_code": <int>,                            // required
  "hw_fingerprint_sha256": "<sha256>",           // required
  "criterion_archive_sha256": "<sha256> | \"\"", // required (may be empty if no archive)
  "rustc": "<rustc --version output>",           // required
  "rust_toolchain_toml_sha256": "<sha256>",      // required
  "dev_container_dockerfile_sha256": "<sha256>"  // required
}
```

Optional fields a future bench harness MAY add without a bump:

- `dhat_archive_sha256` — set when memory profiling is enabled.
- `seed` — explicit deterministic seed (required to bump v2 once we wire `RandomClts` into the bench harness).
- `bench_targets` — array of bench names exercised, for partial-archive comparisons.
- `notes` — short free-text caveat.

## Draft EXPs

An EXP scaffolded by `scripts/new_experiment.sh` carries a `.draft` marker file. While that marker is present:

- `scripts/check_repro.sh` requires only `README.md` + `log.md` (the lab notebook content).
- `manifest.json`, `command.txt`, `hw-fingerprint.txt`, `criterion-archive.tar.zst` are **not** required.
- The CI replay gate skips the EXP entirely.

The marker is removed automatically by `scripts/bench_record.sh` on a successful run, promoting the EXP to a fully-validated archive. Manually delete the marker (`rm experiments/EXP-NNNN-<slug>/.draft`) only if you've populated the required files by hand and you understand the consequences for reproducibility.

This convention lets us **plan an EXP, write its hypothesis pre-registration in `README.md` and `log.md`, and commit the scaffolding** before running the bench — which matters for pre-registered experiments that should not be tuned to the data.

## Iteration & refinement

The scaffold is expected to evolve. To change the manifest schema:

1. Append a new section to this file describing the new version.
2. Bump `schema_version` in `scripts/bench_record.sh`.
3. Update `scripts/check_repro.sh` to validate the new required fields **only for archives at the new version**.
4. Add an ADR in `notebook/decisions.md` explaining the rationale.
5. Existing archives stay intact at their original version. They continue to replay as long as the code paths they exercise still exist.

If a refinement is so deep that older archives can no longer replay (e.g., the bench harness signature changed), document the cliff in the ADR and tag the affected archives with `superseded_by: EXP-NNNN` in their `notes.md` or in `manifest.json` (optional field; no bump needed).
