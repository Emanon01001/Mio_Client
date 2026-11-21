//! The connected session's round-trip time, observed passively on the game's
//! own UDP traffic to the server: nothing is sent and nothing is changed.
//!
//! Only RakNet's framing is read (the game's packets inside it are encrypted
//! and are never looked at):
//! - a connected ping (`0x00`, the game's own) and the server's pong (`0x03`)
//!   echoing its time give an exact round trip;
//! - between those, each data datagram the game sends is numbered and the
//!   server acknowledges it (an ACK datagram); the time from sending to the
//!   acknowledgement is a round trip plus the server's short ACK delay. Of
//!   the datagrams one ACK covers, the most recently sent has waited least,
//!   so the smallest time per ACK is taken.
//!
//! NetherNet (WebRTC) encrypts everything (DTLS) except ICE's connectivity
//! checks: the game keeps sending STUN Binding requests to the peer (every
//! few seconds on an established connection) and the peer answers with the
//! same transaction id, so request-to-response is a round trip. Through a
//! TURN relay they travel wrapped (ChannelData, Send/Data indications) and
//! are unwrapped once; TURN's own requests to the relay are not counted.
use std::collections::VecDeque;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::sync::Mutex;

/// A peer as the WinSock hooks see it: (family, address, port), IPv4-mapped
/// IPv6 already turned into IPv4 (see `net`).
pub(crate) type Peer = (u16, [u8; 16], u16);

const AF_INET: u16 = 2;
const AF_INET6: u16 = 23;
/// Sent datagrams remembered for matching acknowledgements (by number).
const DATAGRAMS: usize = 512;
/// Connected pings remembered for matching pongs.
const PINGS: usize = 8;
/// ACK samples kept (one per acknowledgement datagram).
const ACK_SAMPLES: usize = 32;
/// A pong-based reading is shown for this long (ms) before ACKs take over.
const PING_FRESH_MS: f64 = 10_000.;
/// ACK samples older than this (ms) are not used.
const ACK_FRESH_MS: f64 = 3_000.;
/// Fewest recent ACK samples for an ACK-based reading.
const MIN_ACK_SAMPLES: usize = 3;
/// STUN Binding requests remembered for matching responses.
const STUNS: usize = 16;
/// STUN round trips kept; requests come every few seconds.
const STUN_SAMPLES: usize = 8;
/// STUN round trips older than this (ms) are not used.
const STUN_FRESH_MS: f64 = 15_000.;
/// STUN's fixed magic cookie (RFC 5389).
const MAGIC_COOKIE: [u8; 4] = [0x21, 0x12, 0xA4, 0x42];
/// At most this many datagram numbers are looked up per ACK datagram (a
/// range can claim up to 2^24; a real one covers a few dozen).
const MAX_ACKED_PER_DATAGRAM: u32 = 256;

static ACTIVE: AtomicBool = AtomicBool::new(false);
static SESSION: Mutex<Option<Session>> = Mutex::new(None);
static FREQUENCY: AtomicI64 = AtomicI64::new(0);

/// What the observer has seen since it was switched on (Details, diagnosis).
#[derive(Clone, Copy)]
enum Count {
    Sends,
    TargetSends,
    DataSent,
    PingsSent,
    Receives,
    TargetReceives,
    DataReceived,
    AcksReceived,
    OtherReceived,
    AckSamples,
    Pongs,
    StunRequests,
    StunMatched,
}
static COUNTS: [AtomicU64; 13] = [const { AtomicU64::new(0) }; 13];
fn count(which: Count) {
    COUNTS[which as usize].fetch_add(1, Ordering::Relaxed);
}
/// The last peer something was received from that was not the target, and
/// the first bytes of the last datagram from the target (diagnosis).
static LAST_OTHER_PEER: Mutex<Option<Peer>> = Mutex::new(None);
static LAST_TARGET_HEAD: AtomicU64 = AtomicU64::new(0);
/// Serializes tests that switch the observer or the WinSock hooks.
#[cfg(test)]
pub(crate) static TEST_LOCK: Mutex<()> = Mutex::new(());

/// Where a reading came from.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Source {
    /// The game's connected ping and the server's pong.
    Ping,
    /// Datagram acknowledgements (median of the recent ones).
    Ack,
    /// NetherNet: ICE connectivity checks (median of the recent ones).
    Stun,
}

#[derive(Clone, Copy)]
struct Sent {
    seq: u32,
    at: i64,
    live: bool,
}

