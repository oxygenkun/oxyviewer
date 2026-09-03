//! Endian-aware byte reader with bounds checking (F1, F2).

use crate::core::{Error, Result};

/// A zero-copy reader over a byte slice with bounds-checked access.
#[derive(Clone)]
pub struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    /// Create a new reader over the given bytes.
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    /// Returns the underlying byte slice.
    pub fn data(&self) -> &'a [u8] {
        self.data
    }

    /// Returns the current read position.
    pub fn position(&self) -> usize {
        self.pos
    }

    /// Returns total length of the underlying data.
    pub fn len(&self) -> usize {
        self.data.len()
    }

    /// Returns true if the underlying data is empty.
    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    /// Returns how many bytes remain from the current position.
    pub fn remaining(&self) -> usize {
        self.data.len().saturating_sub(self.pos)
    }

    /// Seek to an absolute position.
    pub fn seek(&mut self, pos: usize) {
        self.pos = pos;
    }

    /// Advance the position by `n` bytes.
    pub fn skip(&mut self, n: usize) -> Result<()> {
        self.check(n)?;
        self.pos += n;
        Ok(())
    }

    /// Returns a bounds-checked sub-slice at an absolute offset.
    pub fn slice(&self, offset: usize, len: usize) -> Result<&'a [u8]> {
        let end = offset
            .checked_add(len)
            .ok_or_else(|| Error::Format("offset overflow".into()))?;
        if end > self.data.len() {
            return Err(Error::Truncated {
                needed: len,
                available: self.data.len().saturating_sub(offset),
            });
        }
        Ok(&self.data[offset..end])
    }

    /// Read `n` bytes at the current position and advance.
    pub fn read_bytes(&mut self, n: usize) -> Result<&'a [u8]> {
        let s = self.slice(self.pos, n)?;
        self.pos += n;
        Ok(s)
    }

    /// Create a sub-reader over a range of the underlying data.
    pub fn sub_reader(&self, offset: usize, len: usize) -> Result<Reader<'a>> {
        let s = self.slice(offset, len)?;
        Ok(Reader::new(s))
    }

    // --- Check helper ---

    fn check(&self, n: usize) -> Result<()> {
        if self.remaining() < n {
            Err(Error::Truncated {
                needed: n,
                available: self.remaining(),
            })
        } else {
            Ok(())
        }
    }

    // --- Single-byte reads ---

    pub fn read_u8(&mut self) -> Result<u8> {
        self.check(1)?;
        let v = self.data[self.pos];
        self.pos += 1;
        Ok(v)
    }

    pub fn read_i8(&mut self) -> Result<i8> {
        self.read_u8().map(|v| v as i8)
    }

    // --- Big-endian reads ---

    pub fn read_u16_be(&mut self) -> Result<u16> {
        let b = self.read_bytes(2)?;
        Ok(u16::from_be_bytes([b[0], b[1]]))
    }

    pub fn read_u32_be(&mut self) -> Result<u32> {
        let b = self.read_bytes(4)?;
        Ok(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }

    pub fn read_u64_be(&mut self) -> Result<u64> {
        let b = self.read_bytes(8)?;
        Ok(u64::from_be_bytes([
            b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
        ]))
    }

    pub fn read_i16_be(&mut self) -> Result<i16> {
        let b = self.read_bytes(2)?;
        Ok(i16::from_be_bytes([b[0], b[1]]))
    }

    pub fn read_i32_be(&mut self) -> Result<i32> {
        let b = self.read_bytes(4)?;
        Ok(i32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }

    pub fn read_i64_be(&mut self) -> Result<i64> {
        let b = self.read_bytes(8)?;
        Ok(i64::from_be_bytes([
            b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
        ]))
    }

    pub fn read_f32_be(&mut self) -> Result<f32> {
        let b = self.read_bytes(4)?;
        Ok(f32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }

    pub fn read_f64_be(&mut self) -> Result<f64> {
        let b = self.read_bytes(8)?;
        Ok(f64::from_be_bytes([
            b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
        ]))
    }

    // --- Little-endian reads ---

    pub fn read_u16_le(&mut self) -> Result<u16> {
        let b = self.read_bytes(2)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }

    pub fn read_u32_le(&mut self) -> Result<u32> {
        let b = self.read_bytes(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    pub fn read_u64_le(&mut self) -> Result<u64> {
        let b = self.read_bytes(8)?;
        Ok(u64::from_le_bytes([
            b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
        ]))
    }

    pub fn read_i16_le(&mut self) -> Result<i16> {
        let b = self.read_bytes(2)?;
        Ok(i16::from_le_bytes([b[0], b[1]]))
    }

    pub fn read_i32_le(&mut self) -> Result<i32> {
        let b = self.read_bytes(4)?;
        Ok(i32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    pub fn read_i64_le(&mut self) -> Result<i64> {
        let b = self.read_bytes(8)?;
        Ok(i64::from_le_bytes([
            b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
        ]))
    }

    pub fn read_f32_le(&mut self) -> Result<f32> {
        let b = self.read_bytes(4)?;
        Ok(f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    pub fn read_f64_le(&mut self) -> Result<f64> {
        let b = self.read_bytes(8)?;
        Ok(f64::from_le_bytes([
            b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
        ]))
    }

    // --- Endian-dispatched reads ---

    pub fn read_u16(&mut self, big_endian: bool) -> Result<u16> {
        if big_endian {
            self.read_u16_be()
        } else {
            self.read_u16_le()
        }
    }

    pub fn read_u32(&mut self, big_endian: bool) -> Result<u32> {
        if big_endian {
            self.read_u32_be()
        } else {
            self.read_u32_le()
        }
    }

    pub fn read_u64(&mut self, big_endian: bool) -> Result<u64> {
        if big_endian {
            self.read_u64_be()
        } else {
            self.read_u64_le()
        }
    }

    pub fn read_i16(&mut self, big_endian: bool) -> Result<i16> {
        if big_endian {
            self.read_i16_be()
        } else {
            self.read_i16_le()
        }
    }

    pub fn read_i32(&mut self, big_endian: bool) -> Result<i32> {
        if big_endian {
            self.read_i32_be()
        } else {
            self.read_i32_le()
        }
    }

    pub fn read_i64(&mut self, big_endian: bool) -> Result<i64> {
        if big_endian {
            self.read_i64_be()
        } else {
            self.read_i64_le()
        }
    }

    pub fn read_f32(&mut self, big_endian: bool) -> Result<f32> {
        if big_endian {
            self.read_f32_be()
        } else {
            self.read_f32_le()
        }
    }

    pub fn read_f64(&mut self, big_endian: bool) -> Result<f64> {
        if big_endian {
            self.read_f64_be()
        } else {
            self.read_f64_le()
        }
    }

    // --- Peek (read without advancing) ---

    /// Peek at a u8 at absolute offset without advancing position.
    pub fn peek_u8(&self, offset: usize) -> Result<u8> {
        if offset >= self.data.len() {
            return Err(Error::Truncated {
                needed: 1,
                available: self.data.len().saturating_sub(offset),
            });
        }
        Ok(self.data[offset])
    }

    /// Peek at a u16 at absolute offset.
    pub fn peek_u16(&self, offset: usize, big_endian: bool) -> Result<u16> {
        let s = self.slice(offset, 2)?;
        if big_endian {
            Ok(u16::from_be_bytes([s[0], s[1]]))
        } else {
            Ok(u16::from_le_bytes([s[0], s[1]]))
        }
    }

    /// Peek at a u32 at absolute offset.
    pub fn peek_u32(&self, offset: usize, big_endian: bool) -> Result<u32> {
        let s = self.slice(offset, 4)?;
        if big_endian {
            Ok(u32::from_be_bytes([s[0], s[1], s[2], s[3]]))
        } else {
            Ok(u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
        }
    }
}

// --- Non-advancing helpers for reading at arbitrary offsets ---

/// Read a u16 from a byte slice at the given offset.
#[allow(dead_code)] // public reader helper, kept for parity with `get_u32`
pub fn get_u16(data: &[u8], offset: usize, big_endian: bool) -> Result<u16> {
    if offset + 2 > data.len() {
        return Err(Error::Truncated {
            needed: 2,
            available: data.len().saturating_sub(offset),
        });
    }
    let b = &data[offset..offset + 2];
    if big_endian {
        Ok(u16::from_be_bytes([b[0], b[1]]))
    } else {
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }
}

/// Read a u32 from a byte slice at the given offset.
#[allow(dead_code)] // public reader helper, kept for parity with `get_u16`
pub fn get_u32(data: &[u8], offset: usize, big_endian: bool) -> Result<u32> {
    if offset + 4 > data.len() {
        return Err(Error::Truncated {
            needed: 4,
            available: data.len().saturating_sub(offset),
        });
    }
    let b = &data[offset..offset + 4];
    if big_endian {
        Ok(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    } else {
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_u8() {
        let mut r = Reader::new(&[0xAB, 0xCD]);
        assert_eq!(r.read_u8().unwrap(), 0xAB);
        assert_eq!(r.read_u8().unwrap(), 0xCD);
        assert!(r.read_u8().is_err());
    }

    #[test]
    fn read_u16_endian() {
        let data = [0x01, 0x02];
        let mut r = Reader::new(&data);
        assert_eq!(r.read_u16_be().unwrap(), 0x0102);
        r.seek(0);
        assert_eq!(r.read_u16_le().unwrap(), 0x0201);
    }

    #[test]
    fn read_u32_endian() {
        let data = [0x01, 0x02, 0x03, 0x04];
        let mut r = Reader::new(&data);
        assert_eq!(r.read_u32_be().unwrap(), 0x01020304);
        r.seek(0);
        assert_eq!(r.read_u32_le().unwrap(), 0x04030201);
    }

    #[test]
    fn read_u64_endian() {
        let data = [0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08];
        let mut r = Reader::new(&data);
        assert_eq!(r.read_u64_be().unwrap(), 0x0102030405060708);
        r.seek(0);
        assert_eq!(r.read_u64_le().unwrap(), 0x0807060504030201);
    }

    #[test]
    fn read_i16_endian() {
        let data = [0xFF, 0xFE]; // -2 in BE, -257 in LE
        let mut r = Reader::new(&data);
        assert_eq!(r.read_i16_be().unwrap(), -2);
        r.seek(0);
        assert_eq!(r.read_i16_le().unwrap(), -257);
    }

    // 3.14 / 2.718... here are arbitrary floats chosen to have an inexact
    // binary expansion, not approximations of a constant.
    #[allow(clippy::approx_constant)]
    #[test]
    fn read_f32_be() {
        let data = 3.14f32.to_be_bytes();
        let mut r = Reader::new(&data);
        let v = r.read_f32_be().unwrap();
        assert!((v - 3.14).abs() < 1e-6);
    }

    // 3.14 / 2.718... here are arbitrary floats chosen to have an inexact
    // binary expansion, not approximations of a constant.
    #[allow(clippy::approx_constant)]
    #[test]
    fn read_f64_le() {
        let data = 2.718281828459045f64.to_le_bytes();
        let mut r = Reader::new(&data);
        let v = r.read_f64_le().unwrap();
        assert!((v - 2.718281828459045).abs() < 1e-12);
    }

    #[test]
    fn endian_dispatch() {
        let data = [0x00, 0x0A];
        let mut r = Reader::new(&data);
        assert_eq!(r.read_u16(true).unwrap(), 10);
        r.seek(0);
        assert_eq!(r.read_u16(false).unwrap(), 0x0A00);
    }

    #[test]
    fn bounds_checking() {
        let mut r = Reader::new(&[0x01]);
        assert!(r.read_u16_be().is_err());
    }

    #[test]
    fn slice_bounds() {
        let r = Reader::new(&[0, 1, 2, 3]);
        assert_eq!(r.slice(1, 2).unwrap(), &[1, 2]);
        assert!(r.slice(3, 2).is_err());
    }

    #[test]
    fn sub_reader() {
        let r = Reader::new(&[0, 1, 2, 3, 4]);
        let mut sub = r.sub_reader(1, 3).unwrap();
        assert_eq!(sub.read_u8().unwrap(), 1);
        assert_eq!(sub.read_u8().unwrap(), 2);
        assert_eq!(sub.read_u8().unwrap(), 3);
        assert!(sub.read_u8().is_err());
    }

    #[test]
    fn skip_and_remaining() {
        let mut r = Reader::new(&[0; 10]);
        assert_eq!(r.remaining(), 10);
        r.skip(4).unwrap();
        assert_eq!(r.remaining(), 6);
        assert_eq!(r.position(), 4);
        assert!(r.skip(7).is_err());
    }

    #[test]
    fn peek_no_advance() {
        let r = Reader::new(&[0xAA, 0xBB, 0xCC]);
        assert_eq!(r.peek_u8(1).unwrap(), 0xBB);
        assert_eq!(r.peek_u16(0, true).unwrap(), 0xAABB);
        assert_eq!(r.peek_u16(0, false).unwrap(), 0xBBAA);
    }

    #[test]
    fn free_functions() {
        let data = [0x00, 0x01, 0x00, 0x02];
        assert_eq!(get_u16(&data, 0, true).unwrap(), 1);
        assert_eq!(get_u32(&data, 0, true).unwrap(), 0x00010002);
        assert!(get_u16(&data, 4, true).is_err());
    }

    #[test]
    fn read_bytes_advances() {
        let mut r = Reader::new(&[1, 2, 3, 4, 5]);
        let b = r.read_bytes(3).unwrap();
        assert_eq!(b, &[1, 2, 3]);
        assert_eq!(r.position(), 3);
    }
}
