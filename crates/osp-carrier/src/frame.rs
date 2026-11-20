//! Frames on a link between neighbours, and the commands only a circuit's
//! last hop reads.
//!
//! A link frame starts with the version, its kind and a circuit ID that
//! means something only on that link: each neighbour connection gives a
//! circuit its own, so no ID goes along the whole path.
//!
//! ```text
//! CREATE      version · kind · circuit ID · setup (80) · backward start (8)
//! CREATED     version · kind · circuit ID
//! DATA        version · kind (its direction) · circuit ID · packet number (8) · payload
//! BUSY        version · kind · circuit ID
//! DESTROY     version · kind · circuit ID · reason
//! ```
//!
//! A `DATA` payload is under the layers. Under the last hop's is a command
//! byte and its body, so a hop in the middle can't tell a packet from an
//! `EXTEND`:
//!
//! ```text
//! EXTEND        setup (80) · the next carrier's descriptor
//! EXTENDED
//! CONNECT_HOST  host slot (16)
//! CONNECTED
//! DATA          the ends' packet
//! BUSY
//! ```
//!
//! Nothing names a place to connect to except a carrier's descriptor and a
//! host slot. Anything malformed or unknown is refused.

use anyhow::{Result, bail, ensure};

use crate::{Descriptor, Direction, SETUP_LEN, Setup, VERSION};

pub type CircuitId = u32;
/// A host slot: a random, short-lived token the host's hop gave the host's
/// connection, with no address in it.
pub type Slot = [u8; SLOT_LEN];
pub const SLOT_LEN: usize = 16;
/// Largest link frame read. QUIC's packets are at least 1200 bytes, and the
/// frame adds 63 with three hops.
pub const MAX_FRAME: usize = 2048;

const CREATE: u8 = 1;
const CREATED: u8 = 2;
const DATA_TO_HOST: u8 = 3;
const DATA_TO_VIEWER: u8 = 4;
const BUSY: u8 = 5;
const DESTROY: u8 = 6;

/// Why a circuit ended, in `DESTROY`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DestroyReason {
    /// Its end closed it.
    Closed,
    /// A setup, an extend or a host slot was refused.
    Refused,
    /// A hop timed out.
    Timeout,
    /// A packet failed a check, or a frame was malformed.
    Protocol,
    /// The next hop went, or a link failed.
    Gone,
}

impl DestroyReason {
    fn byte(self) -> u8 {
        match self {
            Self::Closed => 0,
            Self::Refused => 1,
            Self::Timeout => 2,
            Self::Protocol => 3,
            Self::Gone => 4,
        }
    }

    fn from_byte(b: u8) -> Self {
        match b {
            0 => Self::Closed,
            1 => Self::Refused,
            2 => Self::Timeout,
            4 => Self::Gone,
            // A reason this version doesn't know still ends the circuit.
            _ => Self::Protocol,
        }
    }
}

/// A frame on a link between neighbours.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Frame {
    /// Sets up a hop's layer. `backward_start` is where that hop numbers its
    /// own packets towards the viewer: 0 from the viewer, and from a hop
    /// extending the circuit, past the ones it still uses.
    Create {
        circuit: CircuitId,
        setup: Setup,
        backward_start: u64,
    },
    Created {
        circuit: CircuitId,
    },
    Data {
        circuit: CircuitId,
        direction: Direction,
        pn: u64,
        payload: Vec<u8>,
    },
    /// Not now: the carrier is at a limit. A normal answer.
    Busy {
        circuit: CircuitId,
    },
    Destroy {
        circuit: CircuitId,
        reason: DestroyReason,
    },
}

impl Frame {
    pub fn circuit(&self) -> CircuitId {
        match self {
            Self::Create { circuit, .. }
            | Self::Created { circuit }
            | Self::Data { circuit, .. }
            | Self::Busy { circuit }
            | Self::Destroy { circuit, .. } => *circuit,
        }
    }

    pub fn encode(&self) -> Vec<u8> {
        let head = |kind: u8, circuit: CircuitId, extra: usize| {
            let mut out = Vec::with_capacity(6 + extra);
            out.push(VERSION);
            out.push(kind);
            out.extend_from_slice(&circuit.to_be_bytes());
            out
        };
        match self {
            Self::Create {
                circuit,
                setup,
                backward_start,
            } => {
                let mut out = head(CREATE, *circuit, SETUP_LEN + 8);
                out.extend_from_slice(&setup.to_bytes());
                out.extend_from_slice(&backward_start.to_be_bytes());
                out
            }
            Self::Created { circuit } => head(CREATED, *circuit, 0),
            Self::Data {
                circuit,
                direction,
                pn,
                payload,
            } => {
                let kind = match direction {
                    Direction::ToHost => DATA_TO_HOST,
                    Direction::ToViewer => DATA_TO_VIEWER,
                };
                let mut out = head(kind, *circuit, 8 + payload.len());
                out.extend_from_slice(&pn.to_be_bytes());
                out.extend_from_slice(payload);
                out
            }
            Self::Busy { circuit } => head(BUSY, *circuit, 0),
            Self::Destroy { circuit, reason } => {
                let mut out = head(DESTROY, *circuit, 1);
                out.push(reason.byte());
                out
            }
        }
    }

