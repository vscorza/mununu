# Control requirement (natural language)

> The input to the loop — what a designer or an upstream agent says. Fed through
> `prompt.md` to produce `arbiter.tlsf`.

**Two clients share a bus.** The arbiter must **never grant both at the same time**.
Assuming each client keeps asserting its request until served, **every request must
eventually be granted** (no client is starved).

Signals: inputs `req_0`, `req_1` (client requests); outputs `grant_0`, `grant_1`
(bus grants).
