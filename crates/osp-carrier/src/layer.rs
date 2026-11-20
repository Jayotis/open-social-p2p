//! The keys for each hop, and the layer each one adds or takes off.
//!
//! The viewer sets up a context with each hop by HPKE (RFC 9180) in Base
//! mode, with one suite: `DHKEM(X25519, HKDF-SHA256), HKDF-SHA256,
//! ChaCha20-Poly1305`. HPKE's exporter gives a key for each direction and a
//! context the layers authenticate. Base mode is deliberate: the viewer checks
//! the carrier through its signed descriptor, and the carrier learns nothing
//! about who the viewer is.
//!
//! Packet numbers only go up, in each direction, on each layer. Every layer
//! keeps a window of the numbers it has used, opening or sealing, and refuses
//! one it has used or one older than the window. So no key seals two packets
//! with the same nonce, and a repeated packet is dropped.
//!
//! Towards the host, only the viewer numbers packets. Towards the viewer, the
//! circuit's last hop does, starting where the hop before it stopped
//! ([`Layer::next_own`], passed on in `CREATE`), so the hops nearer the viewer
//! never see a number twice.

use std::collections::{HashMap, HashSet};

use anyhow::{Context, Result, bail, ensure};
use chacha20poly1305::{AeadInOut, ChaCha20Poly1305, KeyInit, Nonce};
use hpke::{
    Deserializable, Kem, OpModeR, OpModeS, Serializable, kdf::HkdfSha256, kem::X25519HkdfSha256,
};
use iroh_base::EndpointId;

use crate::{CarrierInfo, PROTOCOL, Relay, VERSION};

type HpkeAead = hpke::aead::ChaCha20Poly1305;
type PublicKey = <X25519HkdfSha256 as Kem>::PublicKey;
type PrivateKey = <X25519HkdfSha256 as Kem>::PrivateKey;
type EncappedKey = <X25519HkdfSha256 as Kem>::EncappedKey;

/// Packets a layer carries each way before its circuit must be replaced. It
/// never wraps.
pub const MAX_PACKETS: u64 = 1 << 40;
/// Packet numbers a layer remembers behind the newest it has used. One older
/// than this is dropped, as is one inside it it has used.
pub const WINDOW: u64 = 1024;
const WORDS: usize = (WINDOW / 64) as usize;
/// How long a circuit key is used before it's replaced (a starting value),
/// and how long the old one is still taken for circuits being built.
pub const KEY_LIFETIME_SECS: u64 = 24 * 60 * 60;
pub const KEY_GRACE_SECS: u64 = 10 * 60;
/// Bytes in a [`Setup`]: the circuit key it's for, a fresh nonce, and HPKE's
/// encapsulated key.
pub const SETUP_LEN: usize = 32 + NONCE_LEN + 32;
const NONCE_LEN: usize = 16;

/// Which way a packet goes along a circuit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Direction {
    /// From the viewer, through each hop, to the host.
    ToHost,
    /// From the host, back to the viewer.
    ToViewer,
}

impl Direction {
    fn byte(self) -> u8 {
        match self {
            Self::ToHost => 0,
            Self::ToViewer => 1,
        }
    }
}

/// What a viewer sends a hop to set up its layer, in `CREATE` or in
/// `EXTEND`. Made for one hop on one circuit: its nonce and encapsulated key
/// are new each time, so nothing in it matches another hop's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Setup {
    /// Which of the carrier's circuit keys it's for.
    pub circuit_key: [u8; 32],
    nonce: [u8; NONCE_LEN],
    enc: [u8; 32],
}

