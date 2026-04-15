//! Binary I/O helpers for persistence.
//!
//! These extension traits provide a small, type-safe wrapper around common
//! read/write primitives used by the persistence layer (u32, u8, bool).

use std::io::{Read, Write};

use super::PersistenceError;

/// Extension trait for writing primitive values in the persistence format.
pub(crate) trait BinaryWrite {
    fn write_u32(&mut self, value: u32) -> Result<(), PersistenceError>;
    fn write_u8(&mut self, value: u8) -> Result<(), PersistenceError>;
    fn write_bool(&mut self, value: bool) -> Result<(), PersistenceError>;
}

/// Extension trait for reading primitive values in the persistence format.
pub(crate) trait BinaryRead {
    fn read_u32(&mut self) -> Result<u32, PersistenceError>;
    fn read_u8(&mut self) -> Result<u8, PersistenceError>;
    fn read_bool(&mut self) -> Result<bool, PersistenceError>;
}

impl<W: Write> BinaryWrite for W {
    fn write_u32(&mut self, value: u32) -> Result<(), PersistenceError> {
        self.write_all(&value.to_le_bytes())?;
        Ok(())
    }

    fn write_u8(&mut self, value: u8) -> Result<(), PersistenceError> {
        self.write_all(&[value])?;
        Ok(())
    }

    fn write_bool(&mut self, value: bool) -> Result<(), PersistenceError> {
        self.write_u8(if value { 1 } else { 0 })
    }
}

impl<R: Read> BinaryRead for R {
    fn read_u32(&mut self) -> Result<u32, PersistenceError> {
        let mut buf = [0u8; 4];
        self.read_exact(&mut buf)?;
        Ok(u32::from_le_bytes(buf))
    }

    fn read_u8(&mut self) -> Result<u8, PersistenceError> {
        let mut buf = [0u8; 1];
        self.read_exact(&mut buf)?;
        Ok(buf[0])
    }

    fn read_bool(&mut self) -> Result<bool, PersistenceError> {
        Ok(match self.read_u8()? {
            0 => false,
            1 => true,
            other => {
                return Err(PersistenceError::InvalidSnapshot(format!(
                    "invalid boolean value {other}"
                )));
            }
        })
    }
}
