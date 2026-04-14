# Adapter Implementation Plans

Tracking documents for new format adapters. Each plan includes viability phases, benchmarks, and a progress log that agents must update before ending sessions.

## Plans

| Plan | Status | Domain | Key Gate |
|------|--------|--------|----------|
| [systemverilog-adapter.md](systemverilog-adapter.md) | NOT STARTED | RTL behavioral FSM extraction | Phase 0: back-translated hw examples match existing CTXDSL |
| [solidity-adapter.md](solidity-adapter.md) | NOT STARTED | Smart contract security via game-theoretic synthesis | Phase 0: reentrancy detection works on hand-translated CTXDSL |

## Agent Protocol

Before ending any session working on these plans, the agent MUST:

1. Read the relevant plan file
2. Update the Progress Log with a dated entry
3. Update phase status markers (NOT STARTED / IN PROGRESS / BLOCKED / DONE)
4. Add any new risks to the Risk Register
5. Save the file

The plan file is the source of truth for project state across sessions.
