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
