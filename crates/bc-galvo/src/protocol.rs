//! BJJCZ / LMC galvo controller commands (the boards used by EZCad2).
//!
//! From public sources: the open-source "balor" project and MeerK40t's galvo driver.
//! Every command is 12 bytes: six little-endian u16 words (opcode + five parameters).
//! Marking commands go into lists of 256 commands (0xC00 bytes), padded with end-of-list.

// Immediate commands.
pub const DISABLE_LASER: u16 = 0x0002;
pub const ENABLE_LASER: u16 = 0x0004;
pub const EXECUTE_LIST: u16 = 0x0005;
pub const SET_PWM_PULSE_WIDTH: u16 = 0x0006;
pub const GET_VERSION: u16 = 0x0007;
pub const GET_SERIAL_NO: u16 = 0x0009;
pub const GET_LIST_STATUS: u16 = 0x000A;
pub const GET_POSITION_XY: u16 = 0x000C;
pub const GOTO_XY: u16 = 0x000D;
pub const WRITE_COR_LINE: u16 = 0x0010;
pub const RESET_LIST: u16 = 0x0012;
pub const RESTART_LIST: u16 = 0x0013;
pub const WRITE_COR_TABLE: u16 = 0x0015;
pub const SET_CONTROL_MODE: u16 = 0x0016;
pub const SET_DELAY_MODE: u16 = 0x0017;
pub const SET_END_OF_LIST: u16 = 0x0019;
pub const SET_FIRST_PULSE_KILLER: u16 = 0x001A;
pub const SET_LASER_MODE: u16 = 0x001B;
pub const SET_TIMING: u16 = 0x001C;
pub const SET_STANDBY: u16 = 0x001D;
pub const SET_PWM_HALF_PERIOD: u16 = 0x001E;
pub const STOP_EXECUTE: u16 = 0x001F;
pub const STOP_LIST: u16 = 0x0020;
pub const WRITE_PORT: u16 = 0x0021;
pub const WRITE_ANALOG_PORT_1: u16 = 0x0022;
pub const SET_FPK_PARAM_2: u16 = 0x002E;
pub const SET_FLY_RES: u16 = 0x0032;
pub const FIBER_SET_MO: u16 = 0x0033;
pub const ENABLE_Z: u16 = 0x003A;
pub const RESET: u16 = 0x0040;

// List commands.
pub const LIST_JUMP_TO: u16 = 0x8001;
pub const LIST_END_OF_LIST: u16 = 0x8002;
pub const LIST_DELAY_TIME: u16 = 0x8004;
pub const LIST_MARK_TO: u16 = 0x8005;
pub const LIST_JUMP_SPEED: u16 = 0x8006;
pub const LIST_LASER_ON_DELAY: u16 = 0x8007;
pub const LIST_LASER_OFF_DELAY: u16 = 0x8008;
pub const LIST_MARK_FREQ: u16 = 0x800A;
pub const LIST_MARK_POWER_RATIO: u16 = 0x800B;
pub const LIST_MARK_SPEED: u16 = 0x800C;
pub const LIST_JUMP_DELAY: u16 = 0x800D;
pub const LIST_POLYGON_DELAY: u16 = 0x800F;
pub const LIST_WRITE_PORT: u16 = 0x8011;
pub const LIST_MARK_CURRENT: u16 = 0x8012;
pub const LIST_QSWITCH_PERIOD: u16 = 0x801B;
pub const LIST_FIBER_YLPM_PULSE_WIDTH: u16 = 0x8026;
pub const LIST_READY_MARK: u16 = 0x8051;

/// Status bits in word 3 of the GetVersion reply.
pub const STATUS_BUSY: u16 = 0x04;
pub const STATUS_READY: u16 = 0x20;

/// Commands per list packet, and the packet size in bytes.
pub const LIST_COMMANDS: usize = 0x100;
pub const LIST_BYTES: usize = LIST_COMMANDS * 12;

/// One command: opcode and five parameters.
pub type Command = [u16; 6];

pub fn command(op: u16, params: &[u16]) -> Command {
    let mut c = [0u16; 6];
    c[0] = op;
    for (slot, p) in c[1..].iter_mut().zip(params) {
        *slot = *p;
    }
    c
}

/// 12-byte little-endian encoding.
pub fn encode(c: &Command) -> [u8; 12] {
    let mut out = [0u8; 12];
    for (i, w) in c.iter().enumerate() {
        out[i * 2..i * 2 + 2].copy_from_slice(&w.to_le_bytes());
    }
    out
}

/// Splits list commands into 0xC00-byte packets, padding the last with end-of-list.
pub fn list_packets(commands: &[Command]) -> Vec<Vec<u8>> {
    let pad = encode(&command(LIST_END_OF_LIST, &[]));
    let mut packets = Vec::new();
    for chunk in commands.chunks(LIST_COMMANDS) {
        let mut p = Vec::with_capacity(LIST_BYTES);
        for c in chunk {
            p.extend_from_slice(&encode(c));
        }
        while p.len() < LIST_BYTES {
            p.extend_from_slice(&pad);
        }
        packets.push(p);
    }
    packets
}

/// A signed delay as (magnitude, sign word): 0x0000 for positive, 0x8000 for negative.
pub fn signed_delay(us: f64) -> [u16; 2] {
    let v = us.round();
    [v.abs().min(65535.0) as u16, if v >= 0.0 { 0x0000 } else { 0x8000 }]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn little_endian_words() {
        assert_eq!(encode(&command(LIST_MARK_TO, &[0x1234, 0x8000])), [0x05, 0x80, 0x34, 0x12, 0x00, 0x80, 0, 0, 0, 0, 0, 0]);
    }

    #[test]
    fn packets_are_padded() {
        let cmds: Vec<Command> = (0..300).map(|i| command(LIST_JUMP_TO, &[i as u16])).collect();
        let packets = list_packets(&cmds);
        assert_eq!(packets.len(), 2);
        assert!(packets.iter().all(|p| p.len() == LIST_BYTES));
        // The last command slot of packet 2 is end-of-list padding.
        assert_eq!(&packets[1][LIST_BYTES - 12..LIST_BYTES - 10], &[0x02, 0x80]);
        assert_eq!(signed_delay(-100.0), [100, 0x8000]);
    }
}
