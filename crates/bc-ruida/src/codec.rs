//! Byte-level encoding for Ruida controllers.
//!
//! Based on public protocol documentation (EduTech Wiki "Ruida" page) and the
//! open-source MeerK40t Ruida driver. Command bytes have the high bit set; all
//! data bytes carry 7 bits each, most significant group first.

/// Scrambling key used by RDC644xG/644xS/654xG controllers (including the RDC6445G).
pub const MAGIC_644X: u8 = 0x88;

/// Ruida controller families, which differ in their scrambling key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum RuidaModel {
    /// RDC6442/6445 (G and S), RDC6332, RDC6543G and most current boards.
    #[default]
    Rdc644x,
    /// RDC634XG (older).
    Rdc634xg,
    /// RDL9635 (laser cutter/engraver boards from that line).
    Rdl9635,
}

impl RuidaModel {
    pub const ALL: [RuidaModel; 3] = [RuidaModel::Rdc644x, RuidaModel::Rdc634xg, RuidaModel::Rdl9635];

    pub fn label(self) -> &'static str {
        match self {
            RuidaModel::Rdc644x => "RDC644x / 654x (most models)",
            RuidaModel::Rdc634xg => "RDC634XG",
            RuidaModel::Rdl9635 => "RDL9635",
        }
    }

    pub fn magic(self) -> u8 {
        match self {
            RuidaModel::Rdc644x => MAGIC_644X,
            RuidaModel::Rdc634xg => 0x11,
            RuidaModel::Rdl9635 => 0x38,
        }
    }
}

/// Replies on the UDP command port.
pub const ACK: u8 = 0xCC;
pub const ERR: u8 = 0xCD;
pub const ENQ: u8 = 0xCE;
pub const NAK: u8 = 0xCF;

fn swizzle_byte(mut b: u8, magic: u8) -> u8 {
    b ^= b >> 7;
    b ^= b << 7;
    b ^= b >> 7;
    b ^= magic;
    b.wrapping_add(1)
}

/// Byte scrambling ("swizzle") applied to everything sent to and from the controller.
#[derive(Clone)]
pub struct Swizzle {
    encode: [u8; 256],
    decode: [u8; 256],
}

impl Swizzle {
    pub fn new(magic: u8) -> Self {
        let mut encode = [0u8; 256];
        let mut decode = [0u8; 256];
        for i in 0..=255u8 {
            let e = swizzle_byte(i, magic);
            encode[i as usize] = e;
            decode[e as usize] = i;
        }
        Self { encode, decode }
    }

    pub fn encode(&self, data: &[u8]) -> Vec<u8> {
        data.iter().map(|&b| self.encode[b as usize]).collect()
    }

    pub fn decode(&self, data: &[u8]) -> Vec<u8> {
        data.iter().map(|&b| self.decode[b as usize]).collect()
    }
}

/// Two's-complement value in 14 bits (2 bytes). Range -8192..=8191, or 0..=16383 unsigned.
pub fn encode14(v: i64) -> [u8; 2] {
    let v = (v as u64) & 0x3FFF;
    [((v >> 7) & 0x7F) as u8, (v & 0x7F) as u8]
}

/// Two's-complement value in 35 bits (5 bytes).
pub fn encode35(v: i64) -> [u8; 5] {
    let v = (v as u64) & 0x7_FFFF_FFFF;
    [28, 21, 14, 7, 0].map(|shift| ((v >> shift) & 0x7F) as u8)
}

pub fn decode14(b: [u8; 2]) -> i64 {
    let v = ((b[0] as i64 & 0x7F) << 7) | (b[1] as i64 & 0x7F);
    if v > 0x1FFF { v - 0x4000 } else { v }
}

pub fn decode35(b: [u8; 5]) -> i64 {
    let v = b.iter().fold(0i64, |acc, &x| (acc << 7) | (x as i64 & 0x7F));
    if v > 0x3_FFFF_FFFF { v - 0x8_0000_0000 } else { v }
}

/// Power in percent (0–100) as a 14-bit fraction of full scale.
pub fn encode_power(percent: f64) -> [u8; 2] {
    encode14((percent.clamp(0.0, 100.0) * 16383.0 / 100.0).round() as i64)
}

/// Speed in mm/s, sent as µm/s.
pub fn encode_speed(mm_per_s: f64) -> [u8; 5] {
    encode35((mm_per_s * 1000.0).round() as i64)
}

/// A machine coordinate in µm.
pub fn encode_coord(um: i64) -> [u8; 5] {
    encode35(um)
}

/// Layer colour, red in the low byte.
pub fn encode_color([r, g, b]: [u8; 3]) -> [u8; 5] {
    encode35(r as i64 | (g as i64) << 8 | (b as i64) << 16)
}

/// UDP packet: big-endian 16-bit sum of the scrambled payload, then the payload.
pub fn udp_packet(swizzle: &Swizzle, payload: &[u8]) -> Vec<u8> {
    let data = swizzle.encode(payload);
    let sum = data.iter().fold(0u32, |acc, &b| acc + b as u32) as u16;
    let mut packet = Vec::with_capacity(data.len() + 2);
    packet.extend_from_slice(&sum.to_be_bytes());
    packet.extend_from_slice(&data);
    packet
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn swizzle_is_a_bijection_and_round_trips() {
        let s = Swizzle::new(MAGIC_644X);
        let all: Vec<u8> = (0..=255).collect();
        let enc = s.encode(&all);
        let mut sorted = enc.clone();
        sorted.sort_unstable();
        assert_eq!(sorted, all, "every byte must map to a unique byte");
        assert_eq!(s.decode(&enc), all);
    }

    #[test]
    fn swizzle_known_values() {
        // Hand-computed from the published algorithm with magic 0x88.
        // 0x00: no bit-mixing change, ^0x88 = 0x88, +1 = 0x89.
        // 0xD7 (EOF): 0xD7 ^ 1 = 0xD6; ^ (0xD6 << 7 = 0x00) = 0xD6; ^ 1 = 0xD7; ^ 0x88 = 0x5F; +1 = 0x60.
        let s = Swizzle::new(MAGIC_644X);
        assert_eq!(s.encode(&[0x00, 0xD7]), vec![0x89, 0x60]);
    }

    #[test]
    fn numbers_round_trip() {
        for v in [0, 1, -1, 8191, -8192, 1234] {
            assert_eq!(decode14(encode14(v)), v);
        }
        for v in [0, 1, -1, 600_000, -600_000, 0x3_FFFF_FFFF, -0x4_0000_0000] {
            assert_eq!(decode35(encode35(v)), v);
        }
    }

    #[test]
    fn data_bytes_never_have_the_command_bit() {
        assert!(encode35(-1).iter().chain(encode14(-1).iter()).all(|b| b & 0x80 == 0));
    }

    #[test]
    fn power_and_speed_scaling() {
        assert_eq!(encode_power(100.0), encode14(16383));
        assert_eq!(encode_power(0.0), [0, 0]);
        assert_eq!(encode_power(150.0), encode14(16383));
        assert_eq!(decode35(encode_speed(100.0)), 100_000);
    }

    #[test]
    fn packet_checksum_covers_scrambled_bytes() {
        let s = Swizzle::new(MAGIC_644X);
        let p = udp_packet(&s, &[0x00, 0xD7]);
        assert_eq!(p, vec![0x00, 0x89 + 0x60, 0x89, 0x60]);
    }
}
