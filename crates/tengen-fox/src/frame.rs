//! Fox's frames: a length, a message type, a flag, the protobuf payload
//! (optionally zlib-compressed) and an Adler-32 checksum.

use std::io::{Read, Write};

use flate2::read::ZlibDecoder;
use flate2::write::ZlibEncoder;
use flate2::Compression;

/// The largest frame Tengen will accept.
pub const MAX_SIZE: usize = 8 * 1024 * 1024;
const LONG: u16 = 0x7fff;
const LONG_TAG: u16 = 11;
const RAW: u8 = 0;
const ZLIB: u8 = 2;
/// Message type of everything sent through the gateway.
pub const GATEWAY_PACKET: u16 = 26200;

#[derive(Debug, Clone, PartialEq)]
pub struct Frame {
    pub kind: u16,
    pub payload: Vec<u8>,
}

#[derive(Debug, PartialEq)]
pub struct FrameError(pub String);

pub fn adler32(data: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for &byte in data {
        a = (a + u32::from(byte)) % 65521;
        b = (b + a) % 65521;
    }
    (b << 16) | a
}

/// One frame, compressed when that makes it smaller.
pub fn encode(kind: u16, payload: &[u8]) -> Vec<u8> {
    let (flag, body) = match compress(payload) {
        Some(z) if z.len() < payload.len() => (ZLIB, z),
        _ => (RAW, payload.to_vec()),
    };
    let length = body.len() + 7;
    let mut out = Vec::with_capacity(length + 8);
    if length < LONG as usize {
        out.extend_from_slice(&(length as u16).to_be_bytes());
    } else {
        out.extend_from_slice(&LONG.to_be_bytes());
        out.extend_from_slice(&LONG_TAG.to_be_bytes());
        out.extend_from_slice(&(length as u32).to_be_bytes());
    }
    let start = out.len();
    out.extend_from_slice(&kind.to_be_bytes());
    out.push(flag);
    out.extend_from_slice(&body);
    let sum = adler32(&out[start..]);
    out.extend_from_slice(&sum.to_be_bytes());
    out
}

fn compress(data: &[u8]) -> Option<Vec<u8>> {
    if data.is_empty() {
        return None;
    }
    let mut z = ZlibEncoder::new(Vec::new(), Compression::best());
    z.write_all(data).ok()?;
    z.finish().ok()
}

/// Takes one complete frame off the front of `buf`, if one has arrived.
pub fn take(buf: &mut Vec<u8>) -> Result<Option<Frame>, FrameError> {
    if buf.len() < 2 {
        return Ok(None);
    }
    let short = u16::from_be_bytes([buf[0], buf[1]]);
    let (length, prefix) = if short == LONG {
        if buf.len() < 8 {
            return Ok(None);
        }
        (
            u32::from_be_bytes([buf[4], buf[5], buf[6], buf[7]]) as usize,
            8,
        )
    } else if short >= 0x8000 {
        return Err(FrameError("bad frame length".into()));
    } else {
        (short as usize, 2)
    };
    if !(7..=MAX_SIZE).contains(&length) {
        return Err(FrameError(format!("bad frame length {length}")));
    }
    if buf.len() < prefix + length {
        return Ok(None);
    }
    let body: Vec<u8> = buf.drain(..prefix + length).skip(prefix).collect();
    let (content, sum) = body.split_at(length - 4);
    if adler32(content) != u32::from_be_bytes([sum[0], sum[1], sum[2], sum[3]]) {
        return Err(FrameError("checksum mismatch".into()));
    }
    let kind = u16::from_be_bytes([content[0], content[1]]);
    let payload = match content[2] {
        RAW => content[3..].to_vec(),
        ZLIB => {
            let mut out = Vec::new();
            ZlibDecoder::new(&content[3..])
                .take(MAX_SIZE as u64 + 1)
                .read_to_end(&mut out)
                .map_err(|e| FrameError(format!("bad compressed payload: {e}")))?;
            if out.len() > MAX_SIZE {
                return Err(FrameError("compressed payload too large".into()));
            }
            out
        }
        other => return Err(FrameError(format!("unknown frame flag {other}"))),
    };
    Ok(Some(Frame { kind, payload }))
}

/// A gateway payload: the routing header, then the message body.
pub fn gateway_payload(header: &[u8], body: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(2 + header.len() + body.len());
    out.extend_from_slice(&(header.len() as u16).to_be_bytes());
    out.extend_from_slice(header);
    out.extend_from_slice(body);
    out
}

pub fn split_gateway(payload: &[u8]) -> Result<(&[u8], &[u8]), FrameError> {
    if payload.len() < 2 {
        return Err(FrameError("truncated gateway packet".into()));
    }
    let n = u16::from_be_bytes([payload[0], payload[1]]) as usize;
    if payload.len() < 2 + n {
        return Err(FrameError("truncated gateway header".into()));
    }
    Ok((&payload[2..2 + n], &payload[2 + n..]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adler32_matches_the_reference_value() {
        assert_eq!(adler32(b"Wikipedia"), 0x11E6_0398);
    }

    #[test]
    fn frames_round_trip_raw_and_compressed() {
        for payload in [b"short".to_vec(), vec![7u8; 5000]] {
            let mut buf = encode(2000, &payload);
            buf.extend_from_slice(&encode(3000, b"next"));
            let first = take(&mut buf).unwrap().unwrap();
            assert_eq!((first.kind, first.payload), (2000, payload.clone()));
            assert_eq!(take(&mut buf).unwrap().unwrap().kind, 3000);
            assert!(buf.is_empty());
        }
        // Repetitive data really is sent compressed.
        assert_eq!(encode(1, &[7u8; 5000])[4], ZLIB);
    }

    #[test]
    fn a_partial_frame_waits_for_the_rest() {
        let whole = encode(2005, b"hello");
        let mut buf = whole[..5].to_vec();
        assert_eq!(take(&mut buf).unwrap(), None);
        buf.extend_from_slice(&whole[5..]);
        assert_eq!(take(&mut buf).unwrap().unwrap().payload, b"hello");
    }

    #[test]
    fn a_corrupted_frame_is_refused() {
        let mut buf = encode(2005, b"hello");
        buf[6] ^= 1;
        assert!(take(&mut buf).is_err());
    }

    #[test]
    fn long_frames_use_the_long_header() {
        // Random-ish bytes don't compress, so the frame stays long.
        let mut x = 0x2545_f491_4f6c_dd1du64;
        let payload: Vec<u8> = (0..40_000)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                (x >> 24) as u8
            })
            .collect();
        let mut buf = encode(5, &payload);
        assert_eq!(&buf[..4], &[0x7f, 0xff, 0x00, 0x0b]);
        assert_eq!(take(&mut buf).unwrap().unwrap().payload, payload);
    }

    #[test]
    fn gateway_packets_split_back() {
        let p = gateway_payload(b"head", b"body");
        assert_eq!(split_gateway(&p).unwrap(), (&b"head"[..], &b"body"[..]));
    }
}