impl Setup {
    /// Sets up a layer with the carrier `info` describes, which the caller
    /// has checked ([`Descriptor::verify`](crate::Descriptor::verify)) and
    /// meant to reach.
    pub fn begin(info: &CarrierInfo) -> Result<(Self, Layer)> {
        let key = PublicKey::from_bytes(&info.circuit_key)
            .map_err(|e| anyhow::anyhow!("not a circuit key: {e}"))?;
        let mut nonce = [0; NONCE_LEN];
        getrandom::fill(&mut nonce).map_err(|e| anyhow::anyhow!("no random numbers: {e}"))?;
        let (enc, ctx) = hpke::setup_sender::<HpkeAead, HkdfSha256, X25519HkdfSha256>(
            &OpModeS::Base,
            &key,
            &hpke_info(&info.machine, &nonce),
        )
        .map_err(|e| anyhow::anyhow!("HPKE setup failed: {e}"))?;
        let layer = Layer::from_exporter(|label, out| ctx.export(label, out))?;
        let setup = Self {
            circuit_key: info.circuit_key,
            nonce,
            enc: enc.to_bytes().into(),
        };
        Ok((setup, layer))
    }

    pub fn to_bytes(&self) -> [u8; SETUP_LEN] {
        let mut out = [0; SETUP_LEN];
        out[..32].copy_from_slice(&self.circuit_key);
        out[32..32 + NONCE_LEN].copy_from_slice(&self.nonce);
        out[32 + NONCE_LEN..].copy_from_slice(&self.enc);
        out
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        ensure!(bytes.len() == SETUP_LEN, "a setup is {SETUP_LEN} bytes");
        Ok(Self {
            circuit_key: bytes[..32].try_into().expect("32 bytes"),
            nonce: bytes[32..32 + NONCE_LEN].try_into().expect("16 bytes"),
            enc: bytes[32 + NONCE_LEN..].try_into().expect("32 bytes"),
        })
    }
}

/// What the HPKE context is bound to: the protocol, its version, the
/// carrier's machine ID and the circuit's nonce for that hop.
fn hpke_info(machine: &EndpointId, nonce: &[u8; NONCE_LEN]) -> Vec<u8> {
    [PROTOCOL, &[VERSION], machine.as_bytes(), nonce].concat()
}

/// A carrier's X25519 circuit key, for one day.
pub struct CircuitKey {
    secret: PrivateKey,
    public: [u8; 32],
    made: u64,
}

impl CircuitKey {
    /// A new key, made at `now` (unix seconds).
    pub fn new(now: u64) -> Self {
        let (secret, public) = X25519HkdfSha256::gen_keypair();
        Self {
            secret,
            public: public.to_bytes().into(),
            made: now,
        }
    }

    pub fn public(&self) -> [u8; 32] {
        self.public
    }

    /// For the descriptor: when viewers may set circuits up with it.
    pub fn valid_from(&self) -> u64 {
        self.made
    }

    pub fn valid_until(&self) -> u64 {
        self.made + KEY_LIFETIME_SECS
    }

    /// Past this, the carrier refuses setups for it.
    fn taken_until(&self) -> u64 {
        self.valid_until() + KEY_GRACE_SECS
    }
}

/// A carrier's circuit keys, and every setup it has taken with each, so a
/// repeated one is refused until the key it used expires.
pub struct CarrierKeys {
    machine: EndpointId,
    current: CircuitKey,
    previous: Option<CircuitKey>,
    seen: HashMap<[u8; 32], HashSet<[u8; 32]>>,
}

impl CarrierKeys {
    pub fn new(machine: EndpointId, now: u64) -> Self {
        Self {
            machine,
            current: CircuitKey::new(now),
            previous: None,
            seen: HashMap::new(),
        }
    }

    /// The key to publish in the descriptor now.
    pub fn current(&self) -> &CircuitKey {
        &self.current
    }

    /// Replaces the current key once its day is up, keeping the old one for
    /// circuits being built. True when the descriptor must be published again.
    pub fn rotate_if_due(&mut self, now: u64) -> bool {
        self.forget_expired(now);
        if now < self.current.valid_until() {
            return false;
        }
        let old = std::mem::replace(&mut self.current, CircuitKey::new(now));
        if let Some(dropped) = self.previous.replace(old) {
            self.seen.remove(&dropped.public);
        }
        true
    }