struct Session {
    target: Peer,
    /// Counter source the stored times come from (see `timer::profile_counter`).
    source: usize,
    frequency: f64,
    datagrams: [Sent; DATAGRAMS],
    pings: [Option<(u64, i64)>; PINGS],
    next_ping: usize,
    /// Latest pong-based round trip (ms) and when it was taken.
    ping_rtt: Option<(f64, i64)>,
    /// Recent per-ACK round trips (ms) and when they were taken.
    acks: VecDeque<(f64, i64)>,
    /// STUN Binding requests sent: transaction id and time.
    stuns: [Option<([u8; 12], i64)>; STUNS],
    next_stun: usize,
    /// Recent STUN round trips (ms) and when they were taken.
    stun_rtts: VecDeque<(f64, i64)>,
}

impl Session {
    fn new(target: Peer, source: usize, frequency: f64) -> Self {
        Self {
            target,
            source,
            frequency,
            datagrams: [Sent {
                seq: 0,
                at: 0,
                live: false,
            }; DATAGRAMS],
            pings: [None; PINGS],
            next_ping: 0,
            ping_rtt: None,
            acks: VecDeque::with_capacity(ACK_SAMPLES),
            stuns: [None; STUNS],
            next_stun: 0,
            stun_rtts: VecDeque::with_capacity(STUN_SAMPLES),
        }
    }

    fn ms(&self, from: i64, to: i64) -> f64 {
        (to - from) as f64 * 1000. / self.frequency
    }

    /// Times from another counter source cannot be compared: start over.
    fn same_clock(&mut self, source: usize) -> bool {
        if self.source != source {
            *self = Session::new(self.target, source, self.frequency);
            return false;
        }
        true
    }

    fn sent(&mut self, bytes: &[u8], now: (i64, usize)) {
        self.same_clock(now.1);
        if let Some((kind, id)) = stun_in(bytes) {
            if kind == BINDING_REQUEST {
                count(Count::StunRequests);
                self.stuns[self.next_stun] = Some((id, now.0));
                self.next_stun = (self.next_stun + 1) % STUNS;
            }
            return;
        }
        let Datagram::Data { seq, frames } = classify(bytes) else {
            return;
        };
        count(Count::DataSent);
        self.datagrams[seq as usize % DATAGRAMS] = Sent {
            seq,
            at: now.0,
            live: true,
        };
        for_each_frame(frames, |payload| {
            if let Some(time) = connected_ping(payload) {
                count(Count::PingsSent);
                self.pings[self.next_ping] = Some((time, now.0));
                self.next_ping = (self.next_ping + 1) % PINGS;
            }
        });
    }

    fn received(&mut self, bytes: &[u8], now: (i64, usize)) {
        if !self.same_clock(now.1) {
            return;
        }
        if let Some((kind, id)) = stun_in(bytes) {
            if kind == BINDING_SUCCESS || kind == BINDING_ERROR {
                let frequency = self.frequency;
                if let Some(slot) = self
                    .stuns
                    .iter_mut()
                    .find(|s| s.is_some_and(|(sent, _)| sent == id))
                {
                    let (_, at) = slot.take().unwrap();
                    if at <= now.0 {
                        count(Count::StunMatched);
                        if self.stun_rtts.len() == STUN_SAMPLES {
                            self.stun_rtts.pop_front();
                        }
                        let rtt = (now.0 - at) as f64 * 1000. / frequency;
                        self.stun_rtts.push_back((rtt, now.0));
                    }
                }
            }
            return;
        }
        match classify(bytes) {
            Datagram::Ack(records) => {
                count(Count::AcksReceived);
                let mut best: Option<f64> = None;
                let mut budget = MAX_ACKED_PER_DATAGRAM;
                for_each_ack_range(records, |start, end| {
                    let mut seq = start;
                    while budget > 0 {
                        budget -= 1;
                        let slot = &mut self.datagrams[seq as usize % DATAGRAMS];
                        if slot.live && slot.seq == seq && slot.at <= now.0 {
                            slot.live = false;
                            let rtt = (now.0 - slot.at) as f64 * 1000. / self.frequency;
                            best = Some(best.map_or(rtt, |b: f64| b.min(rtt)));
                        }
                        if seq == end {
                            break;
                        }
                        seq = (seq + 1) & 0xFF_FFFF;
                    }
                });
                if let Some(rtt) = best {
                    count(Count::AckSamples);
                    if self.acks.len() == ACK_SAMPLES {
                        self.acks.pop_front();
                    }
                    self.acks.push_back((rtt, now.0));
                }
            }
            Datagram::Data { frames, .. } => {
                count(Count::DataReceived);
                for_each_frame(frames, |payload| {
                    if let Some(time) = connected_pong(payload) {
                        let frequency = self.frequency;
                        if let Some(slot) = self
                            .pings
                            .iter_mut()
                            .find(|p| p.is_some_and(|(sent, _)| sent == time))
                        {
                            let (_, at) = slot.take().unwrap();
                            if at <= now.0 {
                                let rtt = (now.0 - at) as f64 * 1000. / frequency;
                                self.ping_rtt = Some((rtt, now.0));
                                count(Count::Pongs);
                            }
                        }
                    }
                });
            }
            Datagram::Other => count(Count::OtherReceived),
        }
    }

