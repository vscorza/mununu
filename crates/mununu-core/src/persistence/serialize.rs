//! Serialization traits for persistence.
//!
//! These small traits provide a uniform interface for writing and reading
//! persistence structures to and from binary streams. Implementations live in
//! `persistence::mod` so they can reuse the existing low-level I/O helpers.

use std::io::{Read, Write};

use super::PersistenceError;

/// Binary serialization contract for persistence data structures.
pub(crate) trait Serializable {
    fn serialize<W: Write>(&self, writer: &mut W) -> Result<(), PersistenceError>;
}

/// Binary deserialization contract for persistence data structures.
pub(crate) trait Deserializable: Sized {
    fn deserialize<R: Read>(reader: &mut R) -> Result<Self, PersistenceError>;
}
