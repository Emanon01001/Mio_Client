//! Bedrock server-list RTT, using a separate connected UDP socket. This does
//! not read/modify the game's socket or report the connected session's RTT.
use std::io;
use std::net::{SocketAddr, UdpSocket};
use std::time::Duration;

use windows_sys::Win32::System::Performance::QueryPerformanceFrequency;
use windows_sys::Win32::System::SystemInformation::GetTickCount64;

const MAGIC: [u8; 16] = [
    0x00, 0xff, 0xff, 0x00, 0xfe, 0xfe, 0xfe, 0xfe, 0xfd, 0xfd, 0xfd, 0xfd, 0x12, 0x34, 0x56, 0x78,
];
const TIMEOUT_MS: u64 = 1500;

#[derive(Clone, Debug, PartialEq)]
pub(super) enum Reading {
    Rtt(f64),
    Timeout,
    Error(String),
    Cancelled,
}

fn packet(token: u64, guid: u64) -> [u8; 33] {
    let mut out = [0; 33];
    out[0] = 1; // ID_UNCONNECTED_PING, replies even if a server is full.
    out[1..9].copy_from_slice(&token.to_be_bytes());
    out[9..25].copy_from_slice(&MAGIC);
    out[25..33].copy_from_slice(&guid.to_be_bytes());
    out
}

fn pong(bytes: &[u8], token: u64) -> bool {
    if bytes.len() < 35
        || bytes[0] != 0x1c
        || bytes[1..9] != token.to_be_bytes()
        || bytes[17..33] != MAGIC
    {
        return false;
    }
    let len = u16::from_be_bytes([bytes[33], bytes[34]]) as usize;
    len == bytes.len() - 35 && bytes[35..].starts_with(b"MCPE;")
}

fn elapsed_ms(start: (i64, usize), end: (i64, usize), frequency: i64) -> Option<f64> {
    if start.1 != end.1 || frequency <= 0 || end.0 < start.0 {
        return None;
    }
    Some(end.0.checked_sub(start.0)? as f64 * 1000. / frequency as f64)
}

pub(super) fn measure(peer: SocketAddr, guid: u64, cancelled: impl Fn() -> bool) -> Reading {
    match run(peer, guid, cancelled) {
        Ok(reading) => reading,
        Err(e) => Reading::Error(e.to_string()),
    }
}