    fn reading(&self, now: i64) -> Option<(f64, Source)> {
        if let Some((rtt, at)) = self.ping_rtt {
            if self.ms(at, now) <= PING_FRESH_MS {
                return Some((rtt, Source::Ping));
            }
        }
        if let Some(rtt) = self.median(&self.stun_rtts, now, STUN_FRESH_MS, 1) {
            return Some((rtt, Source::Stun));
        }
        self.median(&self.acks, now, ACK_FRESH_MS, MIN_ACK_SAMPLES)
            .map(|rtt| (rtt, Source::Ack))
    }

    /// The median of the samples taken within `fresh` ms, if at least `min`.
    fn median(
        &self,
        samples: &VecDeque<(f64, i64)>,
        now: i64,
        fresh: f64,
        min: usize,
    ) -> Option<f64> {
        let mut recent = [0f64; ACK_SAMPLES];
        let mut n = 0;
        for &(rtt, at) in samples.iter().take(ACK_SAMPLES) {
            if self.ms(at, now) <= fresh {
                recent[n] = rtt;
                n += 1;
            }
        }
        if n < min.max(1) {
            return None;
        }
        let recent = &mut recent[..n];
        recent.sort_by(f64::total_cmp);
        Some(recent[n / 2])
    }
}

#[derive(Debug, PartialEq)]
enum Datagram<'a> {
    /// A numbered data datagram and its frames.
    Data { seq: u32, frames: &'a [u8] },
    /// An acknowledgement and its records.
    Ack(&'a [u8]),
    /// Offline messages, NAKs, other protocols.
    Other,
}

/// STUN message types (method Binding, by class).
const BINDING_REQUEST: u16 = 0x0001;
const BINDING_SUCCESS: u16 = 0x0101;
const BINDING_ERROR: u16 = 0x0111;
/// TURN Send and Data indications, and their DATA attribute.
const SEND_INDICATION: u16 = 0x0016;
const DATA_INDICATION: u16 = 0x0017;
const DATA_ATTRIBUTE: u16 = 0x0013;

/// A STUN message's type and transaction id. Shares a port with DTLS and
/// SRTP but is told apart by its first two bits and its magic cookie.
fn stun_message(bytes: &[u8]) -> Option<(u16, [u8; 12])> {
    if bytes.len() < 20 || bytes[0] & 0xC0 != 0 || bytes[4..8] != MAGIC_COOKIE {
        return None;
    }
    let length = u16::from_be_bytes([bytes[2], bytes[3]]) as usize;
    if !length.is_multiple_of(4) || 20 + length > bytes.len() {
        return None;
    }
    Some((
        u16::from_be_bytes([bytes[0], bytes[1]]),
        bytes[8..20].try_into().ok()?,
    ))
}

/// The value of a STUN message's first attribute of type `wanted`.
fn attribute(message: &[u8], wanted: u16) -> Option<&[u8]> {
    let length = u16::from_be_bytes([message[2], message[3]]) as usize;
    let mut rest = message.get(20..20 + length)?;
    while rest.len() >= 4 {
        let kind = u16::from_be_bytes([rest[0], rest[1]]);
        let len = u16::from_be_bytes([rest[2], rest[3]]) as usize;
        let value = rest.get(4..4 + len)?;
        if kind == wanted {
            return Some(value);
        }
        rest = rest.get(4 + len.div_ceil(4) * 4..).unwrap_or(&[]);
    }
    None
}

/// The STUN message in a datagram, unwrapping TURN relaying once:
/// ChannelData (channel 0x4000-0x7FFF, length, data) or a Send/Data
/// indication's DATA attribute.
fn stun_in(bytes: &[u8]) -> Option<(u16, [u8; 12])> {
    if let [first, _, a, b, rest @ ..] = bytes {
        if (0x40..=0x7F).contains(first) {
            let length = u16::from_be_bytes([*a, *b]) as usize;
            return rest.get(..length).and_then(stun_message);
        }
    }
    let (kind, id) = stun_message(bytes)?;
    if kind == SEND_INDICATION || kind == DATA_INDICATION {
        return attribute(bytes, DATA_ATTRIBUTE).and_then(stun_message);
    }
    Some((kind, id))
}

fn u24(bytes: &[u8]) -> u32 {
    u32::from_le_bytes([bytes[0], bytes[1], bytes[2], 0])
}

fn classify(bytes: &[u8]) -> Datagram<'_> {
    let Some(&flags) = bytes.first() else {
        return Datagram::Other;
    };
    if flags & 0x80 == 0 {
        // Offline (unconnected ping, connection requests) or not RakNet.
        return Datagram::Other;
    }
    if flags & 0x40 != 0 {
        return if bytes.len() >= 3 {
            Datagram::Ack(&bytes[1..])
        } else {
            Datagram::Other
        };
    }
    if flags & 0x20 != 0 || bytes.len() < 4 {
        return Datagram::Other; // NAK
    }
    Datagram::Data {
        seq: u24(&bytes[1..4]),
        frames: &bytes[4..],
    }
}

