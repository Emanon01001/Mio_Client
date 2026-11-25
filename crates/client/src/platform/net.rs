//! Detects the address of the server the player is connected to.
//!
//! Minecraft Bedrock talks to servers over UDP (RakNet or NetherNet). Rather
//! than chase a game-internal offset, this hooks the two WinSock send functions
//! and watches where outbound packets go: the connected server is the address
//! receiving a steady stream. Nothing game-specific is read, so a game update
//! does not move it.
//!
//! Uses the MinHook build bundled with hudhook, like `modules::timer`.

use std::ffi::c_void;
use std::mem::transmute;
use std::net::{Ipv4Addr, Ipv6Addr};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, MutexGuard};

use hudhook::mh::{MH_CreateHook, MH_EnableHook, MH_Initialize, MH_STATUS};
use windows_sys::Win32::NetworkManagement::IpHelper::{
    GetAdaptersAddresses, GAA_FLAG_SKIP_ANYCAST, GAA_FLAG_SKIP_DNS_SERVER, GAA_FLAG_SKIP_MULTICAST,
    IP_ADAPTER_ADDRESSES_LH,
};
use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
use windows_sys::Win32::System::SystemInformation::GetTickCount64;

use crate::log;

const AF_INET: u16 = 2;
const AF_INET6: u16 = 23;

/// Packets to one address are counted in windows of this length (ms).
const RATE_WINDOW_MS: u64 = 1_000;
/// Packets per second before an address is treated as a server. A connected
/// peer is sent to every game tick (20/s); LAN discovery broadcasts and the
/// server list's pings go out about once a second.
const MIN_RATE: u32 = 5;
/// An address not sent to for this long (ms) is no longer current.
const STALE_MS: u64 = 3_000;
/// How many distinct destinations to remember.
const MAX_ENTRIES: usize = 24;
/// How long the list of local subnet broadcast addresses is reused (ms).
const BROADCAST_REFRESH_MS: u64 = 10_000;

// int recvfrom(SOCKET, char* buf, int len, int flags, sockaddr* from, int* fromlen)
type RecvFromFn = unsafe extern "system" fn(usize, *mut u8, i32, i32, *mut u8, *mut i32) -> i32;
// int sendto(SOCKET, const char* buf, int len, int flags, const sockaddr* to, int tolen)
type SendToFn = unsafe extern "system" fn(usize, *const u8, i32, i32, *const u8, i32) -> i32;
// int WSASendTo(SOCKET, LPWSABUF, DWORD, LPDWORD, DWORD, const sockaddr* to, int tolen,
//               LPWSAOVERLAPPED, LPWSAOVERLAPPED_COMPLETION_ROUTINE)
#[allow(clippy::too_many_arguments)]
type WsaSendToFn = unsafe extern "system" fn(
    usize,
    *const c_void,
    u32,
    *mut u32,
    u32,
    *const u8,
    i32,
    *mut c_void,
    *mut c_void,
) -> i32;

struct Entry {
    family: u16,
    addr: [u8; 16],
    port: u16,
    last_ms: u64,
    /// The window being counted, and the packets seen in it so far.
    window_start: u64,
    window_count: u32,
    /// Packets per second over the last completed window.
    rate: u32,
}

impl Entry {
    fn same_as(&self, family: u16, addr: &[u8; 16], port: u16) -> bool {
        self.family == family && self.port == port && self.addr == *addr
    }

    /// Counts one packet sent at `now`.
    fn hit(&mut self, now: u64) {
        let elapsed = now.saturating_sub(self.window_start);
        if elapsed >= RATE_WINDOW_MS {
            // After a pause the old count says nothing about the current rate.
            self.rate = if elapsed < RATE_WINDOW_MS * 2 {
                (self.window_count as u64 * 1000 / elapsed) as u32
            } else {
                0
            };
            self.window_start = now;
            self.window_count = 0;
        }
        self.window_count = self.window_count.saturating_add(1);
        self.last_ms = now;
    }

    fn current(&self, now: u64) -> bool {
        now.saturating_sub(self.last_ms) <= STALE_MS
    }

    fn is_broadcast(&self, broadcasts: &[[u8; 4]]) -> bool {
        self.family == AF_INET && broadcasts.iter().any(|b| b[..] == self.addr[..4])
    }

