//! SIGTAP wire-record parser — mirror of `keyboard-fw/left/src/sigtap.c`.
//!
//! Records on the CH343 serial are framed `AA 55` + body + CRC16. The stream
//! also carries unrelated bytes (km.version() ASCII replies, line noise), so
//! the parser resynchronises on the magic and validates the CRC, discarding
//! anything that does not check out.

pub const MAGIC0: u8 = 0xAA;
pub const MAGIC1: u8 = 0x55;
pub const VERSION: u8 = 0x01;
pub const TYPE_TAP: u8 = 1;
pub const TYPE_SYNC: u8 = 2;
pub const MAX_REPORT: usize = 64; // full-speed MPS; must match firmware SIGTAP_MAX_REPORT

// Byte offsets inside a record, measured from MAGIC0.
const OFF_VERSION: usize = 2;
const OFF_TYPE: usize = 3;
const OFF_SEQ: usize = 4;
const OFF_T_CAP: usize = 8;
const OFF_T_SUB: usize = 16;
const OFF_T_EMIT: usize = 24;
const OFF_RLEN: usize = 32;
const OFF_REPORT: usize = 33;
/// Smallest record we can size: everything up to and including `rlen`.
const HEADER_LEN: usize = OFF_REPORT;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tap {
    pub seq: u32,
    /// esp_timer µs at the lowest-level deframe of the keyboard report.
    pub t_cap_us: i64,
    /// esp_timer µs at `usbd_edpt_xfer` (USB submit to the target PC).
    pub t_sub_us: i64,
    /// esp_timer µs just before the record was written to UART0.
    pub t_emit_us: i64,
    /// Raw report bytes actually submitted (== what the host receives).
    pub report: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sync {
    pub seq: u32,
    pub t_emit_us: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Record {
    Tap(Tap),
    Sync(Sync),
}

/// CCITT-FALSE (poly 0x1021, init 0xFFFF) — identical to the firmware.
pub fn crc16_ccitt(data: &[u8]) -> u16 {
    let mut crc: u16 = 0xFFFF;
    for &b in data {
        crc ^= (b as u16) << 8;
        for _ in 0..8 {
            crc = if crc & 0x8000 != 0 {
                (crc << 1) ^ 0x1021
            } else {
                crc << 1
            };
        }
    }
    crc
}

fn rd_u32(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}
fn rd_i64(b: &[u8], o: usize) -> i64 {
    let mut a = [0u8; 8];
    a.copy_from_slice(&b[o..o + 8]);
    i64::from_le_bytes(a)
}

/// Streaming, resynchronising record parser.
#[derive(Default)]
pub struct Parser {
    buf: Vec<u8>,
    /// Records whose CRC failed (corruption / false magic that still framed).
    pub crc_errors: u64,
}

impl Parser {
    pub fn new() -> Self {
        Parser {
            buf: Vec::with_capacity(512),
            crc_errors: 0,
        }
    }

    /// Feed freshly read bytes; append any complete records to `out`.
    pub fn push(&mut self, data: &[u8], out: &mut Vec<Record>) {
        self.buf.extend_from_slice(data);
        let mut i = 0usize;
        let n = self.buf.len();
        while i + 2 <= n {
            // Find the next magic pair at or after i.
            if !(self.buf[i] == MAGIC0 && i + 1 < n && self.buf[i + 1] == MAGIC1) {
                i += 1;
                continue;
            }
            // Need the fixed header (through rlen) to size the record.
            if i + HEADER_LEN > n {
                break; // wait for more bytes
            }
            let rlen = self.buf[i + OFF_RLEN] as usize;
            if rlen > MAX_REPORT {
                i += 1; // bogus length → false magic, resync
                continue;
            }
            let total = OFF_REPORT + rlen + 2; // + CRC16
            if i + total > n {
                break; // wait for the rest of this record
            }
            let rec = &self.buf[i..i + total];
            let crc_calc = crc16_ccitt(&rec[OFF_VERSION..OFF_REPORT + rlen]);
            let crc_wire = u16::from_le_bytes([rec[OFF_REPORT + rlen], rec[OFF_REPORT + rlen + 1]]);
            if rec[OFF_VERSION] != VERSION || crc_calc != crc_wire {
                self.crc_errors += 1;
                i += 1; // not a real frame here → resync past this byte
                continue;
            }
            let seq = rd_u32(rec, OFF_SEQ);
            match rec[OFF_TYPE] {
                TYPE_TAP => out.push(Record::Tap(Tap {
                    seq,
                    t_cap_us: rd_i64(rec, OFF_T_CAP),
                    t_sub_us: rd_i64(rec, OFF_T_SUB),
                    t_emit_us: rd_i64(rec, OFF_T_EMIT),
                    report: rec[OFF_REPORT..OFF_REPORT + rlen].to_vec(),
                })),
                TYPE_SYNC => out.push(Record::Sync(Sync {
                    seq,
                    t_emit_us: rd_i64(rec, OFF_T_EMIT),
                })),
                _ => {
                    i += 1;
                    continue;
                }
            }
            i += total;
        }
        // Drop everything consumed; keep the unparsed tail for the next push.
        self.buf.drain(0..i);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a wire record exactly as the firmware would.
    fn frame(typ: u8, seq: u32, t_cap: i64, t_sub: i64, t_emit: i64, report: &[u8]) -> Vec<u8> {
        let mut b = vec![MAGIC0, MAGIC1, VERSION, typ];
        b.extend_from_slice(&seq.to_le_bytes());
        b.extend_from_slice(&t_cap.to_le_bytes());
        b.extend_from_slice(&t_sub.to_le_bytes());
        b.extend_from_slice(&t_emit.to_le_bytes());
        b.push(report.len() as u8);
        b.extend_from_slice(report);
        let crc = crc16_ccitt(&b[OFF_VERSION..]);
        b.extend_from_slice(&crc.to_le_bytes());
        b
    }

    #[test]
    fn parse_one_tap() {
        let kb = [0u8, 0, 0x04, 0, 0, 0, 0, 0];
        let bytes = frame(TYPE_TAP, 7, 1_000_000, 1_000_250, 1_005_000, &kb);
        let mut p = Parser::new();
        let mut out = Vec::new();
        p.push(&bytes, &mut out);
        assert_eq!(out.len(), 1);
        match &out[0] {
            Record::Tap(t) => {
                assert_eq!(t.seq, 7);
                assert_eq!(t.t_cap_us, 1_000_000);
                assert_eq!(t.t_sub_us, 1_000_250);
                assert_eq!(t.report, kb);
            }
            _ => panic!("expected tap"),
        }
        assert_eq!(p.crc_errors, 0);
    }

    #[test]
    fn parse_sync() {
        let bytes = frame(TYPE_SYNC, 42, 0, 0, 9_999_999, &[]);
        let mut p = Parser::new();
        let mut out = Vec::new();
        p.push(&bytes, &mut out);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0], Record::Sync(Sync { seq: 42, t_emit_us: 9_999_999 }));
    }

    #[test]
    fn resync_past_garbage_and_ascii() {
        let mut stream = b"km.version() = V1\n".to_vec(); // ASCII reply noise
        stream.push(0xAA); // a lone false magic byte
        stream.extend_from_slice(&frame(TYPE_TAP, 1, 5, 6, 7, &[0, 0, 4, 0, 0, 0, 0, 0]));
        let mut p = Parser::new();
        let mut out = Vec::new();
        p.push(&stream, &mut out);
        assert_eq!(out.len(), 1);
        if let Record::Tap(t) = &out[0] {
            assert_eq!(t.seq, 1);
        } else {
            panic!();
        }
    }

    #[test]
    fn split_across_pushes() {
        let bytes = frame(TYPE_TAP, 99, 100, 200, 300, &[1, 2, 3, 4, 5, 6, 7, 8]);
        let mut p = Parser::new();
        let mut out = Vec::new();
        // Feed one byte at a time — the worst-case fragmentation.
        for &b in &bytes {
            p.push(&[b], &mut out);
        }
        assert_eq!(out.len(), 1);
        if let Record::Tap(t) = &out[0] {
            assert_eq!(t.seq, 99);
            assert_eq!(t.report, vec![1, 2, 3, 4, 5, 6, 7, 8]);
        } else {
            panic!();
        }
    }

    #[test]
    fn corrupt_crc_is_rejected_and_recovers() {
        let mut bytes = frame(TYPE_TAP, 5, 1, 2, 3, &[0, 0, 4, 0, 0, 0, 0, 0]);
        let last = bytes.len() - 1;
        bytes[last] ^= 0xFF; // wreck the CRC
        let good = frame(TYPE_TAP, 6, 1, 2, 3, &[0, 0, 5, 0, 0, 0, 0, 0]);
        bytes.extend_from_slice(&good);
        let mut p = Parser::new();
        let mut out = Vec::new();
        p.push(&bytes, &mut out);
        assert_eq!(out.len(), 1, "only the good record survives");
        assert!(p.crc_errors >= 1);
        if let Record::Tap(t) = &out[0] {
            assert_eq!(t.seq, 6);
        } else {
            panic!();
        }
    }

    #[test]
    fn crc_matches_firmware_vector() {
        // CCITT-FALSE("123456789") == 0x29B1 (the canonical check value).
        assert_eq!(crc16_ccitt(b"123456789"), 0x29B1);
    }
}