    fn forget_expired(&mut self, now: u64) {
        if self
            .previous
            .as_ref()
            .is_some_and(|p| now > p.taken_until())
        {
            let dropped = self.previous.take().expect("checked");
            self.seen.remove(&dropped.public);
        }
    }

    /// Takes a viewer's setup, at `now`: the layer, or why not. A setup it
    /// has already taken is refused, as is one for a key that has expired or
    /// was never its own.
    pub fn accept(&mut self, setup: &Setup, now: u64) -> Result<Layer> {
        self.forget_expired(now);
        let key = if setup.circuit_key == self.current.public {
            &self.current
        } else if let Some(p) = self
            .previous
            .as_ref()
            .filter(|p| p.public == setup.circuit_key)
        {
            p
        } else {
            bail!("a setup for a circuit key this carrier doesn't have");
        };
        ensure!(
            now <= key.taken_until(),
            "a setup for a circuit key that has expired"
        );
        let seen = self.seen.entry(key.public).or_default();
        ensure!(!seen.contains(&setup.enc), "a setup that was already taken");
        let enc = EncappedKey::from_bytes(&setup.enc)
            .map_err(|e| anyhow::anyhow!("not an encapsulated key: {e}"))?;
        let ctx = hpke::setup_receiver::<HpkeAead, HkdfSha256, X25519HkdfSha256>(
            &OpModeR::Base,
            &key.secret,
            &enc,
            &hpke_info(&self.machine, &setup.nonce),
        )
        .map_err(|e| anyhow::anyhow!("HPKE setup failed: {e}"))?;
        let layer = Layer::from_exporter(|label, out| ctx.export(label, out))?;
        seen.insert(setup.enc);
        Ok(layer)
    }

    /// Setups remembered, for limits and tests.
    pub fn remembered(&self) -> usize {
        self.seen.values().map(HashSet::len).sum()
    }
}

/// The packet numbers one layer has used in one direction.
#[derive(Debug, Clone)]
struct Window {
    /// One past the newest used.
    top: u64,
    bits: [u64; WORDS],
}

impl Window {
    fn new() -> Self {
        Self {
            top: 0,
            bits: [0; WORDS],
        }
    }

    fn bit(pn: u64) -> (usize, u64) {
        let i = pn % WINDOW;
        ((i / 64) as usize, 1 << (i % 64))
    }

    fn fresh(&self, pn: u64) -> bool {
        if pn >= MAX_PACKETS {
            return false;
        }
        if pn >= self.top {
            return true;
        }
        if self.top - pn > WINDOW {
            return false;
        }
        let (w, b) = Self::bit(pn);
        self.bits[w] & b == 0
    }

    fn mark(&mut self, pn: u64) {
        debug_assert!(self.fresh(pn));
        if pn >= self.top {
            if pn - self.top >= WINDOW {
                self.bits = [0; WORDS];
            } else {
                for old in self.top..pn {
                    let (w, b) = Self::bit(old);
                    self.bits[w] &= !b;
                }
            }
            self.top = pn + 1;
        }
        let (w, b) = Self::bit(pn);
        self.bits[w] |= b;
    }
}

/// One hop's layer: its key each way, the context it authenticates, and the
/// packet numbers it has used.
pub struct Layer {
    to_host: ChaCha20Poly1305,
    to_viewer: ChaCha20Poly1305,
    context: [u8; 32],
    used_to_host: Window,
    used_to_viewer: Window,
    /// The next number this hop gives a packet it sends towards the viewer
    /// itself, as the circuit's last hop.
    next_own: u64,
}