    fn format(&self) -> String {
        match self.family {
            AF_INET => {
                let ip = Ipv4Addr::new(self.addr[0], self.addr[1], self.addr[2], self.addr[3]);
                format!("{ip}:{}", self.port)
            }
            _ => {
                let ip = Ipv6Addr::from(self.addr);
                format!("[{ip}]:{}", self.port)
            }
        }
    }
}

static TABLE: Mutex<Vec<Entry>> = Mutex::new(Vec::new());
static SENDTO_TARGET: AtomicUsize = AtomicUsize::new(0);
static SENDTO_ORIG: AtomicUsize = AtomicUsize::new(0);
static WSA_TARGET: AtomicUsize = AtomicUsize::new(0);
static WSA_ORIG: AtomicUsize = AtomicUsize::new(0);
/// recvfrom: hooked only while Ping observes the session (`watch_receive`).
static RECV_TARGET: AtomicUsize = AtomicUsize::new(0);
static RECV_ORIG: AtomicUsize = AtomicUsize::new(0);
static IN_FLIGHT: AtomicUsize = AtomicUsize::new(0);

fn table() -> MutexGuard<'static, Vec<Entry>> {
    TABLE.lock().unwrap_or_else(|e| e.into_inner())
}

/// Reads a u16 from two big-endian bytes (network byte order, used for ports).
fn be16(bytes: &[u8]) -> u16 {
    u16::from_be_bytes([bytes[0], bytes[1]])
}

/// The game sends IPv4 traffic through IPv6 sockets, so an IPv4 destination
/// arrives as the IPv4-mapped address `::ffff:a.b.c.d`. Returns it as plain IPv4.
fn normalize(family: u16, addr: [u8; 16]) -> (u16, [u8; 16]) {
    let mapped = family == AF_INET6
        && addr[..10].iter().all(|&b| b == 0)
        && addr[10] == 0xff
        && addr[11] == 0xff;
    if !mapped {
        return (family, addr);
    }
    let mut v4 = [0u8; 16];
    v4[..4].copy_from_slice(&addr[12..]);
    (AF_INET, v4)
}

/// Addresses that are never a game server: unspecified, loopback, multicast
/// and the limited broadcast. LAN/private addresses are kept (LAN worlds).
/// A subnet's own broadcast address is filtered when queried instead, since
/// telling needs the adapter list (see `local_broadcasts`).
fn ignorable(family: u16, addr: &[u8; 16]) -> bool {
    match family {
        AF_INET => {
            let [a, b, c, d] = [addr[0], addr[1], addr[2], addr[3]];
            a == 0                       // 0.0.0.0
                || a == 127              // loopback
                || (224..=239).contains(&a) // multicast
                || (a, b, c, d) == (255, 255, 255, 255) // broadcast
        }
        AF_INET6 => {
            let unspecified = addr.iter().all(|&b| b == 0);
            let loopback = addr[..15].iter().all(|&b| b == 0) && addr[15] == 1;
            let multicast = addr[0] == 0xff;
            unspecified || loopback || multicast
        }
        _ => true,
    }
}

/// Counts one packet to `addr`, adding it to the table if new.
fn observe(table: &mut Vec<Entry>, family: u16, addr: [u8; 16], port: u16, now: u64) {
    let (family, addr) = normalize(family, addr);
    if ignorable(family, &addr) {
        return;
    }
    if let Some(entry) = table.iter_mut().find(|e| e.same_as(family, &addr, port)) {
        entry.hit(now);
        return;
    }
    if table.len() >= MAX_ENTRIES {
        // Evict the least recently used.
        if let Some((i, _)) = table.iter().enumerate().min_by_key(|(_, e)| e.last_ms) {
            table.swap_remove(i);
        }
    }
    table.push(Entry {
        family,
        addr,
        port,
        last_ms: now,
        window_start: now,
        window_count: 1,
        rate: 0,
    });
}

/// The address receiving the steadiest stream, if one looks like a server.
fn best(table: &[Entry], now: u64, broadcasts: &[[u8; 4]]) -> Option<String> {
    table
        .iter()
        .filter(|e| e.current(now) && e.rate >= MIN_RATE && !e.is_broadcast(broadcasts))
        .max_by_key(|e| e.rate)
        .map(Entry::format)
}

