//! The byte format both halves of Simple Voice Chat's protocol are written in: Minecraft's
//! `FriendlyByteBuf`. Numbers are big-endian, a length is a variable-length integer (seven bits a
//! byte, the high bit saying another follows), a string is its length in bytes then UTF-8, and a UUID
//! is two longs, the most significant half first.

/// A reader over a packet's bytes. Every read says whether the bytes were there.
pub struct Reader<'a> {
    data: &'a [u8],
}

impl<'a> Reader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data }
    }

    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    pub fn rest(&self) -> &'a [u8] {
        self.data
    }

    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        if self.data.len() < n {
            return None;
        }
        let (head, tail) = self.data.split_at(n);
        self.data = tail;
        Some(head)
    }

    pub fn u8(&mut self) -> Option<u8> {
        self.take(1).map(|b| b[0])
    }

    pub fn bool(&mut self) -> Option<bool> {
        self.u8().map(|b| b != 0)
    }

    pub fn i16(&mut self) -> Option<i16> {
        self.take(2).map(|b| i16::from_be_bytes([b[0], b[1]]))
    }

    pub fn i32(&mut self) -> Option<i32> {
        self.take(4)
            .map(|b| i32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }

    pub fn i64(&mut self) -> Option<i64> {
        self.take(8)
            .map(|b| i64::from_be_bytes(b.try_into().unwrap()))
    }

    #[cfg(test)]
    pub fn f32(&mut self) -> Option<f32> {
        self.take(4)
            .map(|b| f32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }

    #[cfg(test)]
    pub fn f64(&mut self) -> Option<f64> {
        self.take(8)
            .map(|b| f64::from_be_bytes(b.try_into().unwrap()))
    }

    pub fn uuid(&mut self) -> Option<u128> {
        self.take(16)
            .map(|b| u128::from_be_bytes(b.try_into().unwrap()))
    }

    /// A variable-length integer of at most five bytes.
    pub fn varint(&mut self) -> Option<u32> {
        let mut value = 0u32;
        for shift in 0..5 {
            let byte = self.u8()?;
            value |= u32::from(byte & 0x7f) << (7 * shift);
            if byte & 0x80 == 0 {
                return Some(value);
            }
        }
        None
    }

    /// A length, then that many bytes, no more than `max`.
    pub fn byte_array(&mut self, max: usize) -> Option<&'a [u8]> {
        let len = self.varint()? as usize;
        if len > max {
            return None;
        }
        self.take(len)
    }

    /// A string of at most `max` characters.
    pub fn utf(&mut self, max: usize) -> Option<String> {
        let len = self.varint()? as usize;
        if len > max.checked_mul(3)? {
            return None;
        }
        let text = String::from_utf8(self.take(len)?.to_vec()).ok()?;
        (text.chars().count() <= max).then_some(text)
    }
}

/// A packet being written.
#[derive(Default)]
pub struct Writer {
    data: Vec<u8>,
}

impl Writer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn finish(self) -> Vec<u8> {
        self.data
    }

    pub fn raw(&mut self, bytes: &[u8]) -> &mut Self {
        self.data.extend_from_slice(bytes);
        self
    }

    pub fn u8(&mut self, v: u8) -> &mut Self {
        self.raw(&[v])
    }

    pub fn bool(&mut self, v: bool) -> &mut Self {
        self.u8(u8::from(v))
    }

    pub fn i16(&mut self, v: i16) -> &mut Self {
        self.raw(&v.to_be_bytes())
    }

    pub fn i32(&mut self, v: i32) -> &mut Self {
        self.raw(&v.to_be_bytes())
    }

    pub fn i64(&mut self, v: i64) -> &mut Self {
        self.raw(&v.to_be_bytes())
    }

    pub fn f32(&mut self, v: f32) -> &mut Self {
        self.raw(&v.to_be_bytes())
    }

    pub fn f64(&mut self, v: f64) -> &mut Self {
        self.raw(&v.to_be_bytes())
    }

    pub fn uuid(&mut self, v: u128) -> &mut Self {
        self.raw(&v.to_be_bytes())
    }

    pub fn varint(&mut self, mut v: u32) -> &mut Self {
        loop {
            if v < 0x80 {
                return self.u8(v as u8);
            }
            self.u8((v & 0x7f) as u8 | 0x80);
            v >>= 7;
        }
    }

    pub fn byte_array(&mut self, bytes: &[u8]) -> &mut Self {
        self.varint(bytes.len() as u32).raw(bytes)
    }

    pub fn utf(&mut self, text: &str) -> &mut Self {
        self.byte_array(text.as_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_varint_is_seven_bits_a_byte_with_the_high_bit_for_more() {
        let mut w = Writer::new();
        w.varint(0)
            .varint(127)
            .varint(128)
            .varint(300)
            .varint(2_097_151);
        assert_eq!(
            w.finish(),
            [0x00, 0x7f, 0x80, 0x01, 0xac, 0x02, 0xff, 0xff, 0x7f]
        );
        let mut r = Reader::new(&[0x80, 0x01, 0xac, 0x02, 0xff, 0xff, 0xff, 0xff, 0x0f]);
        assert_eq!(r.varint(), Some(128));
        assert_eq!(r.varint(), Some(300));
        assert_eq!(r.varint(), Some(u32::MAX));
        // Six continuation bytes is not a varint.
        assert_eq!(Reader::new(&[0x80; 6]).varint(), None);
    }

    #[test]
    fn a_uuid_is_two_longs_the_high_half_first() {
        let mut w = Writer::new();
        w.uuid(0x0102_0304_0506_0708_090a_0b0c_0d0e_0f10);
        assert_eq!(
            w.finish(),
            [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16]
        );
    }

    #[test]
    fn numbers_are_big_endian_and_a_short_read_is_none() {
        let mut w = Writer::new();
        w.i16(-2).i32(24454).i64(-1).f32(24.0).f64(48.0).bool(true);
        let bytes = w.finish();
        assert_eq!(&bytes[..2], [0xff, 0xfe]);
        assert_eq!(&bytes[2..6], [0, 0, 0x5f, 0x86]);
        let mut r = Reader::new(&bytes);
        assert_eq!(r.i16(), Some(-2));
        assert_eq!(r.i32(), Some(24454));
        assert_eq!(r.i64(), Some(-1));
        assert_eq!(r.f32(), Some(24.0));
        assert_eq!(r.f64(), Some(48.0));
        assert_eq!(r.bool(), Some(true));
        assert!(r.is_empty());
        assert_eq!(r.u8(), None);
        assert_eq!(Reader::new(&[0, 0, 0]).i32(), None);
    }

    #[test]
    fn a_string_and_an_array_are_limited() {
        let mut w = Writer::new();
        w.utf("Friends").utf("héllo").byte_array(&[1, 2, 3]);
        let bytes = w.finish();
        assert_eq!(&bytes[..8], b"\x07Friends");
        let mut r = Reader::new(&bytes);
        assert_eq!(r.utf(24).as_deref(), Some("Friends"));
        assert_eq!(r.utf(24).as_deref(), Some("héllo"));
        assert_eq!(r.byte_array(3), Some(&[1u8, 2, 3][..]));
        // Too long a string, or array, is refused.
        assert_eq!(Reader::new(&bytes).utf(3), None);
        assert_eq!(Reader::new(&[3, 1, 2, 3]).byte_array(2), None);
        // Bytes that are not UTF-8 are not a string.
        assert_eq!(Reader::new(&[1, 0xff]).utf(8), None);
    }
}