impl Layer {
    fn from_exporter(
        export: impl Fn(&[u8], &mut [u8]) -> Result<(), hpke::HpkeError>,
    ) -> Result<Self> {
        let take = |label: &str| -> Result<[u8; 32]> {
            let mut out = [0; 32];
            export(format!("osp/carrier/1 {label}").as_bytes(), &mut out)
                .map_err(|e| anyhow::anyhow!("HPKE export failed: {e}"))?;
            Ok(out)
        };
        let cipher = |k: [u8; 32]| ChaCha20Poly1305::new(&k.into());
        Ok(Self {
            to_host: cipher(take("towards the host")?),
            to_viewer: cipher(take("towards the viewer")?),
            context: take("circuit context")?,
            used_to_host: Window::new(),
            used_to_viewer: Window::new(),
            next_own: 0,
        })
    }

    fn parts(&mut self, dir: Direction) -> (&ChaCha20Poly1305, &mut Window) {
        match dir {
            Direction::ToHost => (&self.to_host, &mut self.used_to_host),
            Direction::ToViewer => (&self.to_viewer, &mut self.used_to_viewer),
        }
    }

    fn aad(&self, dir: Direction, pn: u64) -> [u8; 42] {
        let mut aad = [0; 42];
        aad[0] = VERSION;
        aad[1] = dir.byte();
        aad[2..10].copy_from_slice(&pn.to_be_bytes());
        aad[10..].copy_from_slice(&self.context);
        aad
    }

    fn nonce(pn: u64) -> Nonce {
        let mut n = [0; 12];
        n[4..].copy_from_slice(&pn.to_be_bytes());
        n.into()
    }

    fn fresh(&self, dir: Direction, pn: u64) -> bool {
        match dir {
            Direction::ToHost => self.used_to_host.fresh(pn),
            Direction::ToViewer => self.used_to_viewer.fresh(pn),
        }
    }

    /// Adds this hop's layer to `buf`, as packet `pn` going `dir`.
    pub fn seal(&mut self, dir: Direction, pn: u64, buf: &mut Vec<u8>) -> Result<()> {
        let aad = self.aad(dir, pn);
        let (cipher, used) = self.parts(dir);
        ensure!(used.fresh(pn), "packet number {pn} was used already");
        cipher
            .encrypt_in_place(&Self::nonce(pn), &aad, buf)
            .map_err(|_| anyhow::anyhow!("a packet too large to seal"))?;
        used.mark(pn);
        Ok(())
    }

    /// Takes this hop's layer off `buf`, packet `pn` going `dir`. A packet
    /// repeated, too old, or failing the check is refused, and `buf` is left
    /// in no useful state.
    pub fn open(&mut self, dir: Direction, pn: u64, buf: &mut Vec<u8>) -> Result<()> {
        ensure!(self.fresh(dir, pn), "packet number {pn} was used already");
        self.open_unmarked(dir, pn, buf)?;
        self.parts(dir).1.mark(pn);
        Ok(())
    }

    fn open_unmarked(&self, dir: Direction, pn: u64, buf: &mut Vec<u8>) -> Result<()> {
        let cipher = match dir {
            Direction::ToHost => &self.to_host,
            Direction::ToViewer => &self.to_viewer,
        };
        cipher
            .decrypt_in_place(&Self::nonce(pn), &self.aad(dir, pn), buf)
            .map_err(|_| anyhow::anyhow!("a packet that fails its check"))
    }

    /// The next number this hop would give a packet of its own towards the
    /// viewer. When it extends the circuit, the next hop starts after the
    /// ones it will still use (`CREATE`'s `backward_start`).
    pub fn next_own(&self) -> u64 {
        self.next_own
    }

    /// Where this hop starts numbering its own packets towards the viewer,
    /// as `CREATE` said. Only ever moves forward.
    pub fn start_own_at(&mut self, start: u64) {
        self.next_own = self.next_own.max(start);
    }