fn run(peer: SocketAddr, guid: u64, cancelled: impl Fn() -> bool) -> io::Result<Reading> {
    let socket = UdpSocket::bind(if peer.is_ipv4() {
        "0.0.0.0:0"
    } else {
        "[::]:0"
    })?;
    // Connected UDP restricts received datagrams to this exact IP/port. send()
    // also avoids the sendto hooks used by net::current(): probes must never
    // keep a disconnected game peer alive in the server detector.
    socket.connect(peer)?;
    socket.set_read_timeout(Some(Duration::from_millis(50)))?;
    socket.set_write_timeout(Some(Duration::from_millis(50)))?;
    let mut frequency = 0;
    unsafe { QueryPerformanceFrequency(&mut frequency) };
    let start = super::super::timer::profile_counter()
        .ok_or_else(|| io::Error::other("高精度タイマーを取得できません"))?;
    let token = start.0 as u64;
    let bytes = packet(token, guid);
    if cancelled() {
        return Ok(Reading::Cancelled);
    }
    let sent_at = unsafe { GetTickCount64() };
    if socket.send(&bytes)? != bytes.len() {
        return Err(io::Error::other("Pingの送信が完了しませんでした"));
    }
    let mut buffer = [0; 2048];
    let mut rejected = 0;
    loop {
        if cancelled() {
            return Ok(Reading::Cancelled);
        }
        if unsafe { GetTickCount64() }.saturating_sub(sent_at) >= TIMEOUT_MS {
            return Ok(Reading::Timeout);
        }
        match socket.recv(&mut buffer) {
            Ok(n) => {
                // Capture before validating: parsing time is not network RTT.
                let end = super::super::timer::profile_counter();
                if pong(&buffer[..n], token) {
                    return Ok(
                        match end.and_then(|end| elapsed_ms(start, end, frequency)) {
                            Some(ms) => Reading::Rtt(ms),
                            None => {
                                Reading::Error("Timerの切り替え中です。次回再測定します".into())
                            }
                        },
                    );
                }
                rejected += 1;
                if rejected >= 32 {
                    return Ok(Reading::Error("応答形式が非対応です".into()));
                }
            }
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::WouldBlock
                        | io::ErrorKind::TimedOut
                        | io::ErrorKind::Interrupted
                ) => {}
            Err(e) => return Err(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn response(token: u64) -> Vec<u8> {
        let text = b"MCPE;Test;999;test;0;10";
        let mut out = vec![0x1c];
        out.extend(token.to_be_bytes());
        out.extend(123u64.to_be_bytes());
        out.extend(MAGIC);
        out.extend((text.len() as u16).to_be_bytes());
        out.extend(text);
        out
    }

    #[test]
    fn validates_packet_echo_magic_lengths_and_bedrock_identifier() {
        let request = packet(123, 456);
        assert_eq!(request[0], 1);
        assert_eq!(&request[1..9], &123u64.to_be_bytes());
        assert_eq!(&request[9..25], &MAGIC);
        assert_eq!(&request[25..], &456u64.to_be_bytes());
        let good = response(123);
        assert!(pong(&good, 123));
        assert!(!pong(&good, 124));
        for len in 0..good.len() {
            assert!(!pong(&good[..len], 123), "truncated length {len}");
        }
        for index in [0, 1, 17, 33, 35] {
            let mut bad = good.clone();
            bad[index] ^= 1;
            assert!(!pong(&bad, 123), "corruption at {index}");
        }
        let mut extra = good;
        extra.push(0);
        assert!(!pong(&extra, 123));
        assert_eq!(elapsed_ms((100, 7), (125, 7), 1000), Some(25.));
        assert_eq!(elapsed_ms((100, 7), (125, 8), 1000), None);
        assert_eq!(elapsed_ms((125, 7), (100, 7), 1000), None);
        assert_eq!(elapsed_ms((100, 7), (125, 7), 0), None);
    }

    #[test]
    fn loopback_probe_rejects_wrong_echo_then_measures_valid_pong() {
        let server = UdpSocket::bind("127.0.0.1:0").unwrap();
        server
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        let peer = server.local_addr().unwrap();
        let thread = std::thread::spawn(move || {
            let mut request = [0; 128];
            let (n, source) = server.recv_from(&mut request).unwrap();
            assert_eq!(n, 33);
            assert_eq!(&request[9..25], &MAGIC);
            let token = u64::from_be_bytes(request[1..9].try_into().unwrap());
            server
                .send_to(&response(token.wrapping_add(1)), source)
                .unwrap();
            std::thread::sleep(Duration::from_millis(25));
            server.send_to(&response(token), source).unwrap();
        });
        let reading = measure(peer, 456, || false);
        thread.join().unwrap();
        assert!(
            matches!(reading, Reading::Rtt(ms) if (15. ..1500.).contains(&ms)),
            "{reading:?}"
        );
    }

    #[test]
    fn ipv6_probe_ignores_a_pong_from_the_wrong_source_port() {
        let server = UdpSocket::bind("[::1]:0").unwrap();
        server
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        let peer = server.local_addr().unwrap();
        let thread = std::thread::spawn(move || {
            let mut request = [0; 128];
            let (_, source) = server.recv_from(&mut request).unwrap();
            let token = u64::from_be_bytes(request[1..9].try_into().unwrap());
            let other = UdpSocket::bind("[::1]:0").unwrap();
            other.send_to(&response(token), source).unwrap();
            std::thread::sleep(Duration::from_millis(25));
            server.send_to(&response(token), source).unwrap();
        });
        let reading = measure(peer, 1, || false);
        thread.join().unwrap();
        assert!(
            matches!(reading, Reading::Rtt(ms) if ms >= 15.),
            "{reading:?}"
        );
    }

    #[test]
    fn silent_peer_times_out_and_cancellation_returns_without_sending() {
        let server = UdpSocket::bind("127.0.0.1:0").unwrap();
        let peer = server.local_addr().unwrap();
        assert_eq!(measure(peer, 1, || true), Reading::Cancelled);
        assert_eq!(measure(peer, 1, || false), Reading::Timeout);
    }
}