    pub fn decode(bytes: &[u8]) -> Result<Self> {
        ensure!(bytes.len() <= MAX_FRAME, "a frame larger than {MAX_FRAME}");
        ensure!(bytes.len() >= 6, "a frame too short");
        ensure!(bytes[0] == VERSION, "carrier frame version {}", bytes[0]);
        let circuit = CircuitId::from_be_bytes(bytes[2..6].try_into().expect("4 bytes"));
        let rest = &bytes[6..];
        let empty = |frame: Frame| -> Result<Frame> {
            ensure!(rest.is_empty(), "bytes after a frame");
            Ok(frame)
        };
        match bytes[1] {
            CREATE => {
                ensure!(rest.len() == SETUP_LEN + 8, "a CREATE of the wrong size");
                Ok(Self::Create {
                    circuit,
                    setup: Setup::from_bytes(&rest[..SETUP_LEN])?,
                    backward_start: u64::from_be_bytes(
                        rest[SETUP_LEN..].try_into().expect("8 bytes"),
                    ),
                })
            }
            CREATED => empty(Self::Created { circuit }),
            kind @ (DATA_TO_HOST | DATA_TO_VIEWER) => {
                ensure!(rest.len() >= 8, "a DATA frame too short");
                Ok(Self::Data {
                    circuit,
                    direction: if kind == DATA_TO_HOST {
                        Direction::ToHost
                    } else {
                        Direction::ToViewer
                    },
                    pn: u64::from_be_bytes(rest[..8].try_into().expect("8 bytes")),
                    payload: rest[8..].to_vec(),
                })
            }
            BUSY => empty(Self::Busy { circuit }),
            DESTROY => {
                ensure!(rest.len() == 1, "a DESTROY of the wrong size");
                Ok(Self::Destroy {
                    circuit,
                    reason: DestroyReason::from_byte(rest[0]),
                })
            }
            kind => bail!("unknown carrier frame kind {kind}"),
        }
    }
}

const R_EXTEND: u8 = 1;
const R_EXTENDED: u8 = 2;
const R_CONNECT_HOST: u8 = 3;
const R_CONNECTED: u8 = 4;
const R_DATA: u8 = 5;
const R_BUSY: u8 = 6;

/// A command under the layers, for the circuit's last hop or from it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Relay {
    /// Towards the host: reach this carrier next, and pass it this setup.
    Extend {
        setup: Setup,
        descriptor: Descriptor,
    },
    /// Towards the viewer: the next carrier took its setup.
    Extended,
    /// Towards the host, to the host's hop: join the host connection that
    /// holds this slot.
    ConnectHost { slot: Slot },
    /// Towards the viewer: joined.
    Connected,
    /// The ends' packet, either way.
    Data(Vec<u8>),
    /// Towards the viewer: the next carrier, or this one, is at a limit.
    Busy,
}

impl Relay {
    pub fn encode(&self) -> Vec<u8> {
        match self {
            Self::Extend { setup, descriptor } => {
                let d = descriptor.to_bytes();
                let mut out = Vec::with_capacity(1 + SETUP_LEN + d.len());
                out.push(R_EXTEND);
                out.extend_from_slice(&setup.to_bytes());
                out.extend_from_slice(&d);
                out
            }
            Self::Extended => vec![R_EXTENDED],
            Self::ConnectHost { slot } => [&[R_CONNECT_HOST][..], slot].concat(),
            Self::Connected => vec![R_CONNECTED],
            Self::Data(packet) => {
                let mut out = Vec::with_capacity(1 + packet.len() + crate::TAG * crate::MAX_HOPS);
                out.push(R_DATA);
                out.extend_from_slice(packet);
                out
            }
            Self::Busy => vec![R_BUSY],
        }
    }