/// The (family, address, port) of a WinSock `sockaddr`, not normalized.
///
/// # Safety
/// `sockaddr` must point to at least `len` readable bytes (as passed to the
/// hooked WinSock call).
unsafe fn address(sockaddr: *const u8, len: i32) -> Option<(u16, [u8; 16], u16)> {
    if sockaddr.is_null() || len < 4 {
        return None;
    }
    let family = u16::from_ne_bytes([*sockaddr, *sockaddr.add(1)]);
    let read = |start: usize, n: usize| std::slice::from_raw_parts(sockaddr.add(start), n);
    let mut addr = [0u8; 16];
    match family {
        AF_INET if len >= 8 => {
            addr[..4].copy_from_slice(read(4, 4));
            Some((family, addr, be16(read(2, 2))))
        }
        AF_INET6 if len >= 24 => {
            addr.copy_from_slice(read(8, 16));
            Some((family, addr, be16(read(2, 2))))
        }
        _ => None,
    }
}

/// Records one outbound destination. Called from the game's network thread(s),
/// so it never blocks: if the table is busy it drops the sample.
///
/// # Safety
/// As for `address`.
unsafe fn record(sockaddr: *const u8, len: i32) {
    let Some((family, addr, port)) = address(sockaddr, len) else {
        return;
    };
    let now = GetTickCount64();
    if let Ok(mut table) = TABLE.try_lock() {
        observe(&mut table, family, addr, port, now);
    }
}

/// Hands one datagram to Ping's passive session observer (read only).
///
/// # Safety
/// As for `address`; `buf` must point to `len` readable bytes.
unsafe fn observe_session(sockaddr: *const u8, len: i32, buf: *const u8, size: i32, sent: bool) {
    use crate::modules::ping::session;
    if !session::active() || buf.is_null() || size <= 0 {
        return;
    }
    let Some((family, addr, port)) = address(sockaddr, len) else {
        return;
    };
    let (family, addr) = normalize(family, addr);
    let bytes = std::slice::from_raw_parts(buf, size as usize);
    if sent {
        session::on_send((family, addr, port), bytes);
    } else {
        session::on_receive((family, addr, port), bytes);
    }
}

unsafe extern "system" fn sendto_detour(
    s: usize,
    buf: *const u8,
    len: i32,
    flags: i32,
    to: *const u8,
    tolen: i32,
) -> i32 {
    IN_FLIGHT.fetch_add(1, Ordering::SeqCst);
    record(to, tolen);
    observe_session(to, tolen, buf, len, true);
    let original: SendToFn = transmute(SENDTO_ORIG.load(Ordering::Acquire));
    let result = original(s, buf, len, flags, to, tolen);
    IN_FLIGHT.fetch_sub(1, Ordering::SeqCst);
    result
}

#[allow(clippy::too_many_arguments)]
unsafe extern "system" fn wsa_sendto_detour(
    s: usize,
    buffers: *const c_void,
    count: u32,
    sent: *mut u32,
    flags: u32,
    to: *const u8,
    tolen: i32,
    overlapped: *mut c_void,
    routine: *mut c_void,
) -> i32 {
    IN_FLIGHT.fetch_add(1, Ordering::SeqCst);
    record(to, tolen);
    if count >= 1 && !buffers.is_null() {
        // WSABUF { ULONG len; CHAR* buf; }: the first buffer only.
        let first = buffers.cast::<(u32, *const u8)>().read_unaligned();
        observe_session(
            to,
            tolen,
            first.1,
            first.0.min(i32::MAX as u32) as i32,
            true,
        );
    }
    let original: WsaSendToFn = transmute(WSA_ORIG.load(Ordering::Acquire));
    let result = original(
        s, buffers, count, sent, flags, to, tolen, overlapped, routine,
    );
    IN_FLIGHT.fetch_sub(1, Ordering::SeqCst);
    result
}

unsafe extern "system" fn recvfrom_detour(
    s: usize,
    buf: *mut u8,
    len: i32,
    flags: i32,
    from: *mut u8,
    fromlen: *mut i32,
) -> i32 {
    IN_FLIGHT.fetch_add(1, Ordering::SeqCst);
    let original: RecvFromFn = transmute(RECV_ORIG.load(Ordering::Acquire));
    let result = original(s, buf, len, flags, from, fromlen);
    // Read only what the call returned; nothing is changed.
    if result > 0 && !fromlen.is_null() {
        observe_session(from, *fromlen, buf, result.min(len), false);
    }
    IN_FLIGHT.fetch_sub(1, Ordering::SeqCst);
    result
}

