//! Bounds-checked big-endian reader for sfnt tables.

use crate::error::{Error, Result};

#[derive(Clone, Copy)]
pub struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    pub fn at(data: &'a [u8], pos: usize) -> Result<Self> {
        if pos > data.len() {
            return Err(Error::OutOfBounds {
                needed: pos,
                len: data.len(),
            });
        }
        Ok(Self { data, pos })
    }

    pub fn data(&self) -> &'a [u8] {
        self.data
    }

    pub fn pos(&self) -> usize {
        self.pos
    }

    pub fn seek(&mut self, pos: usize) -> Result<()> {
        if pos > self.data.len() {
            return Err(Error::OutOfBounds {
                needed: pos,
                len: self.data.len(),
            });
        }
        self.pos = pos;
        Ok(())
    }

    pub fn skip(&mut self, n: usize) -> Result<()> {
        self.seek(self.pos + n)
    }

    pub fn remaining(&self) -> usize {
        self.data.len().saturating_sub(self.pos)
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self.pos.checked_add(n).ok_or(Error::OutOfBounds {
            needed: usize::MAX,
            len: self.data.len(),
        })?;
        if end > self.data.len() {
            return Err(Error::OutOfBounds {
                needed: end,
                len: self.data.len(),
            });
        }
        let out = &self.data[self.pos..end];
        self.pos = end;
        Ok(out)
    }

    pub fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }

    pub fn u16(&mut self) -> Result<u16> {
        let b = self.take(2)?;
        Ok(u16::from_be_bytes([b[0], b[1]]))
    }

    pub fn i16(&mut self) -> Result<i16> {
        Ok(self.u16()? as i16)
    }

    pub fn u32(&mut self) -> Result<u32> {
        let b = self.take(4)?;
        Ok(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }

    pub fn i32(&mut self) -> Result<i32> {
        Ok(self.u32()? as i32)
    }

    pub fn i8(&mut self) -> Result<i8> {
        Ok(self.u8()? as i8)
    }

    pub fn tag(&mut self) -> Result<[u8; 4]> {
        let b = self.take(4)?;
        Ok([b[0], b[1], b[2], b[3]])
    }

    pub fn f2dot14(&mut self) -> Result<f32> {
        Ok(self.i16()? as f32 / 16384.0)
    }

    pub fn bytes(&mut self, n: usize) -> Result<&'a [u8]> {
        self.take(n)
    }
}
