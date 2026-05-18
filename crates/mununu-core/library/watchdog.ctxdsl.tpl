// watchdog.ctxdsl.tpl — parameterised watchdog timer.
//
// Three states: Disabled (default), Armed (counting down towards
// timeout), Expired (timeout fired — typically triggers reset).
// Firmware kicks the watchdog by re-entering Armed from Armed; failure
// to kick before the timeout transitions to Expired.
//
// Labels:
//   watchdog_arm_{instance_id}     — firmware enables / re-arms (controllable)
//   watchdog_kick_{instance_id}    — firmware refreshes the timer (controllable)
//   watchdog_tick_{instance_id}    — environment-driven timer tick (uncontrollable)
//   watchdog_expire_{instance_id}  — timer reached zero (uncontrollable)
//   watchdog_clear_{instance_id}   — firmware acknowledges + disables (controllable)
//
// Property surface:
//   - `reachable(Watchdog_{instance_id}_Expired)` — sanity check
//   - `never(Watchdog_{instance_id}_Expired)` paired with a fairness
//     assumption that watchdog_kick fires sufficiently often — the
//     liveness-equivalent of "the firmware kicks the dog in time"
//   - `bounded_handoff(Watchdog_{instance_id}_Armed,
//                      Watchdog_{instance_id}_Disabled)` — when armed,
//     can always be cleared

context Watchdog_{instance_id} {
    automata {
        automaton Watchdog_{instance_id} {
            states {
                state Watchdog_{instance_id}_Disabled initial;
                state Watchdog_{instance_id}_Armed;
                state Watchdog_{instance_id}_Expired;
            }
            transitions {
                // Firmware arms the watchdog.
                transition Watchdog_{instance_id}_Disabled -> Watchdog_{instance_id}_Armed on label watchdog_arm_{instance_id};
                // Firmware kicks (re-arms) — stays Armed.
                transition Watchdog_{instance_id}_Armed -> Watchdog_{instance_id}_Armed on label watchdog_kick_{instance_id};
                // Tick — modelled as a no-op self-loop; the real time
                // count is abstracted away per docs/abstraction.md.
                transition Watchdog_{instance_id}_Armed -> Watchdog_{instance_id}_Armed on label watchdog_tick_{instance_id};
                // Timeout fires — environment-driven transition to Expired.
                transition Watchdog_{instance_id}_Armed -> Watchdog_{instance_id}_Expired on label watchdog_expire_{instance_id};
                // Firmware clears after expiration (often via reset
                // sequence) — returns to Disabled.
                transition Watchdog_{instance_id}_Expired -> Watchdog_{instance_id}_Disabled on label watchdog_clear_{instance_id};
                // Firmware disables while armed (proactive shutdown).
                transition Watchdog_{instance_id}_Armed -> Watchdog_{instance_id}_Disabled on label watchdog_clear_{instance_id};
            }
        }
    }
}