/// Whether `recvfrom` is hooked (Ping's session observer).
pub fn receive_hooked() -> bool {
    RECV_TARGET.load(Ordering::Acquire) != 0
}

/// Hooks `recvfrom` while Ping observes the session, and unhooks it after,
/// so the game's receive path is untouched otherwise.
pub fn watch_receive(on: bool) {
    unsafe {
        if on {
            if RECV_TARGET.load(Ordering::Acquire) != 0 {
                return;
            }
            match MH_Initialize() {
                MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
                e => {
                    log!("net: MH_Initialize failed: {e:?}");
                    return;
                }
            }
            if let Err(e) = hook(
                c"recvfrom",
                recvfrom_detour as *mut c_void,
                &RECV_TARGET,
                &RECV_ORIG,
            ) {
                log!("Ping: 受信の観測を開始できません（{e}）");
            }
        } else {
            let target = RECV_TARGET.load(Ordering::Acquire);
            if target != 0 && crate::hooks::disable(target) {
                RECV_TARGET.store(0, Ordering::Release);
            }
        }
    }
}

/// Resolves and hooks one WinSock export. Leaves `target`/`orig` at 0 on failure.
unsafe fn hook(
    name: &std::ffi::CStr,
    detour: *mut c_void,
    target: &AtomicUsize,
    orig: &AtomicUsize,
) -> Result<(), String> {
    let ws2 = GetModuleHandleW("ws2_32.dll\0".encode_utf16().collect::<Vec<_>>().as_ptr());
    if ws2.is_null() {
        return Err("ws2_32.dll not loaded".into());
    }
    let proc = GetProcAddress(ws2, name.as_ptr().cast())
        .ok_or_else(|| format!("{} not found", name.to_string_lossy()))?
        as *mut c_void;
    // Created once and kept: after `uninstall` the hook is only disabled.
    if orig.load(Ordering::Acquire) == 0 {
        let mut trampoline = std::ptr::null_mut();
        MH_CreateHook(proc, detour, &mut trampoline)
            .ok()
            .map_err(|e| format!("MH_CreateHook({}): {e:?}", name.to_string_lossy()))?;
        orig.store(trampoline as usize, Ordering::Release);
    }
    MH_EnableHook(proc)
        .ok()
        .map_err(|e| format!("MH_EnableHook({}): {e:?}", name.to_string_lossy()))?;
    target.store(proc as usize, Ordering::Release);
    Ok(())
}

/// Installs the send hooks so the connected server can be detected. Safe to
/// call more than once. Failure is logged but not fatal: `current()` just
/// reports nothing.
pub fn init() {
    if SENDTO_TARGET.load(Ordering::Acquire) != 0 || WSA_TARGET.load(Ordering::Acquire) != 0 {
        return;
    }
    unsafe {
        match MH_Initialize() {
            MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
            e => {
                log!("net: MH_Initialize failed: {e:?}");
                return;
            }
        }
        // Hook both; whichever the game does not use simply never fires.
        let a = hook(
            c"sendto",
            sendto_detour as *mut c_void,
            &SENDTO_TARGET,
            &SENDTO_ORIG,
        );
        let b = hook(
            c"WSASendTo",
            wsa_sendto_detour as *mut c_void,
            &WSA_TARGET,
            &WSA_ORIG,
        );
        if let (Err(ea), Err(eb)) = (&a, &b) {
            log!("net: server-IP detection unavailable ({ea}; {eb})");
        }
    }
}

/// Disable new calls, then drain calls including a possibly blocked socket send.
/// Failure means the caller must keep the DLL mapped, just as for input/chat.
pub fn uninstall() -> bool {
    for target in [&SENDTO_TARGET, &WSA_TARGET, &RECV_TARGET] {
        let addr = target.load(Ordering::Acquire);
        if addr != 0 {
            if !crate::hooks::disable(addr) {
                return false;
            }
            target.store(0, Ordering::Release);
        }
    }
    if !crate::hooks::drain(&IN_FLIGHT, 2) {
        return false;
    }
    table().clear();
    true
}

