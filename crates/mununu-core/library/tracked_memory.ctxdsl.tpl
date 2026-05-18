// tracked_memory.ctxdsl.tpl — parameterised single-address memory tracker.
//
// One template instance = one tracked memory region. Two states:
// Initial (untouched) / Written (some write has committed). Used as
// a building block when verifying memory ordering / visibility
// properties; each tracked address gets its own instance.
//
// Labels:
//   wr_mem_{instance_id}    — any write to the region (controllable by
//                            the writer)
//   rd_mem_{instance_id}    — any read from the region (controllable
//                            by the reader)
//   fence_{instance_id}     — region-scoped fence (no-op on state but
//                            propagates through the composition)
//
// To track multiple addresses, instantiate the template N times via
// `count = N`. Each instance is independent — composing them
// asynchronously gives one CLTS state per (tracked_address × write_state)
// cross-product.

context Memory_{instance_id} {
    automata {
        automaton Memory_{instance_id} {
            states {
                state Mem_{instance_id}_Initial initial;
                state Mem_{instance_id}_Written;
            }
            transitions {
                // First write commits the value.
                transition Mem_{instance_id}_Initial -> Mem_{instance_id}_Written on label wr_mem_{instance_id};
                // Subsequent writes — state stays Written.
                transition Mem_{instance_id}_Written -> Mem_{instance_id}_Written on label wr_mem_{instance_id};
                // Reads — observable but state unchanged.
                transition Mem_{instance_id}_Initial -> Mem_{instance_id}_Initial on label rd_mem_{instance_id};
                transition Mem_{instance_id}_Written -> Mem_{instance_id}_Written on label rd_mem_{instance_id};
                // Fence — composition-level barrier; state unchanged.
                transition Mem_{instance_id}_Initial -> Mem_{instance_id}_Initial on label fence_{instance_id};
                transition Mem_{instance_id}_Written -> Mem_{instance_id}_Written on label fence_{instance_id};
            }
        }
    }
}
