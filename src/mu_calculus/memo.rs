use std::collections::HashMap;

use bitvec::prelude::*;

use super::NodeId;

/// Memoisation cache keyed by node identifier.
#[derive(Debug, Default)]
pub(crate) struct MemoizationCache {
    entries: HashMap<NodeId, BitVec<usize, Lsb0>>,
}

impl MemoizationCache {
    /// Returns a cloned memoised result for `node_id`, if available.
    pub(crate) fn get(&self, node_id: &NodeId) -> Option<BitVec<usize, Lsb0>> {
        self.entries.get(node_id).cloned()
    }

    /// Stores a cloned result for `node_id` in the memoisation cache.
    pub(crate) fn insert(&mut self, node_id: NodeId, value: &BitVec<usize, Lsb0>) {
        self.entries.insert(node_id, value.clone());
    }
}
