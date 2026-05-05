/**
 * Sample TypeScript fixture for compositional extraction.
 *
 * Two `Worker` instances both write to a shared resource (modelled
 * as a separate `composition.resources[]` entry in the extract
 * config). Without coordination this is a classic last-write-wins
 * race: one worker commits, the other commits, the second commit
 * overwrites the first.
 *
 * The resource's state machine is hand-modelled in the extract
 * config (the source itself doesn't make the file explicit). The
 * mu-calculus property `no_clobber` checks that the resource never
 * reaches its `clobbered` state across all reachable interleavings.
 *
 * See `parallel_workers_compositional.extract.json` for the
 * composition + property declaration, and the project wiki page
 * `Compositional-Extraction-Tutorial.md` for the full walkthrough.
 */
class Worker {
    private _committed: boolean = false;

    /** Commit the worker's write to shared storage. Idempotent. */
    commit(): void {
        if (this._committed) {
            return;
        }
        this._committed = true;
    }

    /** Reset the worker so it can commit again. */
    reset(): void {
        this._committed = false;
    }
}