// --- Local subnets -----------------------------------------------------------------

/// The broadcast address of the IPv4 subnet `addr`/`prefix`, if it has one.
fn broadcast_of(addr: [u8; 4], prefix: u8) -> Option<[u8; 4]> {
    // /31 and /32 have no broadcast address; /0 is not a real subnet.
    if !(1..=30).contains(&prefix) {
        return None;
    }
    let host_mask = u32::MAX >> prefix;
    Some((u32::from_be_bytes(addr) | host_mask).to_be_bytes())
}

/// Asks Windows for every adapter's IPv4 addresses and returns their subnets'
/// broadcast addresses. Empty on failure.
fn query_broadcasts() -> Vec<[u8; 4]> {
    let flags = GAA_FLAG_SKIP_ANYCAST | GAA_FLAG_SKIP_MULTICAST | GAA_FLAG_SKIP_DNS_SERVER;
    // u64 storage keeps the buffer aligned for the adapter structures.
    let mut buffer: Vec<u64> = Vec::new();
    let mut size = 16 * 1024u32;
    let mut ok = false;
    for _ in 0..4 {
        buffer.resize((size as usize).div_ceil(8), 0);
        let result = unsafe {
            GetAdaptersAddresses(
                AF_INET as u32,
                flags,
                std::ptr::null(),
                buffer.as_mut_ptr().cast::<IP_ADAPTER_ADDRESSES_LH>(),
                &mut size,
            )
        };
        const ERROR_BUFFER_OVERFLOW: u32 = 111;
        match result {
            0 => {
                ok = true;
                break;
            }
            ERROR_BUFFER_OVERFLOW => continue, // `size` now holds what is needed
            _ => break,
        }
    }
    let mut out = Vec::new();
    if !ok {
        return out;
    }
    unsafe {
        let mut adapter = buffer.as_ptr().cast::<IP_ADAPTER_ADDRESSES_LH>();
        while !adapter.is_null() {
            let mut unicast = (*adapter).FirstUnicastAddress;
            while !unicast.is_null() {
                let sockaddr = (*unicast).Address.lpSockaddr.cast::<u8>();
                let len = (*unicast).Address.iSockaddrLength;
                if !sockaddr.is_null()
                    && len >= 8
                    && u16::from_ne_bytes([*sockaddr, *sockaddr.add(1)]) == AF_INET
                {
                    let mut addr = [0u8; 4];
                    addr.copy_from_slice(std::slice::from_raw_parts(sockaddr.add(4), 4));
                    if let Some(b) = broadcast_of(addr, (*unicast).OnLinkPrefixLength) {
                        out.push(b);
                    }
                }
                unicast = (*unicast).Next;
            }
            adapter = (*adapter).Next;
        }
    }
    out
}

/// (GetTickCount64 of the last adapter query, or 0 for none; its broadcasts)
static BROADCASTS: Mutex<(u64, Vec<[u8; 4]>)> = Mutex::new((0, Vec::new()));

/// Queries the adapters again when the cached list is `BROADCAST_REFRESH_MS`
/// old (or missing). Called from the client's main loop, so the query (a few
/// milliseconds) never runs on the render thread that draws the server HUD.
/// The lock is not held during the query.
pub fn refresh() {
    let now = unsafe { GetTickCount64() };
    let at = BROADCASTS.lock().unwrap_or_else(|e| e.into_inner()).0;
    if at != 0 && now.saturating_sub(at) < BROADCAST_REFRESH_MS {
        return;
    }
    let fresh = query_broadcasts();
    *BROADCASTS.lock().unwrap_or_else(|e| e.into_inner()) = (now.max(1), fresh);
}

/// Broadcast addresses of the local IPv4 subnets (e.g. 192.168.0.255), which
/// LAN discovery sends to. Kept up to date by `refresh`; queried here only if
/// it has never run.
fn local_broadcasts() -> Vec<[u8; 4]> {
    {
        let cache = BROADCASTS.lock().unwrap_or_else(|e| e.into_inner());
        if cache.0 != 0 {
            return cache.1.clone();
        }
    }
    refresh();
    BROADCASTS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .1
        .clone()
}

