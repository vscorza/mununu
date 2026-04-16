/**
 * Test case for compound guard extraction.
 *
 * Method `doWork` has a compound guard: `if (!this._ready && this._active)`.
 * Both guards should be extracted (ready=MustBeFalse after early-return inversion,
 * active=MustBeTrue after early-return inversion).
 *
 * Without compound guard extraction, only `_ready` is captured.
 */
class Worker {
    private _ready: boolean = false;
    private _active: boolean = false;

    activate(): void {
        this._active = true;
    }

    makeReady(): void {
        this._ready = true;
    }

    doWork(): void {
        if (!this._ready || !this._active) {
            return;
        }
        // only proceeds if both ready AND active
    }

    reset(): void {
        this._ready = false;
        this._active = false;
    }
}
