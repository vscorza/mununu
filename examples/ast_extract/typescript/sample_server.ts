/**
 * Sample TypeScript server for demonstrating AST-based extraction.
 *
 * This class has a lifecycle state machine (started, closed, initialized)
 * with a known vulnerability: request handlers do not check the _closed flag.
 */
class Server {
    private _started: boolean = false;
    private _closed: boolean = false;
    private _initialized: boolean = false;

    /** Start the server. Throws if already started. */
    start(): void {
        if (this._started) {
            throw new Error('already started');
        }
        this._started = true;
    }

    /** Initialize the server. Requires start() first. */
    initialize(): void {
        if (this._initialized) {
            throw new Error('already initialized');
        }
        this._initialized = true;
    }

    /** Handle a client request. Does NOT check _closed. */
    handleRequest(): void {
        // Vulnerability: no check for this._closed
        // Requests are processed even after close()
    }

    /** Close the server. Idempotent (returns if already closed). */
    close(): void {
        if (this._closed) {
            return;
        }
        this._closed = true;
    }
}
