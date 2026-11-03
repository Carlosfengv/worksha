//! Bounded wire reader for the fields verified in Antigravity App 2.23.1.
//! Unknown fields are skipped. No binary scraping, executable or runtime dependency.
use super::{NativeError, NativeResult};

#[derive(Clone, Copy)]
enum Value<'a> {
    Number(u64),
    Bytes(&'a [u8]),
    Fixed,
}
#[derive(Default)]
pub(super) struct Message<'a>(Vec<(u32, Value<'a>)>);
fn invalid() -> NativeError {
    NativeError::new("unsupported", "Antigravity Protobuf 结构无效或超出读取限制")
}
fn varint(bytes: &[u8], pos: &mut usize) -> NativeResult<u64> {
    let mut value = 0;
    for shift in (0..70).step_by(7) {
        let byte = *bytes.get(*pos).ok_or_else(invalid)?;
        *pos += 1;
        if shift == 63 && byte > 1 {
            return Err(invalid());
        }
        value |= u64::from(byte & 127) << shift;
        if byte < 128 {
            return Ok(value);
        }
    }
    Err(invalid())
}
impl<'a> Message<'a> {
    pub fn parse(bytes: &'a [u8]) -> NativeResult<Self> {
        if bytes.len() > 8 * 1024 * 1024 {
            return Err(invalid());
        }
        let mut fields = Vec::new();
        let mut pos = 0;
        while pos < bytes.len() {
            if fields.len() >= 16384 {
                return Err(invalid());
            }
            let tag = varint(bytes, &mut pos)?;
            let field = u32::try_from(tag >> 3).map_err(|_| invalid())?;
            if field == 0 || field > 0x1fffffff {
                return Err(invalid());
            }
            let value = match tag & 7 {
                0 => Value::Number(varint(bytes, &mut pos)?),
                1 | 5 => {
                    pos = pos
                        .checked_add(if tag & 7 == 1 { 8 } else { 4 })
                        .ok_or_else(invalid)?;
                    if pos > bytes.len() {
                        return Err(invalid());
                    }
                    Value::Fixed
                }
                2 => {
                    let len = usize::try_from(varint(bytes, &mut pos)?).map_err(|_| invalid())?;
                    let end = pos.checked_add(len).ok_or_else(invalid)?;
                    let value = bytes.get(pos..end).ok_or_else(invalid)?;
                    pos = end;
                    Value::Bytes(value)
                }
                _ => return Err(invalid()),
            };
            fields.push((field, value));
        }
        Ok(Self(fields))
    }
    pub fn bytes(&self, field: u32) -> Option<&'a [u8]> {
        self.0.iter().rev().find_map(|(n, v)| match v {
            Value::Bytes(b) if *n == field => Some(*b),
            _ => None,
        })
    }
    pub fn all(&self, field: u32) -> impl Iterator<Item = &'a [u8]> + '_ {
        self.0.iter().filter_map(move |(n, v)| match v {
            Value::Bytes(b) if *n == field => Some(*b),
            _ => None,
        })
    }
    pub fn number(&self, field: u32) -> Option<u64> {
        self.0.iter().rev().find_map(|(n, v)| match v {
            Value::Number(v) if *n == field => Some(*v),
            _ => None,
        })
    }
    pub fn text(&self, field: u32) -> Option<&'a str> {
        std::str::from_utf8(self.bytes(field)?).ok()
    }
    pub fn child(&self, field: u32) -> NativeResult<Self> {
        Self::parse(self.bytes(field).unwrap_or_default())
    }
    pub fn time(&self, field: u32) -> Option<String> {
        let timestamp = self.child(field).ok()?;
        // Proto3 omits a zero nanosecond value.
        let nanos = u32::try_from(timestamp.number(2).unwrap_or(0)).ok()?;
        if nanos >= 1_000_000_000 {
            return None;
        }
        let seconds = i64::try_from(timestamp.number(1)?).ok()?;
        chrono::DateTime::from_timestamp(seconds, nanos).map(|t| t.to_rfc3339())
    }
}
