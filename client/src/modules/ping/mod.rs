//! Opt-in Ping. The connected session's round trip, observed read-only on
//! the game's own RakNet traffic (`session`), comes first; until there is one
//! (or for connections it cannot read) a separate server-list ping fills in.
//! Network waits never run on the client/render thread.
mod probe;
pub(crate) mod session;

use std::collections::VecDeque;
use std::net::SocketAddr;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::Duration;

use hudhook::imgui::Ui;

use super::settings::{Detail, Value};
use super::{key_or_warn, Module, Tick};
use crate::{config, log, net};
use probe::Reading;

struct State {
    stop: bool,
    generation: u64,
    peer: Option<SocketAddr>,
    interval_ms: u64,
    reading: Option<Reading>,
    samples: VecDeque<f64>,
    attempts: u64,
    failures: u64,
}
impl State {
    fn new(interval_ms: u64) -> Self {
        Self {
            stop: false,
            generation: 0,
            peer: None,
            interval_ms,
            reading: None,
            samples: VecDeque::with_capacity(20),
            attempts: 0,
            failures: 0,
        }
    }
    fn configure(&mut self, peer: Option<SocketAddr>, interval_ms: u64) -> bool {
        if self.peer == peer && self.interval_ms == interval_ms {
            return false;
        }
        self.generation = self.generation.wrapping_add(1);
        self.peer = peer;
        self.interval_ms = interval_ms;
        self.reading = None;
        self.samples.clear();
        self.attempts = 0;
        self.failures = 0;
        true
    }
    fn publish(&mut self, generation: u64, reading: Reading) {
        if self.stop || self.generation != generation || reading == Reading::Cancelled {
            return;
        }
        self.attempts = self.attempts.saturating_add(1);
        if let Reading::Rtt(ms) = reading {
            if self.samples.len() == 20 {
                self.samples.pop_front();
            }
            self.samples.push_back(ms);
        } else {
            self.failures = self.failures.saturating_add(1);
        }
        self.reading = Some(reading);
    }
    fn text(&self) -> String {
        if self.peer.is_none() {
            return "接続先サーバーを待機中".into();
        }
        match &self.reading {
            Some(Reading::Rtt(ms)) => format!("{ms:.1} ms（サーバー一覧RTT）"),
            Some(Reading::Timeout) => "接続の応答を待機中".into(),
            Some(Reading::Error(e)) => format!("取得不可: {e}"),
            _ => "測定中（サーバー一覧RTT）".into(),
        }
    }
}

type Shared = Arc<(Mutex<State>, Condvar)>;
fn lock(shared: &Shared) -> MutexGuard<'_, State> {
    shared.0.lock().unwrap_or_else(|e| e.into_inner())
}

