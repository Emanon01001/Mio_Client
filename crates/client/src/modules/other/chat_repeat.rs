//! Explicitly started, finite normal-chat batches. Configuration never starts sending.
use crate::modules::{
    game, key_or_warn,
    settings::{Detail, Value},
    Module, Tick,
};
use crate::{
    config, gui,
    native_chat::repeat::{self, Batch, Outcome, Ticket},
};
use hudhook::imgui::Ui;
const MIN_INTERVAL_SECONDS: f64 = 0.001;
const MAX_INTERVAL_SECONDS: f64 = 3600.;
const MAX_INTERVAL_MS: i32 = 3_600_000;

fn normalize_interval(seconds: f64) -> f64 {
    (seconds.clamp(MIN_INTERVAL_SECONDS, MAX_INTERVAL_SECONDS) * 1000.).round() / 1000.
}

/// A paused game screen may suspend the callback that owns a request. Restart
/// the native-response watchdog after that screen is closed.
fn pending_timed_out(waiting_since: &mut u64, now: u64, active: bool) -> bool {
    if !active {
        *waiting_since = now;
        false
    } else {
        now.saturating_sub(*waiting_since) > 1500
    }
}

#[derive(Default)]
struct Schedule {
    next: Option<u64>,
}
impl Schedule {
    fn due(&mut self, now: u64, active: bool, interval: u64) -> bool {
        if !active {
            self.next = None;
            return false;
        }
        now >= *self
            .next
            .get_or_insert_with(|| now.saturating_add(interval))
    }
    fn completed(&mut self, now: u64, interval: u64) {
        self.next = Some(now.saturating_add(interval));
    }
}
struct Run {
    batch: Batch,
    clock: Schedule,
    pending: Option<(Ticket, u64)>,
}
pub struct ChatRepeat {
    key: u16,
    on: bool,
    text: String,
    interval: f64,
    count: u32,
    sent: u32,
    run: Option<Run>,
    state: &'static str,
    error: String,
}
impl ChatRepeat {
    pub fn new(c: &config::ChatRepeat) -> Self {
        Self {
            key: key_or_warn("chatrepeat", &c.key),
            on: false,
            text: c.text.clone(),
            interval: if c.interval.is_finite() {
                normalize_interval(c.interval)
            } else {
                5.
            },
            count: c.count.clamp(1, 1000) as u32,
            sent: 0,
            run: None,
            state: "開始待ち",
            error: String::new(),
        }
    }
    fn stop(&mut self, state: &'static str) {
        self.run = None;
        self.state = state;
    }
    fn start(&mut self, now: u64) -> Result<(), String> {
        if !self.on {
            return Err("ChatRepeatをONにしてください".into());
        }
        repeat::validate(&self.text)?;
        let batch = Batch::begin(now)?;
        self.run = Some(Run {
            batch,
            clock: Schedule::default(),
            pending: None,
        });
        self.sent = 0;
        self.error.clear();
        self.state = "メニューを閉じると送信を開始します";
        Ok(())
    }
    fn edit(&mut self) {
        self.stop("設定変更により停止 / 開始待ち");
        self.error.clear();
    }
    fn interval_ms(&self) -> u64 {
        (self.interval * 1000.).round() as u64
    }
}
impl Module for ChatRepeat {
    fn name(&self) -> &str {
        "chatrepeat"
    }
    fn key(&self) -> u16 {
        self.key
    }
    fn enabled(&self) -> bool {
        self.on
    }
    fn set_enabled(&mut self, on: bool) {
        if on == self.on {
            return;
        }
        if on {
            if let Err(e) = repeat::available().and_then(|_| game::acquire()) {
                self.error = e;
                return;
            }
            repeat::set_active(true);
            self.on = true;
            self.state = "開始待ち";
        } else {
            self.stop("停止");
            repeat::set_active(false);
            match game::release() {
                Ok(()) => self.on = false,
                Err(e) => self.error = e,
            }
        }
    }
    fn options(&self) -> Vec<Detail> {
        vec![
            Detail::text("text", self.text.clone()),
            Detail::number(
                "interval",
                "送信間隔（秒）",
                "0.001～3600秒。下のミリ秒欄でも入力できます。停止や処理遅延の後も、まとめて連続送信しません。",
                self.interval,
                MIN_INTERVAL_SECONDS,
                MAX_INTERVAL_SECONDS,
            ),
            Detail::integer(
                "count",
                "送信回数",
                "1～1000回。指定回数に達すると停止します。",
                self.count as i32,
                1,
                1000,
            ),
        ]
    }
    fn set_option(&mut self, id: &str, value: &Value) -> Result<(), String> {
        match id {
            "text" => {
                let v = value.text()?;
                if !v.is_empty() {
                    repeat::validate(v)?;
                }
                self.text = v.to_owned();
            }
            "interval" => {
                let v = value.number()?;
                if !v.is_finite() || !(MIN_INTERVAL_SECONDS..=MAX_INTERVAL_SECONDS).contains(&v) {
                    return Err("送信間隔は0.001～3600秒（1～3600000ms）にしてください".into());
                }
                self.interval = normalize_interval(v);
            }
            "count" => {
                let v = value.number()?;
                if !v.is_finite() || !(1. ..=1000.).contains(&v) || v.fract() != 0. {
                    return Err("送信回数は1～1000の整数にしてください".into());
                }
                self.count = v as u32;
            }
            _ => return Err(format!("不明な設定項目: {id}")),
        }
        self.edit();
        Ok(())
    }
    fn tick(&mut self, t: &Tick) {
        let interval = self.interval_ms();
        let Some(run) = self.run.as_mut() else {
            return;
        };
        let world = run.batch.same_world(t.now_ms);
        if world == Some(false) {
            self.stop("ワールドが変わったため停止しました");
            return;
        }
        let chat = repeat::chat_active(t.now_ms);
        let guards = [
            t.active || chat,
            t.focused,
            world == Some(true),
            game::actions::world_input_active(t.now_ms) || chat,
            !game::inventory::screen_open() || chat,
            !gui::captures_input(),
        ];
        let active = guards.iter().all(|&allowed| allowed);
        if guards[1] && guards[5] {
            let flags = guards
                .iter()
                .enumerate()
                .fold(0, |flags, (i, &allowed)| flags | (u64::from(allowed) << i));
            repeat::worker_flags(t.now_ms, flags);
        }
        if active {
            run.batch.resume(t.now_ms);
        } else {
            run.batch.pause();
            run.clock.next = None;
        }
        self.state = if active && chat {
            "送信間隔を待機中（チャット表示中も継続）"
        } else if active {
            "送信間隔を待機中"
        } else {
            "一時停止: メニュー・インベントリ・画面外"
        };
        if let Some((ticket, queued)) = &mut run.pending {
            match ticket.status() {
                Outcome::Pending => {
                    if pending_timed_out(queued, t.now_ms, active) {
                        self.stop("送信処理の応答がないため停止しました");
                    }
                    return;
                }
                Outcome::Sent => {
                    self.sent += 1;
                    run.clock.completed(t.now_ms, interval);
                }
                Outcome::Paused => run.clock.next = None,
                Outcome::Cancelled => {
                    self.stop("停止");
                    return;
                }
                Outcome::Failed(reason) => {
                    self.error = reason;
                    self.stop("送信を停止しました");
                    return;
                }
            }
            run.pending = None;
            if self.sent >= self.count {
                self.stop("指定回数の送信処理が完了しました");
                return;
            }
        }
        if run.clock.due(t.now_ms, active, interval) {
            match run.batch.submit(&self.text, t.now_ms) {
                Ok(ticket) => run.pending = Some((ticket, t.now_ms)),
                Err(reason) => {
                    self.error = reason;
                    self.stop("送信を停止しました");
                }
            }
        }
    }
    fn status(&self) -> String {
        if self.error.is_empty() {
            format!("{} / {}回 / {}", self.sent, self.count, self.state)
        } else {
            format!("{} / {}", self.state, self.error)
        }
    }
    fn draw_settings(&mut self, ui: &Ui) {
        ui.text_colored(crate::theme::MUTED, "送信間隔（ミリ秒）");
        let mut milliseconds = self.interval_ms() as i32;
        ui.set_next_item_width(-1.);
        if ui
            .input_scalar("##interval_ms", &mut milliseconds)
            .step(1)
            .step_fast(100)
            .build()
        {
            self.interval = f64::from(milliseconds.clamp(1, MAX_INTERVAL_MS)) / 1000.;
            self.edit();
        }
        ui.text_colored(
            crate::theme::MUTED,
            format!("{}ms = {:.3}秒", self.interval_ms(), self.interval),
        );
        ui.text_wrapped("1～3600000ms。数値入力と±ボタンで調整できます。実際の送信はゲームの更新周期に合わせて行います。");
        if ui.input_text("本文", &mut self.text).build() {
            if let Some((at, _)) = self.text.char_indices().nth(256) {
                self.text.truncate(at);
            }
            self.edit();
        }
        ui.text_wrapped(
            "通常チャットの本文を、指定した間隔と回数で送信します。チャットを開いている間も継続し、入力中の本文には触れません。ONにしただけでは送信しません。",
        );
        if self.on {
            if self.run.is_some() {
                if ui.button("送信を停止") {
                    self.stop("停止");
                }
            } else if ui.button("送信を開始") {
                let now =
                    unsafe { windows_sys::Win32::System::SystemInformation::GetTickCount64() };
                if let Err(e) = self.start(now) {
                    self.error = e;
                }
            }
        } else {
            ui.text("ChatRepeatをONにしてから開始してください。");
        }
    }
}
impl Drop for ChatRepeat {
    fn drop(&mut self) {
        self.run = None;
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn opening_chat_does_not_time_out_a_request_and_resume_still_has_a_watchdog() {
        let mut since = 100;
        assert!(!pending_timed_out(&mut since, 200, true));
        for now in [300, 2000, 10000, 60000] {
            assert!(!pending_timed_out(&mut since, now, false));
        }
        assert!(!pending_timed_out(&mut since, 60001, true));
        assert!(!pending_timed_out(&mut since, 61500, true));
        assert!(pending_timed_out(&mut since, 61501, true));
    }
    #[test]
    fn millisecond_intervals_survive_config_loading_and_setting_without_float_truncation() {
        for (seconds, expected_ms) in [(0.001, 1), (0.25, 250), (1.001, 1001), (3600., 3_600_000)] {
            let config: config::ChatRepeat =
                toml::from_str(&format!("interval = {seconds}")).unwrap();
            let mut m = ChatRepeat::new(&config);
            assert_eq!(m.interval_ms(), expected_ms);
            m.set_option("interval", &Value::Number(5.)).unwrap();
            m.set_option("interval", &Value::Number(seconds)).unwrap();
            assert_eq!(m.interval_ms(), expected_ms);
            assert!(m.run.is_none());
        }
        let mut m = ChatRepeat::new(&config::ChatRepeat::default());
        for invalid in [0., -1., 0.0009, 3600.001, f64::NAN, f64::INFINITY] {
            assert!(m.set_option("interval", &Value::Number(invalid)).is_err());
            assert_eq!(m.interval_ms(), 5000);
        }
    }
    #[test]
    fn subsecond_deadlines_wait_again_after_pausing_instead_of_catching_up() {
        for interval in [1, 250, 1001] {
            let mut s = Schedule::default();
            assert!(!s.due(1000, true, interval));
            assert!(!s.due(1000 + interval - 1, true, interval));
            assert!(s.due(1000 + interval, true, interval));
            s.completed(9000, interval);
            assert!(!s.due(9000, true, interval));
            assert!(s.due(9000 + interval, true, interval));
            assert!(!s.due(10000, false, interval));
            assert!(!s.due(50000, true, interval));
            assert!(!s.due(50000 + interval - 1, true, interval));
            assert!(s.due(50000 + interval, true, interval));
        }
    }
    #[test]
    fn concurrent_newer_observations_do_not_keep_resetting_the_send_deadline() {
        let world = crate::api::bridge::World(1, 2);
        let mut clock = Schedule::default();
        let mut due_at = None;
        for now in (1000..=7000).step_by(2) {
            let native_at = if now % 20 == 0 { now + 1 } else { now };
            let active = repeat::observed_world(Some((world, native_at)), now) == Some(world);
            if clock.due(now, active, 5000) {
                due_at = Some(now);
                break;
            }
        }
        assert_eq!(due_at, Some(6000));
    }
    #[test]
    fn deadlines_pause_resume_and_delays_never_catch_up_with_bursts() {
        let mut s = Schedule::default();
        assert!(!s.due(0, true, 1000));
        assert!(!s.due(999, true, 1000));
        assert!(s.due(1000, true, 1000));
        s.completed(9000, 1000);
        assert!(!s.due(9000, true, 1000));
        assert!(!s.due(9999, true, 1000));
        assert!(s.due(10000, true, 1000));
        assert!(!s.due(10001, false, 1000));
        assert!(!s.due(50000, true, 1000));
        assert!(s.due(51000, true, 1000));
    }
    #[test]
    fn settings_never_start_a_batch_and_invalid_text_preserves_the_previous_value() {
        let c = config::ChatRepeat {
            text: "hello".into(),
            enabled: true,
            ..Default::default()
        };
        let mut m = ChatRepeat::new(&c);
        assert!(!m.enabled());
        assert!(m.run.is_none());
        assert!(m
            .set_option(
                "text",
                &Value::Text {
                    text: "こんにちは".into()
                }
            )
            .is_ok());
        assert!(m
            .set_option(
                "text",
                &Value::Text {
                    text: "bad\ntext".into()
                }
            )
            .is_err());
        assert_eq!(m.text, "こんにちは");
        assert!(m.set_option("count", &Value::Number(0.)).is_err());
        assert!(m.run.is_none());
    }
}
