/// Sample Rust protocol for demonstrating AST-based extraction.
///
/// Models a connection with started/closed lifecycle.

pub struct Connection {
    started: bool,
    closed: bool,
}

impl Connection {
    pub fn start(&mut self) {
        if self.started {
            panic!("already started");
        }
        self.started = true;
    }

    pub fn close(&mut self) {
        if self.closed {
            return;
        }
        self.closed = true;
    }

    pub fn send(&mut self) {
        // Does not check self.closed — vulnerability
    }
}