struct Worker {
    shared: Shared,
    thread: Option<JoinHandle<()>>,
}
impl Worker {
    fn start(interval_ms: u64) -> std::io::Result<Self> {
        let shared = Arc::new((Mutex::new(State::new(interval_ms)), Condvar::new()));
        let state = Arc::clone(&shared);
        let thread = std::thread::Builder::new()
            .name("mio-ping".into())
            .spawn(move || run(state))?;
        Ok(Self {
            shared,
            thread: Some(thread),
        })
    }
    fn configure(&self, peer: Option<SocketAddr>, interval_ms: u64) {
        if lock(&self.shared).configure(peer, interval_ms) {
            self.shared.1.notify_one();
        }
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        lock(&self.shared).stop = true;
        self.shared.1.notify_one();
        // Never detach a DLL-owned thread: it must exit before the DLL unloads.
        // A running probe checks cancellation between <=50ms socket waits.
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
fn run(shared: Shared) {
    let guid = (unsafe { windows_sys::Win32::System::Threading::GetCurrentProcessId() } as u64)
        << 32
        | unsafe { windows_sys::Win32::System::SystemInformation::GetTickCount64() };
    loop {
        let (peer, generation, interval_ms) = {
            let state = shared
                .1
                .wait_while(lock(&shared), |s| !s.stop && s.peer.is_none())
                .unwrap_or_else(|e| e.into_inner());
            if state.stop {
                return;
            }
            (state.peer.unwrap(), state.generation, state.interval_ms)
        };
        // While the session itself gives a round trip, send nothing.
        let reading = session::reading().is_none().then(|| {
            probe::measure(peer, guid, || {
                let state = lock(&shared);
                state.stop || state.generation != generation
            })
        });
        let mut state = lock(&shared);
        if let Some(reading) = reading {
            state.publish(generation, reading);
        }
        // Generation changes cancel the old result and immediately probe the
        // new destination. Spurious wakeups do not create extra packets.
        let _ = shared
            .1
            .wait_timeout_while(state, Duration::from_millis(interval_ms), |s| {
                !s.stop && s.generation == generation
            })
            .unwrap_or_else(|e| e.into_inner());
        if lock(&shared).stop {
            return;
        }
    }
}

pub struct Ping {
    key: u16,
    x: i32,
    y: i32,
    interval_ms: u64,
    next: u64,
    worker: Option<Worker>,
}
impl Ping {
    pub fn new(c: &config::Ping) -> Self {
        Self {
            key: key_or_warn("ping", &c.key),
            x: c.x.clamp(-7680, 7680),
            y: c.y.clamp(-4320, 4320),
            interval_ms: c.interval_ms.clamp(1000, 10000),
            next: 0,
            worker: None,
        }
    }
}
impl Module for Ping {
    fn name(&self) -> &str {
        "ping"
    }
    fn key(&self) -> u16 {
        self.key
    }
    fn enabled(&self) -> bool {
        self.worker.is_some()
    }
    fn set_enabled(&mut self, on: bool) {
        if on == self.enabled() {
            return;
        }
        if on {
            match Worker::start(self.interval_ms) {
                Ok(worker) => {
                    self.worker = Some(worker);
                    self.next = 0;
                    session::set_enabled(true);
                }
                Err(e) => log!("Ping: 測定スレッドを開始できません: {e}"),
            }
        } else {
            session::set_enabled(false);
            self.worker = None;
        }
    }
    fn tick(&mut self, tick: &Tick) {
        if tick.now_ms < self.next {
            return;
        }
        let Some(worker) = &self.worker else {
            return;
        };
        self.next = tick.now_ms.saturating_add(500);
        // Numeric endpoint only: never block on a DNS lookup. The detector
        // filters LAN discovery broadcasts and expires a departed server.
        let peer = net::current().and_then(|s| s.parse().ok());
        session::set_target(peer);
        worker.configure(peer, self.interval_ms);
    }
    fn status(&self) -> String {
        let Some(worker) = &self.worker else {
            return String::new();
        };
        match session::reading() {
            Some((ms, _)) => format!("{ms:.0} ms（接続）"),
            None => lock(&worker.shared).text(),
        }
    }
    fn options(&self) -> Vec<Detail> {
        vec![
            Detail::integer(
                "interval_ms",
                "測定間隔（ms）",
                "サーバー一覧用Pingを1回送ります。応答待ちは別スレッドで最大1500msです。",
                self.interval_ms as i32,
                1000,
                10000,
            ),
            Detail::x_offset(self.x, "Negative: from right."),
            Detail::y_offset(self.y, "Negative: from bottom."),
        ]
    }
    fn set_option(&mut self, id: &str, v: &Value) -> Result<(), String> {
        let value = v.number()?;
        if !value.is_finite() {
            return Err("有限の数値を指定してください".into());
        }
        match id {
            "interval_ms" => {
                self.interval_ms = value.clamp(1000., 10000.) as u64;
                self.next = 0;
            }
            "x" => self.x = value.clamp(-7680., 7680.) as i32,
            "y" => self.y = value.clamp(-4320., 4320.) as i32,
            _ => return Err(format!("不明な設定項目: {id}")),
        }
        Ok(())
    }
    fn draw_settings(&mut self, ui: &Ui) {
        ui.text_wrapped("ゲームの接続そのものの往復時間を、ゲームの通信（RakNetの接続中Pingの応答とACK、NetherNetの接続確認STUN）から読み取り専用で観測します。こちらからは何も送りません。取得できるまでの間は、サーバー一覧用Pingの往復時間で補います。");
        if let Some(worker) = &self.worker {
            match session::reading() {
                Some((ms, session::Source::Ping)) => {
                    ui.text(format!("接続: {ms:.1} ms（接続中Pingの応答）"))
                }
                Some((ms, session::Source::Ack)) => {
                    ui.text(format!("接続: {ms:.1} ms（ACKからの推定、直近の中央値）"))
                }
                Some((ms, session::Source::Stun)) => ui.text(format!(
                    "接続: {ms:.1} ms（NetherNetの接続確認STUNの応答、直近の中央値）"
                )),
                None => ui.text("接続: 観測待ち"),
            }
            for line in session::diagnostic() {
                ui.text_disabled(line);
            }
            let state = lock(&worker.shared);
            if let Some(peer) = state.peer {
                ui.text(format!("測定先: {peer}"));
            }
            ui.text(format!(
                "測定{}回 / 取得不可{}回",
                state.attempts, state.failures
            ));
            if !state.samples.is_empty() {
                let mean = state.samples.iter().sum::<f64>() / state.samples.len() as f64;
                let min = state.samples.iter().copied().fold(f64::INFINITY, f64::min);
                let max = state.samples.iter().copied().fold(0., f64::max);
                ui.text(format!(
                    "直近{}件: 平均{mean:.1} / 最小{min:.1} / 最大{max:.1} ms",
                    state.samples.len()
                ));
            }
        }
    }
    fn draw(&mut self, ui: &Ui) {
        if self.enabled() && super::hud_view() {
            super::item_hud::panel(
                ui,
                "ping",
                self.x,
                self.y,
                &[format!("Ping  {}", self.status())],
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn server_changes_and_disconnect_discard_late_results_and_history_is_bounded() {
        let mut state = State::new(2000);
        assert!(state.configure(Some("127.0.0.1:19132".parse().unwrap()), 2000));
        let old = state.generation;
        for i in 0..25 {
            state.publish(old, Reading::Rtt(i as f64));
        }
        assert_eq!(state.samples.len(), 20);
        assert_eq!(state.samples[0], 5.);
        state.publish(old, Reading::Timeout);
        assert_eq!((state.attempts, state.failures), (26, 1));
        assert_eq!(state.text(), "接続の応答を待機中");
        state.configure(Some("127.0.0.1:19133".parse().unwrap()), 2000);
        state.publish(old, Reading::Rtt(999.));
        assert!(state.reading.is_none() && state.samples.is_empty());
        let current = state.generation;
        state.configure(None, 2000);
        state.publish(current, Reading::Rtt(999.));
        assert!(state.reading.is_none());
        assert!(state.text().contains("待機中"));
        state.stop = true;
        state.publish(state.generation, Reading::Timeout);
        assert_eq!(state.attempts, 0);
    }
    #[test]
    fn worker_shutdown_cancels_inflight_probe_and_joins() {
        let server = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        server
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        let worker = Worker::start(2000).unwrap();
        let shared = Arc::clone(&worker.shared);
        worker.configure(Some(server.local_addr().unwrap()), 2000);
        let mut buffer = [0; 128];
        assert_eq!(server.recv_from(&mut buffer).unwrap().0, 33);
        drop(worker);
        // Access to DLL code cannot outlive Worker::drop; the handle was joined.
        assert!(lock(&shared).stop);
        assert_eq!(Arc::strong_count(&shared), 1);
        server
            .set_read_timeout(Some(Duration::from_millis(100)))
            .unwrap();
        assert!(server.recv_from(&mut buffer).is_err());
    }

    #[test]
    fn defaults_are_opt_in_and_legacy_and_shipped_configs_agree() {
        let _serial = session::TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let old: config::Config = toml::from_str("[server]\nenabled = true").unwrap();
        let shipped: config::Config =
            toml::from_str(include_str!("../../../default_config.toml")).unwrap();
        assert!(old.server.enabled && !old.ping.enabled && !shipped.ping.enabled);
        assert_eq!(old.ping.key, "NONE");
        assert_eq!(old.ping.interval_ms, shipped.ping.interval_ms);
        assert_eq!((old.ping.x, old.ping.y), (shipped.ping.x, shipped.ping.y));
        let explicit: config::Config =
            toml::from_str("[ping]\nenabled = true\ninterval_ms = 1\nx = -35\ny = 70").unwrap();
        let mut ping = Ping::new(&explicit.ping);
        assert_eq!(ping.interval_ms, 1000);
        // Construction alone must not launch any network worker.
        assert!(!ping.enabled());
        ping.set_option("interval_ms", &Value::Number(30000.))
            .unwrap();
        assert_eq!(ping.interval_ms, 10000);
        assert!(ping
            .set_option("interval_ms", &Value::Number(f64::NAN))
            .is_err());
        assert_eq!(ping.interval_ms, 10000);
        ping.set_enabled(true);
        assert!(ping.enabled());
        assert!(ping.status().contains("待機中"));
        ping.set_enabled(false);
        assert!(ping.worker.is_none() && ping.status().is_empty());
    }
}