/// Calls `each` with every complete (non-split) frame's payload, stopping at
/// the first malformed frame.
fn for_each_frame(mut body: &[u8], mut each: impl FnMut(&[u8])) {
    while body.len() >= 3 {
        let flags = body[0];
        let reliability = flags >> 5;
        let split = flags & 0x10 != 0;
        let length = (u16::from_be_bytes([body[1], body[2]]) as usize).div_ceil(8);
        let mut header = 3;
        if matches!(reliability, 2 | 3 | 4 | 6 | 7) {
            header += 3; // reliable message index
        }
        if matches!(reliability, 1 | 4) {
            header += 3; // sequence index
        }
        if matches!(reliability, 1 | 3 | 4 | 7) {
            header += 4; // order index and channel
        }
        if split {
            header += 10; // split count, id and index
        }
        let Some(payload) = body.get(header..header + length) else {
            return;
        };
        if !split {
            each(payload);
        }
        body = &body[header + length..];
    }
}

/// Calls `each` with every (first, last) datagram number an ACK covers.
fn for_each_ack_range(records: &[u8], mut each: impl FnMut(u32, u32)) {
    if records.len() < 2 {
        return;
    }
    let count = u16::from_be_bytes([records[0], records[1]]);
    let mut rest = &records[2..];
    for _ in 0..count {
        match rest {
            [1, a, b, c, tail @ ..] => {
                let seq = u24(&[*a, *b, *c]);
                each(seq, seq);
                rest = tail;
            }
            [0, a, b, c, d, e, f, tail @ ..] => {
                each(u24(&[*a, *b, *c]), u24(&[*d, *e, *f]));
                rest = tail;
            }
            _ => return,
        }
    }
}

/// The time in a connected ping (`0x00` + u64).
fn connected_ping(payload: &[u8]) -> Option<u64> {
    match payload {
        [0x00, time @ ..] if time.len() == 8 => Some(u64::from_be_bytes(time.try_into().ok()?)),
        _ => None,
    }
}

/// The echoed ping time in a connected pong (`0x03` + ping u64 + pong u64).
fn connected_pong(payload: &[u8]) -> Option<u64> {
    match payload {
        [0x03, rest @ ..] if rest.len() == 16 => {
            Some(u64::from_be_bytes(rest[..8].try_into().ok()?))
        }
        _ => None,
    }
}

fn peer_of(addr: SocketAddr) -> Peer {
    let mut bytes = [0; 16];
    match addr {
        SocketAddr::V4(v4) => {
            bytes[..4].copy_from_slice(&v4.ip().octets());
            (AF_INET, bytes, v4.port())
        }
        SocketAddr::V6(v6) => match v6.ip().to_ipv4_mapped() {
            Some(v4) => {
                bytes[..4].copy_from_slice(&v4.octets());
                (AF_INET, bytes, v6.port())
            }
            None => (AF_INET6, v6.ip().octets(), v6.port()),
        },
    }
}