// --- Queries -----------------------------------------------------------------------

/// The address of the connected server, if packets are streaming to one.
pub fn current() -> Option<String> {
    let broadcasts = local_broadcasts();
    best(&table(), unsafe { GetTickCount64() }, &broadcasts)
}

/// Every destination sent to recently with its packets per second, busiest
/// first, for diagnostics. Subnet broadcasts are included and marked.
pub fn recent() -> Vec<(String, u32)> {
    let broadcasts = local_broadcasts();
    let now = unsafe { GetTickCount64() };
    let mut rows: Vec<(String, u32)> = table()
        .iter()
        .filter(|e| e.current(now))
        .map(|e| {
            let note = if e.is_broadcast(&broadcasts) {
                " (broadcast)"
            } else {
                ""
            };
            (format!("{}{note}", e.format()), e.rate)
        })
        .collect();
    rows.sort_by_key(|a| std::cmp::Reverse(a.1));
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v4(a: u8, b: u8, c: u8, d: u8) -> [u8; 16] {
        let mut addr = [0u8; 16];
        addr[..4].copy_from_slice(&[a, b, c, d]);
        addr
    }

    fn mapped(a: u8, b: u8, c: u8, d: u8) -> [u8; 16] {
        let mut addr = [0u8; 16];
        addr[10] = 0xff;
        addr[11] = 0xff;
        addr[12..].copy_from_slice(&[a, b, c, d]);
        addr
    }

    /// Sends `per_second` packets a second to one address from `from` to `to` (ms).
    fn stream(
        table: &mut Vec<Entry>,
        family: u16,
        addr: [u8; 16],
        per_second: u64,
        from: u64,
        to: u64,
    ) {
        let step = 1000 / per_second;
        let mut now = from;
        while now < to {
            observe(table, family, addr, 19132, now);
            now += step;
        }
    }

    #[test]
    fn ignores_loopback_broadcast_and_multicast_but_keeps_lan() {
        assert!(ignorable(AF_INET, &v4(127, 0, 0, 1)));
        assert!(ignorable(AF_INET, &v4(0, 0, 0, 0)));
        assert!(ignorable(AF_INET, &v4(255, 255, 255, 255)));
        assert!(ignorable(AF_INET, &v4(224, 0, 2, 60)));
        assert!(!ignorable(AF_INET, &v4(192, 168, 1, 20)));
        assert!(!ignorable(AF_INET, &v4(203, 0, 113, 9)));
    }

    #[test]
    fn ipv6_loopback_and_multicast_ignored() {
        let mut loopback = [0u8; 16];
        loopback[15] = 1;
        assert!(ignorable(AF_INET6, &loopback));
        assert!(ignorable(AF_INET6, &[0u8; 16]));
        let mut multicast = [0u8; 16];
        multicast[0] = 0xff;
        assert!(ignorable(AF_INET6, &multicast));
        let mut global = [0u8; 16];
        global[0] = 0x20;
        assert!(!ignorable(AF_INET6, &global));
    }

    #[test]
    fn ipv4_mapped_destinations_are_treated_as_ipv4() {
        assert_eq!(
            normalize(AF_INET6, mapped(203, 0, 113, 9)),
            (AF_INET, v4(203, 0, 113, 9))
        );
        // Real IPv6 and plain IPv4 are left alone.
        let mut global = [0u8; 16];
        global[0] = 0x20;
        assert_eq!(normalize(AF_INET6, global), (AF_INET6, global));
        assert_eq!(
            normalize(AF_INET, v4(1, 2, 3, 4)),
            (AF_INET, v4(1, 2, 3, 4))
        );

        // So the IPv4 rules apply to them: loopback and broadcast are dropped...
        let mut table = Vec::new();
        stream(&mut table, AF_INET6, mapped(127, 0, 0, 1), 20, 0, 3000);
        stream(
            &mut table,
            AF_INET6,
            mapped(255, 255, 255, 255),
            20,
            0,
            3000,
        );
        assert!(table.is_empty());
        // ...and a server is reported in IPv4 form.
        stream(&mut table, AF_INET6, mapped(203, 0, 113, 9), 20, 0, 3000);
        assert_eq!(best(&table, 3000, &[]), Some("203.0.113.9:19132".into()));
    }

    /// The case seen in the game: on the main menu the only traffic is LAN
    /// discovery to the subnet broadcast address, about once a second.
    #[test]
    fn lan_discovery_is_not_a_server() {
        let broadcasts = [[192, 168, 0, 255]];
        let mut table = Vec::new();
        stream(&mut table, AF_INET6, mapped(192, 168, 0, 255), 1, 0, 60_000);
        assert_eq!(best(&table, 60_000, &broadcasts), None);
        // Too slow to be a server even where the subnet is unknown.
        assert_eq!(best(&table, 60_000, &[]), None);
        // Even a fast broadcast is not a server.
        stream(
            &mut table,
            AF_INET6,
            mapped(192, 168, 0, 255),
            20,
            60_000,
            63_000,
        );
        assert_eq!(best(&table, 63_000, &broadcasts), None);
        // A LAN host on the same subnet is.
        stream(
            &mut table,
            AF_INET6,
            mapped(192, 168, 0, 20),
            20,
            60_000,
            63_000,
        );
        assert_eq!(
            best(&table, 63_000, &broadcasts),
            Some("192.168.0.20:19132".into())
        );
    }

    #[test]
    fn steady_stream_wins_and_expires_when_it_stops() {
        let mut table = Vec::new();
        // A slow pinger for a long time, then a real connection.
        stream(&mut table, AF_INET, v4(198, 51, 100, 7), 1, 0, 30_000);
        assert_eq!(best(&table, 30_000, &[]), None);
        stream(&mut table, AF_INET, v4(203, 0, 113, 9), 20, 30_000, 33_000);
        stream(&mut table, AF_INET, v4(198, 51, 100, 7), 1, 30_000, 33_000);
        assert_eq!(best(&table, 33_000, &[]), Some("203.0.113.9:19132".into()));
        // Not reported before a full window has been measured.
        let mut fresh = Vec::new();
        stream(&mut fresh, AF_INET, v4(203, 0, 113, 9), 20, 0, 900);
        assert_eq!(best(&fresh, 900, &[]), None);
        // After disconnecting it drops out.
        assert_eq!(best(&table, 33_000 + STALE_MS + 100, &[]), None);
    }

    #[test]
    fn a_pause_resets_the_rate() {
        let mut table = Vec::new();
        stream(&mut table, AF_INET, v4(203, 0, 113, 9), 20, 0, 3000);
        assert!(table[0].rate >= 15, "{}", table[0].rate);
        // One packet long after: the old count must not look like a rate.
        observe(&mut table, AF_INET, v4(203, 0, 113, 9), 19132, 60_000);
        assert_eq!(table[0].rate, 0);
    }

    #[test]
    fn subnet_broadcast_addresses() {
        assert_eq!(
            broadcast_of([192, 168, 0, 17], 24),
            Some([192, 168, 0, 255])
        );
        assert_eq!(broadcast_of([10, 1, 2, 3], 8), Some([10, 255, 255, 255]));
        assert_eq!(broadcast_of([172, 16, 5, 9], 20), Some([172, 16, 15, 255]));
        assert_eq!(broadcast_of([192, 168, 0, 17], 30), Some([192, 168, 0, 19]));
        assert_eq!(broadcast_of([192, 168, 0, 17], 31), None);
        assert_eq!(broadcast_of([192, 168, 0, 17], 32), None);
        assert_eq!(broadcast_of([192, 168, 0, 17], 0), None);
    }

    #[test]
    fn formats_v4_and_v6() {
        let entry = |family, addr| Entry {
            family,
            addr,
            port: 19132,
            last_ms: 0,
            window_start: 0,
            window_count: 0,
            rate: 0,
        };
        assert_eq!(
            entry(AF_INET, v4(203, 0, 113, 9)).format(),
            "203.0.113.9:19132"
        );
        let mut addr = [0u8; 16];
        addr[0] = 0x20;
        addr[1] = 0x01;
        addr[15] = 0x01;
        assert_eq!(entry(AF_INET6, addr).format(), "[2001::1]:19132");
    }

    /// End to end: real `sendto` calls go through the hook, and the adapter
    /// list is read from Windows. The packets stay on this machine (sent to a
    /// socket bound on its own LAN address).
    #[test]
    fn hook_sees_real_sendto_and_stops_after_uninstall() {
        use std::net::UdpSocket;
        use std::time::Duration;
        let _serial = crate::modules::ping::session::TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());

        // Learn a non-loopback local address; connecting a UDP socket sends nothing.
        let probe = UdpSocket::bind("0.0.0.0:0").unwrap();
        if probe.connect("192.0.2.1:9").is_err() {
            return; // no network
        }
        let Ok(local) = probe.local_addr() else {
            return;
        };
        if local.ip().is_loopback() || local.ip().is_unspecified() {
            return;
        }
        // This machine's own subnet is among the ones Windows reports.
        let std::net::IpAddr::V4(local_v4) = local.ip() else {
            return;
        };
        let broadcasts = local_broadcasts();
        assert!(!broadcasts.is_empty());
        assert!(!broadcasts.contains(&local_v4.octets()));

        let receiver = UdpSocket::bind((local.ip(), 0)).unwrap();
        let target = receiver.local_addr().unwrap();
        let sender = UdpSocket::bind((local.ip(), 0)).unwrap();
        // 25 packets a second for a little over one rate window.
        let send_burst = || {
            for _ in 0..40 {
                sender.send_to(&[0], target).unwrap();
                std::thread::sleep(Duration::from_millis(40));
            }
        };

        init();
        send_burst();
        assert_eq!(current(), Some(target.to_string()));
        assert_eq!(
            recent().first().map(|r| r.0.clone()),
            Some(target.to_string())
        );

        uninstall();
        send_burst();
        assert_eq!(current(), None);
    }

    /// End to end on loopback: RakNet-shaped datagrams sent with `sendto`
    /// and acknowledgements received with `recvfrom` reach Ping's observer
    /// through the hooks, which change nothing; switching it off unhooks.
    #[test]
    fn session_observer_reads_real_sendto_and_recvfrom() {
        use crate::modules::ping::session;
        use std::net::UdpSocket;
        use std::time::Duration;
        let _serial = session::TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let server = UdpSocket::bind("127.0.0.1:0").unwrap();
        let client = UdpSocket::bind("127.0.0.1:0").unwrap();
        for socket in [&server, &client] {
            socket
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
        }
        let target = server.local_addr().unwrap();
        init();
        session::set_enabled(true);
        assert_ne!(RECV_TARGET.load(Ordering::Acquire), 0);
        session::set_target(Some(target));
        let mut buffer = [0u8; 64];
        for seq in 0u8..4 {
            // A data datagram: flags, number, one unreliable frame.
            let datagram = [0x84, seq, 0, 0, 0x00, 0, 8, 0xFE];
            client.send_to(&datagram, target).unwrap();
            let (n, from) = server.recv_from(&mut buffer).unwrap();
            assert_eq!(&buffer[..n], &datagram, "the hook must not change it");
            std::thread::sleep(Duration::from_millis(5));
            let ack = [0xC0, 0, 1, 1, seq, 0, 0];
            server.send_to(&ack, from).unwrap();
            let (n, _) = client.recv_from(&mut buffer).unwrap();
            assert_eq!(&buffer[..n], &ack, "the hook must not change it");
        }
        let reading = session::reading();
        assert!(
            matches!(reading, Some((ms, _)) if (4. ..1000.).contains(&ms)),
            "{reading:?}"
        );
        session::set_enabled(false);
        assert_eq!(RECV_TARGET.load(Ordering::Acquire), 0);
        assert_eq!(session::reading(), None);
        uninstall();
    }
}

#[cfg(test)]
mod bench {
    /// Cost of one adapter query, which `local_broadcasts` repeats every
    /// `BROADCAST_REFRESH_MS` on whatever thread asks (the HUD: the render thread).
    #[test]
    #[ignore = "manual: cargo test -p client --release --lib query_broadcasts_cost -- --ignored --nocapture"]
    fn query_broadcasts_cost() {
        let mut times: Vec<_> = (0..50)
            .map(|_| {
                let start = std::time::Instant::now();
                std::hint::black_box(super::query_broadcasts());
                start.elapsed()
            })
            .collect();
        times.sort();
        println!(
            "GetAdaptersAddresses + parse: min {:?}, median {:?}, max {:?} ({} subnets)",
            times[0],
            times[25],
            times[49],
            super::query_broadcasts().len()
        );
    }
}
