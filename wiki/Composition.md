# Composition

> **Alpha Software** — Mununu is under active development. APIs, syntax, and behavior may change. We welcome feedback and bug reports via [GitHub Issues](https://github.com/vscorza/mununu/issues).

Composition is how you build larger systems from smaller automata. Mununu supports two modes -- synchronous and asynchronous -- and lets you nest them for hierarchical, cross-domain designs.

## Synchronous Composition

In synchronous composition, **all members step simultaneously on every cycle**. Shared labels (labels that appear in more than one member) must fire together. Non-shared labels combine freely when both sides move.

```
composition {
    synchronous pipeline {
        members [Producer, Consumer];
    }
}
```

### Skip Labels

Because every member must take a step on every cycle, each automaton needs its own **skip label** -- a self-loop that lets it do nothing while the other side acts. If two automata share the same idle/skip label, they are forced to idle together, which is usually not what you want.

```
automaton Producer {
    // ...
    transitions {
        transition Empty -> Empty on label p_skip;
        transition DataReady -> DataReady on label p_skip;
        // ...
    }
}

automaton Consumer {
    // ...
    transitions {
        transition Waiting -> Waiting on label c_skip;
        transition Processing -> Processing on label c_skip;
        // ...
    }
}
```

### Example: Producer-Consumer Pipeline

A producer generates data and hands it to a consumer via a shared `transfer` label. Both run on the same clock.

```
context sync_producer_consumer {
    automata {
        automaton Producer {
            controllable { label produce; }
            states {
                state Empty initial;
                state DataReady;
            }
            transitions {
                transition Empty -> DataReady on label produce;
                transition DataReady -> Empty on label transfer;
                transition Empty -> Empty on label p_skip;
                transition DataReady -> DataReady on label p_skip;
            }
        }

        automaton Consumer {
            controllable { label consume; }
            states {
                state Waiting initial;
                state Processing;
            }
            transitions {
                transition Waiting -> Processing on label transfer;
                transition Processing -> Waiting on label consume;
                transition Waiting -> Waiting on label c_skip;
                transition Processing -> Processing on label c_skip;
            }
        }
    }

    composition {
        synchronous pipeline {
            members [Producer, Consumer];
        }
    }
}
```

## Asynchronous Composition

In asynchronous composition, **any single member can step independently** while the others stay still. This models interleaved execution -- no shared clock, no lock-step requirement. Shared labels still synchronize when they fire.

```
composition {
    asynchronous sensor_network {
        members [SensorA, SensorB, Monitor];
    }
}
```

No skip labels are needed because members are not forced to move on every step.

### Example: Sensor Network

Two vibration sensors operate independently. A central monitor receives their reports.

```
context async_sensors {
    automata {
        automaton SensorA {
            controllable { label sensor_a_sample; }
            states {
                state IdleA initial;
                state SampledA;
            }
            transitions {
                transition IdleA -> SampledA on label sensor_a_sample;
                transition SampledA -> IdleA on label sensor_a_report;
            }
        }

        automaton SensorB {
            controllable { label sensor_b_sample; }
            states {
                state IdleB initial;
                state SampledB;
            }
            transitions {
                transition IdleB -> SampledB on label sensor_b_sample;
                transition SampledB -> IdleB on label sensor_b_report;
            }
        }

        automaton Monitor {
            controllable { label monitor_process; }
            states {
                state Ready initial;
                state ReceivedA;
                state ReceivedB;
            }
            transitions {
                transition Ready -> ReceivedA on label sensor_a_report;
                transition Ready -> ReceivedB on label sensor_b_report;
                transition ReceivedA -> Ready on label monitor_process;
                transition ReceivedB -> Ready on label monitor_process;
            }
        }
    }

    composition {
        asynchronous sensor_network {
            members [SensorA, SensorB, Monitor];
        }
    }
}
```

## Hierarchical Composition

You can nest compositions: compose groups synchronously first, then combine those groups asynchronously (or vice versa). This is the natural way to model multi-clock-domain hardware.

### Example: Cross-Domain ADC Pipeline

A fast sampling domain (ADC + buffer) runs lock-step on a fast clock. A slow processing domain (processor + result store) runs lock-step on a slow clock. The two domains interleave independently, communicating through a shared FIFO label.

```
context cross_domain_adc {
    automata {
        // Fast domain: ADC and SampleBuffer (each with skip labels)
        automaton ADC { /* ... */ }
        automaton SampleBuffer { /* ... */ }

        // Slow domain: Processor and ResultStore (each with skip labels)
        automaton Processor { /* ... */ }
        automaton ResultStore { /* ... */ }
    }

    composition {
        // Step 1: compose each clock domain synchronously
        synchronous fast_domain {
            members [ADC, SampleBuffer];
        }
        synchronous slow_domain {
            members [Processor, ResultStore];
        }

        // Step 2: compose the two domains asynchronously
        asynchronous system {
            members [fast_domain, slow_domain];
        }
    }
}
```

The full example is in `tutorial/examples/04_cross_domain.ctxdsl`.

## Synchronous vs. Asynchronous: When to Use Each

| | Synchronous | Asynchronous |
|---|---|---|
| **Execution model** | Lock-step; all members move every cycle | Interleaved; one member moves per step |
| **Shared labels** | Must fire simultaneously | Must fire simultaneously (when chosen) |
| **Skip labels** | Required (per-automaton) | Not needed |
| **Typical use** | Components on the same clock domain | Independent subsystems, different clocks |
| **State space** | Product of member states (dense) | Product of member states (sparser in practice) |
| **Example** | ADC + sample buffer on a fast clock | Two sensors reporting to a monitor |

## Key Gotchas

- **Shared idle labels force mutual idling.** In synchronous mode, if two automata share the same skip/idle label, they can only idle together. Always use per-automaton skip labels (e.g., `p_skip`, `c_skip`).
- **Skip labels are only needed for synchronous composition.** In asynchronous mode, members are not required to step, so self-loop skip labels are unnecessary.
- **Hierarchical composition is order-sensitive.** Inner compositions must be declared before the outer composition that references them.
- **Formulas can target any level.** You can write mu-calculus properties over individual automata, inner compositions, or the outermost system -- just set the `over` field accordingly.
