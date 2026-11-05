//! Just enough protobuf to read and write Fox's messages without their
//! schema: fields by number, varints and length-delimited bytes.

use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Int(u64),
    Bytes(Vec<u8>),
}

/// A decoded message: every value seen for each field number, in order.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Msg {
    fields: BTreeMap<u32, Vec<Value>>,
}

#[derive(Debug, PartialEq)]
pub struct DecodeError(pub &'static str);

impl Msg {
    pub fn decode(data: &[u8]) -> Result<Msg, DecodeError> {
        let mut msg = Msg::default();
        let mut at = 0;
        while at < data.len() {
            let tag = read_varint(data, &mut at)?;
            let (number, wire) = ((tag >> 3) as u32, tag & 7);
            if number == 0 {
                return Err(DecodeError("field number 0"));
            }
            let value = match wire {
                0 => Value::Int(read_varint(data, &mut at)?),
                2 => {
                    let len = read_varint(data, &mut at)? as usize;
                    let end = at
                        .checked_add(len)
                        .filter(|&e| e <= data.len())
                        .ok_or(DecodeError("truncated bytes"))?;
                    let bytes = data[at..end].to_vec();
                    at = end;
                    Value::Bytes(bytes)
                }
                1 | 5 => {
                    let size = if wire == 1 { 8 } else { 4 };
                    let end = at
                        .checked_add(size)
                        .filter(|&e| e <= data.len())
                        .ok_or(DecodeError("truncated fixed"))?;
                    let mut buf = [0u8; 8];
                    buf[..size].copy_from_slice(&data[at..end]);
                    at = end;
                    Value::Int(u64::from_le_bytes(buf))
                }
                _ => return Err(DecodeError("unsupported wire type")),
            };
            msg.fields.entry(number).or_default().push(value);
        }
        Ok(msg)
    }

    pub fn has(&self, n: u32) -> bool {
        self.fields.contains_key(&n)
    }

    fn last(&self, n: u32) -> Option<&Value> {
        self.fields.get(&n).and_then(|v| v.last())
    }

    pub fn int(&self, n: u32) -> Option<u64> {
        match self.last(n)? {
            Value::Int(i) => Some(*i),
            Value::Bytes(_) => None,
        }
    }

    /// An int32 field, which protobuf sends sign-extended to 64 bits.
    pub fn i32(&self, n: u32) -> Option<i32> {
        self.int(n).map(|v| v as i64 as i32)
    }

    pub fn u(&self, n: u32) -> u64 {
        self.int(n).unwrap_or(0)
    }

    pub fn bytes(&self, n: u32) -> Option<&[u8]> {
        match self.last(n)? {
            Value::Bytes(b) => Some(b),
            Value::Int(_) => None,
        }
    }

    pub fn text(&self, n: u32) -> String {
        self.bytes(n)
            .map(|b| String::from_utf8_lossy(b).into_owned())
            .unwrap_or_default()
    }

    /// A nested message; empty when absent or unreadable.
    pub fn sub(&self, n: u32) -> Msg {
        self.bytes(n)
            .and_then(|b| Msg::decode(b).ok())
            .unwrap_or_default()
    }

    /// Every nested message of a repeated field.
    pub fn subs(&self, n: u32) -> Vec<Msg> {
        self.fields
            .get(&n)
            .map(|vs| {
                vs.iter()
                    .filter_map(|v| match v {
                        Value::Bytes(b) => Msg::decode(b).ok(),
                        Value::Int(_) => None,
                    })
                    .collect()
            })
            .unwrap_or_default()
    }
}

fn read_varint(data: &[u8], at: &mut usize) -> Result<u64, DecodeError> {
    let mut value = 0u64;
    for shift in (0..70).step_by(7) {
        let b = *data.get(*at).ok_or(DecodeError("truncated varint"))?;
        *at += 1;
        if shift == 63 && b > 1 {
            return Err(DecodeError("varint overflow"));
        }
        value |= u64::from(b & 0x7f) << shift;
        if b < 0x80 {
            return Ok(value);
        }
    }
    Err(DecodeError("varint overflow"))
}

/// Builds a message field by field, in the order given.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Pb(Vec<u8>);

impl Pb {
    pub fn new() -> Self {
        Pb(Vec::new())
    }

    pub fn int(mut self, n: u32, v: u64) -> Self {
        put_varint(&mut self.0, u64::from(n) << 3);
        put_varint(&mut self.0, v);
        self
    }

    /// An int32 (negative values are sign-extended, as protobuf does).
    pub fn i32(self, n: u32, v: i32) -> Self {
        self.int(n, v as i64 as u64)
    }

    pub fn bool(self, n: u32, v: bool) -> Self {
        self.int(n, u64::from(v))
    }

    pub fn bytes(mut self, n: u32, v: &[u8]) -> Self {
        put_varint(&mut self.0, (u64::from(n) << 3) | 2);
        put_varint(&mut self.0, v.len() as u64);
        self.0.extend_from_slice(v);
        self
    }

    pub fn str(self, n: u32, v: &str) -> Self {
        self.bytes(n, v.as_bytes())
    }

    pub fn msg(self, n: u32, v: Pb) -> Self {
        self.bytes(n, &v.0)
    }

    pub fn build(self) -> Vec<u8> {
        self.0
    }
}

fn put_varint(out: &mut Vec<u8>, mut v: u64) {
    while v >= 0x80 {
        out.push((v as u8 & 0x7f) | 0x80);
        v >>= 7;
    }
    out.push(v as u8);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_ints_text_and_nesting() {
        let bytes = Pb::new()
            .int(1, 300)
            .str(2, "fox")
            .msg(3, Pb::new().int(1, 7))
            .i32(4, -2)
            .bool(5, true)
            .build();
        let m = Msg::decode(&bytes).unwrap();
        assert_eq!(m.int(1), Some(300));
        assert_eq!(m.text(2), "fox");
        assert_eq!(m.sub(3).int(1), Some(7));
        assert_eq!(m.i32(4), Some(-2));
        assert_eq!(m.u(5), 1);
        assert_eq!(m.u(99), 0);
    }

    #[test]
    fn repeated_fields_keep_every_value() {
        let bytes = Pb::new()
            .msg(9, Pb::new().int(1, 1))
            .msg(9, Pb::new().int(1, 2))
            .build();
        let users = Msg::decode(&bytes).unwrap().subs(9);
        assert_eq!(users.iter().map(|u| u.u(1)).collect::<Vec<_>>(), vec![1, 2]);
    }

    #[test]
    fn known_encoding() {
        // Field 1 = 150 is the protobuf documentation's example: 08 96 01.
        assert_eq!(Pb::new().int(1, 150).build(), vec![0x08, 0x96, 0x01]);
    }

    #[test]
    fn truncated_input_is_an_error() {
        assert!(Msg::decode(&[0x12, 0x05, b'a']).is_err());
        assert!(Msg::decode(&[0x08, 0x96]).is_err());
    }
}
