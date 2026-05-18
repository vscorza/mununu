// plic.ctxdsl.tpl — parameterised RISC-V PLIC (Platform Interrupt Controller)
// for a single interrupt source.
//
// One template instance = one tracked source × one observing hart.
// State: IRQ_Cleared (default) / IRQ_Pending (raised, not yet served).
//
// Labels:
//   irq_assert_{instance_id}  — hardware raises the interrupt (env-driven)
//   irq_ack_{instance_id}     — hart acknowledges + clears (firmware-driven)
//
// For an N-source PLIC, instantiate this template N times via the
// verify framework's `count = N` parameterisation (plan Part 6 item 6).
// Each instance gets a unique `{instance_id}` substitution and produces
// independent IRQ_Cleared / IRQ_Pending state per source.

context PLIC_{instance_id} {
    automata {
        automaton PLIC_{instance_id} {
            states {
                state PLIC_{instance_id}_Cleared initial;
                state PLIC_{instance_id}_Pending;
            }
            transitions {
                // Environment-driven: the interrupt is asserted from
                // outside (peripheral, timer, external pin). Hart has
                // not acknowledged yet.
                transition PLIC_{instance_id}_Cleared -> PLIC_{instance_id}_Pending on label irq_assert_{instance_id};
                // Hart acknowledges + clears. Controllable from the
                // firmware's perspective.
                transition PLIC_{instance_id}_Pending -> PLIC_{instance_id}_Cleared on label irq_ack_{instance_id};
                // Spurious ack while not pending (firmware writes the
                // claim register without an actual pending IRQ).
                // Modelled as a no-op self-loop so the label remains
                // composable with other automata that fire it.
                transition PLIC_{instance_id}_Cleared -> PLIC_{instance_id}_Cleared on label irq_ack_{instance_id};
                // Repeated asserts while already pending — also a no-op.
                transition PLIC_{instance_id}_Pending -> PLIC_{instance_id}_Pending on label irq_assert_{instance_id};
            }
        }
    }
}