    /// As the circuit's last hop, sends `relay` towards the viewer under this
    /// hop's layer: the packet number and the payload.
    pub fn originate(&mut self, relay: &Relay) -> Result<(u64, Vec<u8>)> {
        let pn = self.next_own;
        ensure!(pn < MAX_PACKETS, "this circuit must be replaced");
        let mut buf = relay.encode();
        self.seal(Direction::ToViewer, pn, &mut buf)?;
        self.next_own = pn + 1;
        Ok((pn, buf))
    }

    /// As the circuit's last hop, reads a cell from the viewer: takes this
    /// hop's layer off, then the command under it.
    pub fn receive(&mut self, pn: u64, mut payload: Vec<u8>) -> Result<Relay> {
        self.open(Direction::ToHost, pn, &mut payload)?;
        Relay::decode(payload)
    }
}

/// The viewer's side of a circuit: a layer for each hop, the viewer's first.
#[derive(Default)]
pub struct Onion {
    hops: Vec<Layer>,
    next: u64,
}

impl Onion {
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds the layer of a hop the circuit now reaches, after its last.
    pub fn push(&mut self, layer: Layer) {
        self.hops.push(layer);
    }

    pub fn hops(&self) -> usize {
        self.hops.len()
    }

    /// True once the circuit has used most of its packet numbers, so it's
    /// time to build another.
    pub fn needs_replacing(&self) -> bool {
        self.next >= MAX_PACKETS - MAX_PACKETS / 16
    }

    /// Wraps `relay` for the circuit's last hop, then each before it: the
    /// packet number and the payload for the first hop's `DATA` frame.
    pub fn wrap(&mut self, relay: &Relay) -> Result<(u64, Vec<u8>)> {
        ensure!(!self.hops.is_empty(), "a circuit with no hops");
        let pn = self.next;
        ensure!(pn < MAX_PACKETS, "this circuit must be replaced");
        let mut buf = relay.encode();
        for layer in self.hops.iter_mut().rev() {
            layer.seal(Direction::ToHost, pn, &mut buf)?;
        }
        self.next = pn + 1;
        Ok((pn, buf))
    }

