# Control requirement — clock-gating interlock

A datapath domain can have its clock gated to save power. Write the
**clock-gating kernel** that drives `gate`, given the domain's `activity` (it has
work in flight) and a `sleep_req` from the power policy.

- **Interlock (never gate while active)** — never gate the clock while the domain
  is doing work; that would corrupt a live computation. This is the safety
  property a single missed corner turns into silent data corruption.
- **Honor sleep requests** — once the domain is idle, a `sleep_req` is eventually
  granted (the clock is gated to save power).

Assume the environment is fair: the domain eventually goes idle (`G F !activity`),
so gating is always eventually possible.
