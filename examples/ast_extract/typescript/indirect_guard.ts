/**
 * Test case for indirect guard detection (L2).
 *
 * Method `doAction` assigns `this._locked` to a local variable `isLocked`,
 * then checks `isLocked` in an if-statement. The extractor should detect
 * that `isLocked` refers to `this._locked` and extract the guard.
 */
class Resource {
    private _locked: boolean = false;
    private _ready: boolean = false;

    lock(): void {
        this._locked = true;
    }

    prepare(): void {
        this._ready = true;
    }

    doAction(): void {
        const isLocked = this._locked;
        if (isLocked) {
            return;
        }
        // only proceeds if not locked
    }
}