fn now() -> Option<(i64, usize)> {
    super::super::timer::profile_counter()
}

fn frequency() -> f64 {
    let mut value = FREQUENCY.load(Ordering::Relaxed);
    if value <= 0 {
        unsafe { windows_sys::Win32::System::Performance::QueryPerformanceFrequency(&mut value) };
        FREQUENCY.store(value, Ordering::Relaxed);
    }
    value.max(1) as f64
}

/// Whether the WinSock hooks should report traffic.
pub(crate) fn active() -> bool {
    ACTIVE.load(Ordering::Relaxed)
}

/// Starts or stops observing (and the receive hook with it).
pub(crate) fn set_enabled(on: bool) {
    if on {
        crate::net::watch_receive(true);
    }
    if on && !ACTIVE.load(Ordering::Relaxed) {
        for c in &COUNTS {
            c.store(0, Ordering::Relaxed);
        }
        LAST_TARGET_HEAD.store(0, Ordering::Relaxed);
        *LAST_OTHER_PEER.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }
    ACTIVE.store(on, Ordering::Relaxed);
    if !on {
        crate::net::watch_receive(false);
        *SESSION.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }
}

/// The server to observe; a different one starts over.
pub(crate) fn set_target(target: Option<SocketAddr>) {
    let target = target.map(peer_of);
    let mut session = SESSION.lock().unwrap_or_else(|e| e.into_inner());
    if session.as_ref().map(|s| s.target) == target {
        return;
    }
    *session = match (target, now()) {
        (Some(target), Some((_, source))) => Some(Session::new(target, source, frequency())),
        _ => None,
    };
}

/// The current round trip (ms) and where it came from.
pub(crate) fn reading() -> Option<(f64, Source)> {
    let now = now()?;
    let session = SESSION.lock().unwrap_or_else(|e| e.into_inner());
    let session = session.as_ref()?;
    (session.source == now.1)
        .then(|| session.reading(now.0))
        .flatten()
}

/// A datagram the game sent to `peer`. Called on the game's network thread:
/// never blocks (a busy session drops the sample).
pub(crate) fn on_send(peer: Peer, bytes: &[u8]) {
    count(Count::Sends);
    if let (Ok(mut session), Some(now)) = (SESSION.try_lock(), now()) {
        if let Some(session) = session.as_mut().filter(|s| s.target == peer) {
            count(Count::TargetSends);
            session.sent(bytes, now);
        }
    }
}

/// A datagram the game received from `peer`; see `on_send`.
pub(crate) fn on_receive(peer: Peer, bytes: &[u8]) {
    count(Count::Receives);
    if let (Ok(mut session), Some(now)) = (SESSION.try_lock(), now()) {
        match session.as_mut() {
            Some(session) if session.target == peer => {
                count(Count::TargetReceives);
                let mut head = [0u8; 8];
                let n = bytes.len().min(8);
                head[..n].copy_from_slice(&bytes[..n]);
                LAST_TARGET_HEAD.store(u64::from_be_bytes(head), Ordering::Relaxed);
                session.received(bytes, now);
            }
            Some(_) => {
                if let Ok(mut last) = LAST_OTHER_PEER.try_lock() {
                    *last = Some(peer);
                }
            }
            None => {}
        }
    }
}

fn format_peer((family, addr, port): Peer) -> String {
    if family == AF_INET {
        format!("{}.{}.{}.{}:{port}", addr[0], addr[1], addr[2], addr[3])
    } else {
        format!("[{}]:{port}", std::net::Ipv6Addr::from(addr))
    }
}