    /// Takes every hop's layer off a packet that came back, then reads the
    /// command the circuit's last hop put under them. Nothing is marked used
    /// unless every layer passes.
    pub fn unwrap(&mut self, pn: u64, mut payload: Vec<u8>) -> Result<Relay> {
        ensure!(!self.hops.is_empty(), "a circuit with no hops");
        ensure!(
            self.hops.iter().all(|l| l.fresh(Direction::ToViewer, pn)),
            "packet number {pn} was used already"
        );
        for layer in &self.hops {
            layer
                .open_unmarked(Direction::ToViewer, pn, &mut payload)
                .context("a hop's layer")?;
        }
        for layer in &mut self.hops {
            layer.used_to_viewer.mark(pn);
        }
        Relay::decode(payload)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::descriptor::tests::info;
    use iroh_base::SecretKey;

    /// A carrier for tests: its machine key and its circuit keys.
    struct Carrier {
        info: CarrierInfo,
        keys: CarrierKeys,
    }

    const NOW: u64 = 1_000_000;

    fn carrier(seed: u8) -> Carrier {
        let machine = SecretKey::from_bytes(&[seed; 32]);
        let keys = CarrierKeys::new(machine.public(), NOW);
        let mut info = info(&machine);
        info.circuit_key = keys.current().public();
        Carrier { info, keys }
    }

    /// Builds a circuit through `carriers` as the viewer would, handing each
    /// carrier its setup directly: the viewer's onion and each hop's layer.
    fn build(carriers: &mut [Carrier]) -> (Onion, Vec<Layer>) {
        let mut onion = Onion::new();
        let mut layers = Vec::new();
        for c in carriers.iter_mut() {
            let (setup, mine) = Setup::begin(&c.info).unwrap();
            let setup = Setup::from_bytes(&setup.to_bytes()).unwrap();
            let theirs = c.keys.accept(&setup, NOW).unwrap();
            onion.push(mine);
            layers.push(theirs);
        }
        (onion, layers)
    }

    /// Sends `packet` from the viewer to the last hop through every hop.
    fn to_host(onion: &mut Onion, hops: &mut [Layer], packet: &[u8]) -> Result<Relay> {
        let (pn, mut payload) = onion.wrap(&Relay::Data(packet.to_vec()))?;
        let (last, before) = hops.split_last_mut().unwrap();
        for hop in before {
            hop.open(Direction::ToHost, pn, &mut payload)?;
        }
        last.receive(pn, payload)
    }

    /// Sends `packet` from the last hop back to the viewer.
    fn to_viewer(onion: &mut Onion, hops: &mut [Layer], packet: &[u8]) -> Result<Relay> {
        let (last, before) = hops.split_last_mut().unwrap();
        let (pn, mut payload) = last.originate(&Relay::Data(packet.to_vec()))?;
        for hop in before.iter_mut().rev() {
            hop.seal(Direction::ToViewer, pn, &mut payload)?;
        }
        onion.unwrap(pn, payload)
    }

    #[test]
    fn a_packet_goes_both_ways_through_three_hops() {
        let mut carriers = [carrier(1), carrier(2), carrier(3)];
        let (mut onion, mut hops) = build(&mut carriers);
        for i in 0..5u8 {
            let packet = vec![i; 1200];
            assert_eq!(
                to_host(&mut onion, &mut hops, &packet).unwrap(),
                Relay::Data(packet.clone())
            );
            assert_eq!(
                to_viewer(&mut onion, &mut hops, &packet).unwrap(),
                Relay::Data(packet)
            );
        }
    }

    #[test]
    fn the_overhead_is_as_documented() {
        let mut carriers = [carrier(1), carrier(2), carrier(3)];
        let (mut onion, _) = build(&mut carriers);
        let (_, payload) = onion.wrap(&Relay::Data(vec![0; 1200])).unwrap();
        assert_eq!(
            crate::DATA_HEADER + payload.len(),
            1200 + crate::overhead(3)
        );
        assert_eq!(crate::overhead(3), 63);
    }

    #[test]
    fn leaving_out_a_hop_fails() {
        for skipped in 0..3 {
            let mut carriers = [carrier(1), carrier(2), carrier(3)];
            let (mut onion, mut hops) = build(&mut carriers);
            let sent = Relay::Data(vec![1; 1200]);
            let (pn, mut payload) = onion.wrap(&sent).unwrap();
            let mut failed = false;
            for (i, hop) in hops.iter_mut().enumerate() {
                if i == skipped {
                    continue;
                }
                if hop.open(Direction::ToHost, pn, &mut payload).is_err() {
                    failed = true;
                    break;
                }
            }
            // A hop before the one left out finds the next layer fails its
            // check. Leaving out the last leaves its layer on: what reaches
            // the host isn't the packet sent, and the ends' QUIC drops it.
            if skipped < 2 {
                assert!(failed, "skipping hop {skipped} wasn't caught by a layer");
            } else {
                assert!(!failed);
                assert_ne!(payload, sent.encode());
                assert_eq!(payload.len(), sent.encode().len() + crate::TAG);
            }
        }
    }

    #[test]
    fn hops_in_the_wrong_order_fail() {
        let mut carriers = [carrier(1), carrier(2), carrier(3)];
        let (mut onion, mut hops) = build(&mut carriers);
        let (pn, mut payload) = onion.wrap(&Relay::Data(vec![1; 1200])).unwrap();
        assert!(hops[1].open(Direction::ToHost, pn, &mut payload).is_err());
    }

    #[test]
    fn a_changed_byte_fails_at_the_next_hop() {
        let mut carriers = [carrier(1), carrier(2), carrier(3)];
        let (mut onion, mut hops) = build(&mut carriers);
        let (pn, payload) = onion.wrap(&Relay::Data(vec![1; 64])).unwrap();
        for i in 0..payload.len() {
            let mut bad = payload.clone();
            bad[i] ^= 0x40;
            assert!(hops[0].open(Direction::ToHost, pn, &mut bad).is_err());
        }
        // P is honest, and M changes a byte before passing it on: Q refuses it.
        let mut p = payload;
        hops[0].open(Direction::ToHost, pn, &mut p).unwrap();
        hops[1].open(Direction::ToHost, pn, &mut p).unwrap();
        p[3] ^= 1;
        assert!(hops[2].receive(pn, p).is_err());
        // Towards the viewer too.
        let (pn, mut back) = hops[2].originate(&Relay::Data(vec![2; 64])).unwrap();
        hops[1].seal(Direction::ToViewer, pn, &mut back).unwrap();
        back[0] ^= 1;
        hops[0].seal(Direction::ToViewer, pn, &mut back).unwrap();
        assert!(onion.unwrap(pn, back).is_err());
    }

    #[test]
    fn a_changed_packet_number_or_direction_fails() {
        let mut carriers = [carrier(1)];
        let (mut onion, mut hops) = build(&mut carriers);
        let (pn, payload) = onion.wrap(&Relay::Data(vec![1; 64])).unwrap();
        assert!(hops[0].receive(pn + 1, payload.clone()).is_err());
        let mut p = payload.clone();
        assert!(hops[0].open(Direction::ToViewer, pn, &mut p).is_err());
        assert!(hops[0].receive(pn, payload).is_ok());
    }

    #[test]
    fn a_repeated_packet_is_refused() {
        let mut carriers = [carrier(1), carrier(2)];
        let (mut onion, mut hops) = build(&mut carriers);
        let (pn, payload) = onion.wrap(&Relay::Data(vec![1; 64])).unwrap();
        let mut first = payload.clone();
        hops[0].open(Direction::ToHost, pn, &mut first).unwrap();
        let mut again = payload;
        assert!(hops[0].open(Direction::ToHost, pn, &mut again).is_err());
        // A repeat that gets past a dishonest first hop is caught at the next.
        hops[1].receive(pn, first.clone()).unwrap();
        assert!(hops[1].receive(pn, first).is_err());
        // And towards the viewer.
        let (pn, mut back) = hops[1].originate(&Relay::Data(vec![2; 64])).unwrap();
        hops[0].seal(Direction::ToViewer, pn, &mut back).unwrap();
        onion.unwrap(pn, back.clone()).unwrap();
        assert!(onion.unwrap(pn, back).is_err());
    }

    #[test]
    fn a_hop_never_seals_a_number_twice() {
        let mut carriers = [carrier(1), carrier(2)];
        let (_, mut hops) = build(&mut carriers);
        let mut a = vec![0; 10];
        hops[0].seal(Direction::ToViewer, 5, &mut a).unwrap();
        let mut b = vec![1; 10];
        assert!(hops[0].seal(Direction::ToViewer, 5, &mut b).is_err());
        // A last hop that starts below what the hop before it used gets its
        // packets dropped there, not sealed again.
        hops[1].start_own_at(5);
        let (pn, mut c) = hops[1].originate(&Relay::Extended).unwrap();
        assert_eq!(pn, 5);
        assert!(hops[0].seal(Direction::ToViewer, pn, &mut c).is_err());
    }

    #[test]
    fn packets_out_of_order_inside_the_window_pass() {
        let mut carriers = [carrier(1)];
        let (mut onion, mut hops) = build(&mut carriers);
        let sent: Vec<_> = (0..10)
            .map(|i| onion.wrap(&Relay::Data(vec![i; 8])).unwrap())
            .collect();
        for (pn, payload) in sent.into_iter().rev() {
            assert_eq!(
                hops[0].receive(pn, payload).unwrap(),
                Relay::Data(vec![pn as u8; 8])
            );
        }
    }

    #[test]
    fn the_window() {
        let mut w = Window::new();
        w.mark(0);
        assert!(!w.fresh(0));
        w.mark(WINDOW + 5);
        assert!(!w.fresh(5), "older than the window");
        assert!(w.fresh(6));
        w.mark(6);
        assert!(!w.fresh(6));
        assert!(w.fresh(WINDOW + 4));
        assert!(!w.fresh(WINDOW + 5));
        // A jump past the whole window clears it.
        w.mark(10 * WINDOW);
        assert!(w.fresh(9 * WINDOW + 1));
        assert!(!w.fresh(9 * WINDOW));
        assert!(!w.fresh(MAX_PACKETS));
    }

    #[test]
    fn a_repeated_setup_is_refused() {
        let mut c = carrier(1);
        let (setup, _) = Setup::begin(&c.info).unwrap();
        c.keys.accept(&setup, NOW).unwrap();
        assert!(c.keys.accept(&setup, NOW).is_err());
        assert_eq!(c.keys.remembered(), 1);
    }

    #[test]
    fn a_setup_for_another_carrier_gives_keys_that_dont_match() {
        // Jimmy's node sets up a layer for carrier 1, and carrier 2 (with
        // carrier 1's circuit key, say a copy) takes it: its machine ID is
        // bound in, so the layers don't agree.
        let mut one = carrier(1);
        let mut two = carrier(2);
        two.keys = CarrierKeys {
            machine: two.info.machine,
            current: CircuitKey {
                secret: one.keys.current.secret.clone(),
                public: one.keys.current.public,
                made: NOW,
            },
            previous: None,
            seen: HashMap::new(),
        };
        let (setup, mine) = Setup::begin(&one.info).unwrap();
        let mut onion = Onion::new();
        onion.push(mine);
        let mut theirs = two.keys.accept(&setup, NOW).unwrap();
        let (pn, payload) = onion.wrap(&Relay::Data(vec![1; 8])).unwrap();
        assert!(theirs.receive(pn, payload.clone()).is_err());
        assert!(
            one.keys
                .accept(&setup, NOW)
                .unwrap()
                .receive(pn, payload)
                .is_ok()
        );
    }

    #[test]
    fn keys_are_replaced_daily_and_the_old_one_kept_briefly() {
        let mut c = carrier(1);
        let (old_setup, _) = Setup::begin(&c.info).unwrap();
        assert!(!c.keys.rotate_if_due(NOW + 60));
        assert!(c.keys.rotate_if_due(NOW + KEY_LIFETIME_SECS));
        assert_ne!(c.keys.current().public(), c.info.circuit_key);
        // Taken in the grace period, refused after it.
        let (late, _) = Setup::begin(&c.info).unwrap();
        c.keys
            .accept(&old_setup, NOW + KEY_LIFETIME_SECS + 60)
            .unwrap();
        assert!(
            c.keys
                .accept(&late, NOW + KEY_LIFETIME_SECS + KEY_GRACE_SECS + 1)
                .is_err()
        );
        assert_eq!(c.keys.remembered(), 0, "the old key's setups are forgotten");
        // A setup for a key it never had.
        let other = carrier(2);
        let (stranger, _) = Setup::begin(&other.info).unwrap();
        assert!(c.keys.accept(&stranger, NOW).is_err());
    }

    #[test]
    fn a_key_left_unreplaced_stops_being_taken() {
        let mut c = carrier(1);
        let (setup, _) = Setup::begin(&c.info).unwrap();
        assert!(
            c.keys
                .accept(&setup, NOW + KEY_LIFETIME_SECS + KEY_GRACE_SECS + 1)
                .is_err()
        );
    }

    #[test]
    fn setups_differ_for_each_hop() {
        let c = carrier(1);
        let (a, _) = Setup::begin(&c.info).unwrap();
        let (b, _) = Setup::begin(&c.info).unwrap();
        assert_eq!(a.circuit_key, b.circuit_key);
        assert_ne!(a.nonce, b.nonce);
        assert_ne!(a.enc, b.enc);
    }
}
