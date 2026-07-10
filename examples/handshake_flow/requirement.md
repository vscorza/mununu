# Control requirement — producer / FIFO / consumer flow control

A streaming datapath moves items from a **producer** through a small **bounded
FIFO** to a **consumer**. Write the **flow-control kernel** that drives the
FIFO's `enq`/`deq`, given the producer's `p_valid`, the consumer's `c_ready`,
and the FIFO's `full`/`empty` status.

It must never lose or invent data, and it must not stall:

- **Never overflow** — never enqueue into a full FIFO.
- **Never underflow** — never dequeue an empty FIFO.
- **Never accept phantom data** — only enqueue when the producer is offering.
- **Never drop** — only dequeue when the consumer is ready to take the item.
- **Eager accept** — whenever the producer is offering and the FIFO has room,
  accept the item.
- **Eager deliver** — whenever the FIFO holds data and the consumer is ready,
  deliver it.

Assume the environment is fair: the consumer is ready infinitely often, the FIFO
drains (has room infinitely often), and the producer keeps offering.
