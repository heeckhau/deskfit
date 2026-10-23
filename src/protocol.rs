//! Packet encoding and status decoding. See docs/protocol.md.

use std::fmt;

use serde::Serialize;
use uuid::Uuid;

pub const STATUS_CHARACTERISTIC: Uuid = Uuid::from_u128(0x0000fff1_0000_1000_8000_00805f9b34fb);
pub const COMMAND_CHARACTERISTIC: Uuid = Uuid::from_u128(0x0000fff2_0000_1000_8000_00805f9b34fb);

const COMMAND_HEADER: [u8; 3] = [0xf0, 0xc3, 0x03];
const STATUS_HEADER: [u8; 3] = [0xf0, 0xd3, 0x10];
const STATUS_LEN: usize = 20;

/// Belt speed in tenths of a level: 10 is level 1, 80 is level 8.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct Speed(u8);

impl Speed {
    pub const MIN: Speed = Speed(10);
    pub const MAX: Speed = Speed(80);

    pub fn new(raw: u8) -> Option<Speed> {
        (Self::MIN.0..=Self::MAX.0).contains(&raw).then_some(Speed(raw))
    }

    pub fn raw(self) -> u8 {
        self.0
    }
}

impl fmt::Display for Speed {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}.{}", self.0 / 10, self.0 % 10)
    }
}

/// The only commands this crate can send. The treadmill reportedly also has
/// EEPROM commands; arbitrary bytes are deliberately not expressible.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    FetchStatus,
    Start,
    Stop,
    SetSpeed(Speed),
    Pause,
}

impl Command {
    pub fn encode(self) -> [u8; 7] {
        let (command, value) = match self {
            Command::FetchStatus => (0x00, 0),
            Command::Start => (0x01, 0),
            Command::Stop => (0x02, 0),
            Command::SetSpeed(speed) => (0x03, speed.raw()),
            Command::Pause => (0x04, 0),
        };
        let mut packet = [COMMAND_HEADER[0], COMMAND_HEADER[1], COMMAND_HEADER[2], command, value, 0, 0];
        packet[6] = checksum(&packet[..6]);
        packet
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    Stopped,
    Starting,
    Running,
    Paused,
    /// A well-formed packet with a state byte we have not seen documented.
    Unknown(u8),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Status {
    pub state: State,
    /// Reported belt speed in tenths of a level. Not a `Speed`: it passes
    /// through values below 10 while the belt slows down.
    pub speed: u8,
    pub elapsed_seconds: u32,
    pub distance_hundredths_km: u32,
    pub calories: u32,
    pub steps: u32,
    /// Seconds until the belt moves, while starting.
    pub countdown: Option<u8>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum DecodeError {
    Length(usize),
    Header([u8; 3]),
    Checksum { expected: u8, actual: u8 },
}

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            DecodeError::Length(len) => write!(f, "status packet has {len} bytes, expected {STATUS_LEN}"),
            DecodeError::Header(h) => write!(f, "unexpected status header {:02x} {:02x} {:02x}", h[0], h[1], h[2]),
            DecodeError::Checksum { expected, actual } => {
                write!(f, "status checksum {actual:02x}, expected {expected:02x}")
            }
        }
    }
}

impl std::error::Error for DecodeError {}

pub fn decode_status(packet: &[u8]) -> Result<Status, DecodeError> {
    if packet.len() != STATUS_LEN {
        return Err(DecodeError::Length(packet.len()));
    }
    let header = [packet[0], packet[1], packet[2]];
    if header != STATUS_HEADER {
        return Err(DecodeError::Header(header));
    }
    let expected = checksum(&packet[..STATUS_LEN - 1]);
    let actual = packet[STATUS_LEN - 1];
    if expected != actual {
        return Err(DecodeError::Checksum { expected, actual });
    }

    let b = |i: usize| u32::from(packet[i]);
    let state = match packet[14] {
        1 => State::Stopped,
        2 => State::Starting,
        3 => State::Running,
        4 => State::Paused,
        other => State::Unknown(other),
    };
    Ok(Status {
        state,
        speed: packet[10],
        elapsed_seconds: b(4) * 60 + b(3),
        distance_hundredths_km: b(5) * 100 + b(6),
        calories: b(7) * 100 + b(8),
        steps: b(12) * 100 + b(13),
        countdown: (state == State::Starting).then_some(packet[16]),
    })
}

fn checksum(bytes: &[u8]) -> u8 {
    bytes.iter().fold(0u8, |sum, &b| sum.wrapping_add(b))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Captured from the treadmill on 2026-10-23, just after a pause.
    const PAUSED: [u8; 20] = [
        0xf0, 0xd3, 0x10, 0x1c, 0x04, 0x00, 0x1d, 0x00, 0x0c, 0x00, 0x1e, 0x00, 0x01, 0x62, 0x04, 0x00, 0x00,
        0x36, 0x00, 0xd7,
    ];

    #[test]
    fn encodes_verified_pause_packet() {
        assert_eq!(Command::Pause.encode(), [0xf0, 0xc3, 0x03, 0x04, 0x00, 0x00, 0xba]);
    }

    #[test]
    fn encodes_speed_value_and_checksum() {
        let packet = Command::SetSpeed(Speed::new(30).unwrap()).encode();
        assert_eq!(packet, [0xf0, 0xc3, 0x03, 0x03, 0x1e, 0x00, 0xd7]);
    }

    #[test]
    fn speed_is_bounded() {
        assert_eq!(Speed::new(9), None);
        assert_eq!(Speed::new(81), None);
        assert_eq!(Speed::new(10), Some(Speed::MIN));
        assert_eq!(Speed::new(80), Some(Speed::MAX));
        assert_eq!(Speed::new(25).unwrap().to_string(), "2.5");
    }

    #[test]
    fn decodes_captured_status() {
        let status = decode_status(&PAUSED).unwrap();
        assert_eq!(
            status,
            Status {
                state: State::Paused,
                speed: 30,
                elapsed_seconds: 4 * 60 + 28,
                distance_hundredths_km: 29,
                calories: 12,
                steps: 198,
                countdown: None,
            }
        );
    }

    #[test]
    fn rejects_bad_length() {
        assert_eq!(decode_status(&PAUSED[..19]), Err(DecodeError::Length(19)));
    }

    #[test]
    fn rejects_bad_header() {
        let mut packet = PAUSED;
        packet[1] = 0xb0;
        assert_eq!(decode_status(&packet), Err(DecodeError::Header([0xf0, 0xb0, 0x10])));
    }

    #[test]
    fn rejects_bad_checksum() {
        let mut packet = PAUSED;
        packet[10] = 0x1f;
        assert_eq!(decode_status(&packet), Err(DecodeError::Checksum { expected: 0xd8, actual: 0xd7 }));
    }

    #[test]
    fn reports_countdown_while_starting() {
        let mut packet = PAUSED;
        packet[14] = 2;
        packet[16] = 3;
        packet[19] = checksum(&packet[..19]);
        let status = decode_status(&packet).unwrap();
        assert_eq!(status.state, State::Starting);
        assert_eq!(status.countdown, Some(3));
    }

    #[test]
    fn keeps_unknown_state_byte() {
        let mut packet = PAUSED;
        packet[14] = 7;
        packet[19] = checksum(&packet[..19]);
        assert_eq!(decode_status(&packet).unwrap().state, State::Unknown(7));
    }
}