    pub fn decode(mut bytes: Vec<u8>) -> Result<Self> {
        ensure!(!bytes.is_empty(), "an empty command");
        let rest = &bytes[1..];
        let empty = |r: Relay| -> Result<Relay> {
            ensure!(rest.is_empty(), "bytes after a command");
            Ok(r)
        };
        match bytes[0] {
            R_EXTEND => {
                ensure!(rest.len() > SETUP_LEN, "an EXTEND too short");
                Ok(Self::Extend {
                    setup: Setup::from_bytes(&rest[..SETUP_LEN])?,
                    descriptor: Descriptor::from_bytes(&rest[SETUP_LEN..])?,
                })
            }
            R_EXTENDED => empty(Self::Extended),
            R_CONNECT_HOST => {
                ensure!(rest.len() == SLOT_LEN, "a CONNECT_HOST of the wrong size");
                Ok(Self::ConnectHost {
                    slot: rest.try_into().expect("16 bytes"),
                })
            }
            R_CONNECTED => empty(Self::Connected),
            R_DATA => {
                bytes.remove(0);
                Ok(Self::Data(bytes))
            }
            R_BUSY => empty(Self::Busy),
            cmd => bail!("unknown carrier command {cmd}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use iroh_base::SecretKey;

    use super::*;
    use crate::{CarrierKeys, descriptor::tests::info};

    fn setup() -> (Setup, Descriptor) {
        let machine = SecretKey::from_bytes(&[1; 32]);
        let keys = CarrierKeys::new(machine.public(), 1_000);
        let mut i = info(&machine);
        i.circuit_key = keys.current().public();
        let d = Descriptor::new(&machine, &i).unwrap();
        (Setup::begin(&i).unwrap().0, d)
    }

    #[test]
    fn frames_round_trip() {
        let (s, _) = setup();
        let frames = [
            Frame::Create {
                circuit: 7,
                setup: s,
                backward_start: 42,
            },
            Frame::Created { circuit: 7 },
            Frame::Data {
                circuit: u32::MAX,
                direction: Direction::ToHost,
                pn: 9,
                payload: vec![3; 1263],
            },
            Frame::Data {
                circuit: 0,
                direction: Direction::ToViewer,
                pn: u64::MAX,
                payload: vec![],
            },
            Frame::Busy { circuit: 1 },
            Frame::Destroy {
                circuit: 1,
                reason: DestroyReason::Gone,
            },
        ];
        for f in frames {
            assert_eq!(Frame::decode(&f.encode()).unwrap(), f);
        }
        let data = Frame::Data {
            circuit: 1,
            direction: Direction::ToHost,
            pn: 0,
            payload: vec![],
        };
        assert_eq!(data.encode().len(), crate::DATA_HEADER);
    }

    #[test]
    fn malformed_frames_are_refused() {
        let good = Frame::Created { circuit: 1 }.encode();
        let mut v2 = good.clone();
        v2[0] = 2;
        let mut unknown = good.clone();
        unknown[1] = 200;
        let mut trailing = good.clone();
        trailing.push(0);
        for bad in [
            vec![],
            good[..5].to_vec(),
            v2,
            unknown,
            trailing,
            vec![VERSION; MAX_FRAME + 1],
        ] {
            assert!(Frame::decode(&bad).is_err(), "{bad:?}");
        }
        let (s, _) = setup();
        let create = Frame::Create {
            circuit: 1,
            setup: s,
            backward_start: 0,
        }
        .encode();
        assert!(Frame::decode(&create[..create.len() - 1]).is_err());
        let data = Frame::Data {
            circuit: 1,
            direction: Direction::ToHost,
            pn: 0,
            payload: vec![],
        }
        .encode();
        assert!(Frame::decode(&data[..data.len() - 1]).is_err());
    }

    #[test]
    fn an_unknown_destroy_reason_still_ends_the_circuit() {
        let mut f = Frame::Destroy {
            circuit: 1,
            reason: DestroyReason::Closed,
        }
        .encode();
        f[6] = 99;
        assert_eq!(
            Frame::decode(&f).unwrap(),
            Frame::Destroy {
                circuit: 1,
                reason: DestroyReason::Protocol
            }
        );
    }

    #[test]
    fn commands_round_trip() {
        let (s, d) = setup();
        for r in [
            Relay::Extend {
                setup: s,
                descriptor: d,
            },
            Relay::Extended,
            Relay::ConnectHost {
                slot: [5; SLOT_LEN],
            },
            Relay::Connected,
            Relay::Data(vec![1, 2, 3]),
            Relay::Data(vec![]),
            Relay::Busy,
        ] {
            assert_eq!(Relay::decode(r.encode()).unwrap(), r);
        }
    }

    #[test]
    fn malformed_commands_are_refused() {
        let (s, d) = setup();
        let extend = Relay::Extend {
            setup: s,
            descriptor: d,
        }
        .encode();
        for bad in [
            vec![],
            vec![0],
            vec![99],
            vec![R_EXTENDED, 0],
            vec![R_CONNECT_HOST; 1 + SLOT_LEN + 1],
            vec![R_CONNECT_HOST; SLOT_LEN],
            extend[..1 + SETUP_LEN].to_vec(),
            extend[..extend.len() - 1].to_vec(),
        ] {
            assert!(Relay::decode(bad.clone()).is_err(), "{bad:?}");
        }
    }
}