/// Lines describing what the observer has seen, for the Details panel.
pub(crate) fn diagnostic() -> Vec<String> {
    let c = |which: Count| COUNTS[which as usize].load(Ordering::Relaxed);
    let mut lines = vec![
        format!(
            "受信の観測: {} / 受信 {}（接続先から {}: データ {}・ACK {}・その他 {}）",
            if crate::net::receive_hooked() {
                "有効"
            } else {
                "無効"
            },
            c(Count::Receives),
            c(Count::TargetReceives),
            c(Count::DataReceived),
            c(Count::AcksReceived),
            c(Count::OtherReceived),
        ),
        format!(
            "送信 {}（接続先へ {}: データ {}・接続中Ping {}）/ 一致: ACK {}・Pong {}",
            c(Count::Sends),
            c(Count::TargetSends),
            c(Count::DataSent),
            c(Count::PingsSent),
            c(Count::AckSamples),
            c(Count::Pongs),
        ),
        format!(
            "NetherNet（STUN）: 確認要求 {}・応答一致 {}",
            c(Count::StunRequests),
            c(Count::StunMatched),
        ),
    ];
    if c(Count::TargetReceives) > 0 {
        let head = LAST_TARGET_HEAD.load(Ordering::Relaxed);
        lines.push(format!("接続先から最後に受信した先頭: {head:016X}"));
    }
    if let Some(peer) = *LAST_OTHER_PEER.lock().unwrap_or_else(|e| e.into_inner()) {
        lines.push(format!("接続先以外からの受信: {}", format_peer(peer)));
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    const PEER: Peer = (
        AF_INET,
        [10, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
        19132,
    );
    /// 1 tick = 1 µs, so `ms(1000)` is 1 ms of ticks.
    const FREQ: f64 = 1_000_000.;
    fn ms(value: i64) -> i64 {
        value * 1000
    }

    /// A data datagram with `seq` holding the frames given as
    /// (reliability, payload).
    fn data(seq: u32, frames: &[(u8, &[u8])]) -> Vec<u8> {
        let mut out = vec![0x84];
        out.extend(&seq.to_le_bytes()[..3]);
        for &(reliability, payload) in frames {
            out.push(reliability << 5);
            out.extend(((payload.len() * 8) as u16).to_be_bytes());
            if matches!(reliability, 2 | 3 | 4 | 6 | 7) {
                out.extend([7, 0, 0]);
            }
            if matches!(reliability, 1 | 4) {
                out.extend([8, 0, 0]);
            }
            if matches!(reliability, 1 | 3 | 4 | 7) {
                out.extend([9, 0, 0, 0]);
            }
            out.extend(payload);
        }
        out
    }

    /// An ACK covering the given (first, last) ranges.
    fn ack(ranges: &[(u32, u32)]) -> Vec<u8> {
        let mut out = vec![0xC0];
        out.extend((ranges.len() as u16).to_be_bytes());
        for &(first, last) in ranges {
            if first == last {
                out.push(1);
                out.extend(&first.to_le_bytes()[..3]);
            } else {
                out.push(0);
                out.extend(&first.to_le_bytes()[..3]);
                out.extend(&last.to_le_bytes()[..3]);
            }
        }
        out
    }

    fn ping(time: u64) -> Vec<u8> {
        let mut out = vec![0x00];
        out.extend(time.to_be_bytes());
        out
    }

    fn pong(time: u64) -> Vec<u8> {
        let mut out = vec![0x03];
        out.extend(time.to_be_bytes());
        out.extend(999u64.to_be_bytes());
        out
    }

    #[test]
    fn frames_of_every_reliability_are_parsed_and_split_ones_skipped() {
        let payloads: [&[u8]; 3] = [b"abc", &[0xFE, 1, 2, 3, 4], b"z"];
        for reliability in 0..8 {
            let bytes = data(5, &payloads.map(|p| (reliability, p)));
            let Datagram::Data { seq, frames } = classify(&bytes) else {
                panic!("{bytes:?}");
            };
            assert_eq!(seq, 5);
            let mut seen = Vec::new();
            for_each_frame(frames, |p| seen.push(p.to_vec()));
            assert_eq!(
                seen,
                payloads.map(|p| p.to_vec()),
                "reliability {reliability}"
            );
        }
        // A split frame is skipped; a truncated one ends the walk safely.
        let mut split = vec![0x10, 0, 8, 0, 0, 0, 2, 0, 1, 0, 0, 0, 0, b'x'];
        split.extend([0, 0, 8, b'y']);
        let mut seen = Vec::new();
        for_each_frame(&split, |p| seen.push(p.to_vec()));
        assert_eq!(seen, vec![b"y".to_vec()]);
        for_each_frame(&[0x40, 0, 80, 1, 2], |_| panic!("truncated"));
        // Offline messages, NAKs and empty input are not data.
        assert_eq!(classify(&[0x01, 0, 0, 0]), Datagram::Other);
        assert_eq!(classify(&[0xA0, 0, 1, 1, 0, 0, 0]), Datagram::Other);
        assert_eq!(classify(&[]), Datagram::Other);
    }

    #[test]
    fn ack_ranges_parse_and_stop_at_malformed_records() {
        let mut seen = Vec::new();
        let bytes = ack(&[(3, 3), (10, 12)]);
        let Datagram::Ack(records) = classify(&bytes) else {
            panic!()
        };
        for_each_ack_range(records, |a, b| seen.push((a, b)));
        assert_eq!(seen, vec![(3, 3), (10, 12)]);
        seen.clear();
        for_each_ack_range(&[0, 2, 1, 5, 0], |a, b| seen.push((a, b)));
        assert!(seen.is_empty());
    }

    #[test]
    fn pongs_give_exact_round_trips_and_acks_the_smallest_per_ack() {
        let mut s = Session::new(PEER, 1, FREQ);
        // The game sends datagrams 0..=4 every 10 ms; 2 carries its ping.
        for seq in 0..5u32 {
            let frames: Vec<(u8, Vec<u8>)> = if seq == 2 {
                vec![(0, ping(4242))]
            } else {
                vec![(3, vec![0xFE, 1, 2])]
            };
            let refs: Vec<(u8, &[u8])> = frames.iter().map(|(r, p)| (*r, p.as_slice())).collect();
            s.sent(&data(seq, &refs), (ms(seq as i64 * 10), 1));
        }
        // An ACK of 0..=4 at 75 ms: the newest (sent at 40 ms) waited 35 ms.
        s.received(&ack(&[(0, 4)]), (ms(75), 1));
        assert_eq!(s.acks.len(), 1);
        assert!((s.acks[0].0 - 35.).abs() < 1e-9);
        // Acknowledged again (a duplicate ACK): no new sample.
        s.received(&ack(&[(0, 4)]), (ms(80), 1));
        assert_eq!(s.acks.len(), 1);
        // Not enough ACK samples yet, no pong: no reading.
        assert_eq!(s.reading(ms(80)), None);
        // The pong for the ping sent at 20 ms arrives at 52 ms.
        s.received(&data(0, &[(0, &pong(4242))]), (ms(52), 1));
        assert_eq!(s.reading(ms(100)), Some((32., Source::Ping)));
        // An unknown pong changes nothing.
        s.received(&data(1, &[(0, &pong(1))]), (ms(60), 1));
        assert_eq!(s.reading(ms(100)), Some((32., Source::Ping)));
        // Ten seconds on, ACKs take over (median of the recent ones).
        let base = ms(20_000);
        for (i, rtt) in [30, 50, 40].into_iter().enumerate() {
            let seq = 100 + i as u32;
            s.sent(&data(seq, &[(0, b"x")]), (base + ms(i as i64 * 100), 1));
            s.received(&ack(&[(seq, seq)]), (base + ms(i as i64 * 100 + rtt), 1));
        }
        assert_eq!(s.reading(base + ms(400)), Some((40., Source::Ack)));
        // Stale ACKs give no reading.
        assert_eq!(s.reading(base + ms(10_000)), None);
    }

    #[test]
    fn number_wraparound_huge_ranges_and_clock_changes_are_safe() {
        let mut s = Session::new(PEER, 1, FREQ);
        s.sent(&data(0xFF_FFFF, &[(0, b"x")]), (ms(0), 1));
        s.sent(&data(0, &[(0, b"x")]), (ms(5), 1));
        s.received(&ack(&[(0xFF_FFFF, 0)]), (ms(25), 1));
        assert!((s.acks[0].0 - 20.).abs() < 1e-9);
        // A range claiming the whole number space looks at a bounded count.
        s.received(&ack(&[(1, 0xFF_FFFF)]), (ms(30), 1));
        // Times from another counter source are never compared.
        s.sent(&data(7, &[(0, b"x")]), (ms(40), 1));
        s.received(&ack(&[(7, 7)]), (ms(50), 2));
        assert!(s.acks.is_empty() && s.source == 2);
        // Receive-before-send times (a clock oddity) are ignored.
        s.sent(&data(8, &[(0, b"x")]), (ms(100), 2));
        s.received(&ack(&[(8, 8)]), (ms(90), 2));
        assert!(s.acks.is_empty());
    }

    /// A STUN message of `kind` with transaction id `id` and attributes.
    fn stun(kind: u16, id: u8, attributes: &[(u16, &[u8])]) -> Vec<u8> {
        let mut body = Vec::new();
        for &(t, value) in attributes {
            body.extend(t.to_be_bytes());
            body.extend((value.len() as u16).to_be_bytes());
            body.extend(value);
            body.resize(body.len().div_ceil(4) * 4, 0);
        }
        let mut out = kind.to_be_bytes().to_vec();
        out.extend((body.len() as u16).to_be_bytes());
        out.extend(MAGIC_COOKIE);
        out.extend([id; 12]);
        out.extend(body);
        out
    }

    #[test]
    fn nethernet_stun_checks_give_round_trips_direct_and_through_turn() {
        let mut s = Session::new(PEER, 1, FREQ);
        // Direct: a Binding request at 0 ms, its success response at 42 ms.
        s.sent(
            &stun(BINDING_REQUEST, 1, &[(0x0006, b"user:name")]),
            (ms(0), 1),
        );
        s.received(&stun(BINDING_SUCCESS, 1, &[(0x0020, &[0; 8])]), (ms(42), 1));
        assert_eq!(s.reading(ms(50)), Some((42., Source::Stun)));
        // Through TURN, wrapped in ChannelData out and back: 60 ms.
        let channel = |inner: Vec<u8>| {
            let mut out = vec![0x40, 0x01];
            out.extend((inner.len() as u16).to_be_bytes());
            out.extend(inner);
            out
        };
        s.sent(&channel(stun(BINDING_REQUEST, 2, &[])), (ms(1000), 1));
        s.received(&channel(stun(BINDING_SUCCESS, 2, &[])), (ms(1060), 1));
        // In a Send indication out, a Data indication back: 50 ms.
        let request = stun(BINDING_REQUEST, 3, &[]);
        s.sent(
            &stun(
                SEND_INDICATION,
                9,
                &[(0x0012, &[0; 8]), (DATA_ATTRIBUTE, &request)],
            ),
            (ms(2000), 1),
        );
        let response = stun(BINDING_ERROR, 3, &[]);
        s.received(
            &stun(DATA_INDICATION, 9, &[(DATA_ATTRIBUTE, &response)]),
            (ms(2050), 1),
        );
        // Median of 42, 60, 50.
        assert_eq!(s.reading(ms(2100)), Some((50., Source::Stun)));
        // TURN's own requests, unknown ids, DTLS records and stale samples
        // give nothing.
        s.sent(&stun(0x0004, 4, &[]), (ms(3000), 1));
        s.received(&stun(0x0104, 4, &[]), (ms(3010), 1));
        s.received(&stun(BINDING_SUCCESS, 7, &[]), (ms(3020), 1));
        s.received(
            &[
                0x17, 0xFE, 0xFD, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            ],
            (ms(3030), 1),
        );
        assert_eq!(s.stun_rtts.len(), 3);
        assert_eq!(s.reading(ms(30_000)), None);
        // A pong-based reading still comes first while fresh.
        s.sent(&data(1, &[(0, &ping(77))]), (ms(30_000), 1));
        s.received(&data(1, &[(0, &pong(77))]), (ms(30_020), 1));
        assert_eq!(s.reading(ms(30_100)), Some((20., Source::Ping)));
    }

    #[test]
    fn stun_parsing_rejects_lookalikes() {
        let good = stun(BINDING_REQUEST, 5, &[]);
        assert_eq!(stun_in(&good), Some((BINDING_REQUEST, [5; 12])));
        let mut no_cookie = good.clone();
        no_cookie[4] ^= 1;
        assert_eq!(stun_in(&no_cookie), None);
        let mut long = good.clone();
        long[3] = 4; // claims an attribute that is not there
        assert_eq!(stun_in(&long), None);
        assert_eq!(stun_in(&good[..19]), None);
        // RakNet datagrams are never STUN (first bit set) and ChannelData
        // with a short body is rejected.
        assert_eq!(stun_in(&[0x84, 0, 0, 0]), None);
        assert_eq!(stun_in(&[0x40, 0, 0, 30, 1, 2]), None);
    }

    #[test]
    fn mapped_ipv6_targets_match_the_hooks_ipv4_peers() {
        let v4: SocketAddr = "10.0.0.1:19132".parse().unwrap();
        let mapped: SocketAddr = "[::ffff:10.0.0.1]:19132".parse().unwrap();
        assert_eq!(peer_of(v4), PEER);
        assert_eq!(peer_of(mapped), PEER);
        let v6: SocketAddr = "[2001:db8::1]:19133".parse().unwrap();
        assert_eq!(peer_of(v6).0, AF_INET6);
    }
}
