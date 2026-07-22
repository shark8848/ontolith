//! Deterministic canonical encoding helpers (SAS-0401 GO-005 / P1-04 baseline).
//!
//! Encoding rules (R1 baseline):
//! - length-prefixed UTF-8 fields: `u32 LE length || bytes`
//! - tagged variants start with a short ASCII tag byte sequence
//! - no unstable map iteration order is introduced by this module itself
//!
//! Higher layers (RDF statements, graphs) compose these primitives.

/// Growable buffer used while building a canonical byte representation.
#[derive(Debug, Default, Clone)]
pub struct CanonicalWriter {
    buf: Vec<u8>,
}

impl CanonicalWriter {
    pub fn new() -> Self {
        Self { buf: Vec::new() }
    }

    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            buf: Vec::with_capacity(capacity),
        }
    }

    pub fn write_tag(&mut self, tag: &[u8]) {
        self.buf.extend_from_slice(tag);
    }

    pub fn write_u8(&mut self, value: u8) {
        self.buf.push(value);
    }

    pub fn write_u64(&mut self, value: u64) {
        self.buf.extend_from_slice(&value.to_le_bytes());
    }

    pub fn write_bytes(&mut self, bytes: &[u8]) {
        let len = bytes.len() as u32;
        self.buf.extend_from_slice(&len.to_le_bytes());
        self.buf.extend_from_slice(bytes);
    }

    pub fn write_str(&mut self, value: &str) {
        self.write_bytes(value.as_bytes());
    }

    pub fn as_slice(&self) -> &[u8] {
        &self.buf
    }

    pub fn into_bytes(self) -> Vec<u8> {
        self.buf
    }

    /// Hex encoding for tests and debug logs (not a storage format).
    pub fn to_hex(&self) -> String {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        let mut out = String::with_capacity(self.buf.len() * 2);
        for byte in &self.buf {
            out.push(HEX[(byte >> 4) as usize] as char);
            out.push(HEX[(byte & 0x0f) as usize] as char);
        }
        out
    }
}

/// Types that can produce a deterministic byte encoding.
pub trait CanonicalEncode {
    fn write_canonical(&self, out: &mut CanonicalWriter);

    fn canonical_bytes(&self) -> Vec<u8> {
        let mut out = CanonicalWriter::new();
        self.write_canonical(&mut out);
        out.into_bytes()
    }

    fn canonical_hex(&self) -> String {
        let mut out = CanonicalWriter::new();
        self.write_canonical(&mut out);
        out.to_hex()
    }
}

impl CanonicalEncode for str {
    fn write_canonical(&self, out: &mut CanonicalWriter) {
        out.write_str(self);
    }
}

impl CanonicalEncode for String {
    fn write_canonical(&self, out: &mut CanonicalWriter) {
        out.write_str(self);
    }
}

impl CanonicalEncode for u64 {
    fn write_canonical(&self, out: &mut CanonicalWriter) {
        out.write_u64(*self);
    }
}
