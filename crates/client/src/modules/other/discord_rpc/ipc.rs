//! Discord's local RPC: length-prefixed JSON frames over the named pipe
//! `\\.\pipe\discord-ipc-N` that the Discord client listens on.
//!
//! One background thread owns the pipe; the module only hands it the wanted
//! activity. Reads never block (`PeekNamedPipe` first) and the thread checks
//! its stop flag every `POLL`, so disabling the module or ejecting the client
//! joins it promptly.

use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::windows::io::AsRawHandle;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use serde_json::{json, Map, Value};
use windows_sys::Win32::System::Pipes::PeekNamedPipe;

pub(super) const PIPE_PREFIX: &str = r"\\.\pipe\discord-ipc-";

const HANDSHAKE: u32 = 0;
const FRAME: u32 = 1;
const CLOSE: u32 = 2;
const PING: u32 = 3;
const PONG: u32 = 4;
/// Discord's frames are small; anything larger is not Discord.
const MAX_FRAME: usize = 64 * 1024;
const RETRY: Duration = Duration::from_secs(15);
/// Discord accepts 5 activity updates per 20 seconds.
const MIN_UPDATE_GAP: Duration = Duration::from_secs(4);
const POLL: Duration = Duration::from_millis(100);
const READY_TIMEOUT: Duration = Duration::from_secs(5);

/// One frame: opcode and payload length (little-endian u32 each), then JSON.
pub(super) fn encode(op: u32, payload: &Value) -> Vec<u8> {
    let body = payload.to_string().into_bytes();
    let mut out = Vec::with_capacity(8 + body.len());
    out.extend_from_slice(&op.to_le_bytes());
    out.extend_from_slice(&(body.len() as u32).to_le_bytes());
    out.extend_from_slice(&body);
    out
}

/// The activity to show, already in Discord's terms.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Activity {
    pub details: String,
    pub state: Option<String>,
    /// Unix seconds; Discord counts the elapsed time from it.
    pub start: Option<u64>,
    pub large_image: Option<String>,
    pub large_text: Option<String>,
}

/// Discord rejects texts shorter than 2 or longer than 128 characters:
/// longer ones are cut, shorter ones are left out.
fn field(text: &str) -> Option<String> {
    let text = text.trim();
    (text.chars().count() >= 2).then(|| text.chars().take(128).collect())
}

impl Activity {
    pub(super) fn to_json(&self) -> Value {
        let mut activity = Map::new();
        if let Some(details) = field(&self.details) {
            activity.insert("details".into(), details.into());
        }
        if let Some(state) = self.state.as_deref().and_then(field) {
            activity.insert("state".into(), state.into());
        }
        if let Some(start) = self.start {
            activity.insert("timestamps".into(), json!({ "start": start }));
        }
        if let Some(image) = self.large_image.as_deref().map(str::trim) {
            if !image.is_empty() {
                let mut assets = Map::new();
                assets.insert("large_image".into(), image.into());
                if let Some(text) = self.large_text.as_deref().and_then(field) {
                    assets.insert("large_text".into(), text.into());
                }
                activity.insert("assets".into(), assets.into());
            }
        }
        activity.insert("instance".into(), false.into());
        activity.into()
    }
}

/// The SET_ACTIVITY command; `None` clears the activity.
pub(super) fn set_activity(pid: u32, nonce: u64, activity: Option<&Activity>) -> Value {
    json!({
        "cmd": "SET_ACTIVITY",
        "args": { "pid": pid, "activity": activity.map(Activity::to_json) },
        "nonce": nonce.to_string(),
    })
}

struct Connection {
    pipe: File,
    client_id: String,
}

impl Connection {
    fn send(&mut self, op: u32, payload: &Value) -> io::Result<()> {
        self.pipe.write_all(&encode(op, payload))
    }

    /// The bytes waiting in the pipe (copied into `peek`, up to its length)
    /// and their total, without removing them.
    fn peek(&self, peek: &mut [u8]) -> io::Result<usize> {
        let (mut read, mut available) = (0u32, 0u32);
        // SAFETY: the handle is the open pipe; `peek` is writable for its length.
        let ok = unsafe {
            PeekNamedPipe(
                self.pipe.as_raw_handle(),
                peek.as_mut_ptr().cast(),
                peek.len() as u32,
                &mut read,
                &mut available,
                std::ptr::null_mut(),
            )
        };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(available as usize)
    }

    /// A whole frame if one has arrived; never waits for one.
    fn poll(&mut self) -> io::Result<Option<(u32, Value)>> {
        let mut header = [0u8; 8];
        if self.peek(&mut header)? < 8 {
            return Ok(None);
        }
        let op = u32::from_le_bytes(header[..4].try_into().unwrap());
        let len = u32::from_le_bytes(header[4..].try_into().unwrap()) as usize;
        if len > MAX_FRAME {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "frame too large",
            ));
        }
        if self.peek(&mut [])? < 8 + len {
            return Ok(None);
        }
        // Both are in the pipe already: these reads return at once.
        self.pipe.read_exact(&mut header)?;
        let mut body = vec![0; len];
        self.pipe.read_exact(&mut body)?;
        let payload = serde_json::from_slice(&body)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        Ok(Some((op, payload)))
    }
}

fn text(value: &Value, key: &str) -> String {
    value[key].as_str().unwrap_or_default().to_string()
}

/// Opens the first Discord pipe and waits (at most `READY_TIMEOUT`, or until
/// `stop`) for Discord to accept `client_id`.
fn connect(prefix: &str, client_id: &str, stop: &AtomicBool) -> Result<Connection, String> {
    let pipe = (0..10)
        .find_map(|i| {
            OpenOptions::new()
                .read(true)
                .write(true)
                .open(format!("{prefix}{i}"))
                .ok()
        })
        .ok_or("Discordが起動していません（Discordを起動すると自動で接続します）")?;
    let mut connection = Connection {
        pipe,
        client_id: client_id.to_string(),
    };
    let lost = |e: io::Error| format!("Discordとの接続に失敗しました: {e}");
    connection
        .send(HANDSHAKE, &json!({ "v": 1, "client_id": client_id }))
        .map_err(lost)?;
    let deadline = Instant::now() + READY_TIMEOUT;
    while Instant::now() < deadline && !stop.load(Ordering::Acquire) {
        match connection.poll().map_err(lost)? {
            Some((FRAME, payload)) if payload["evt"] == "READY" => return Ok(connection),
            Some((CLOSE, payload)) => {
                return Err(format!(
                    "Discordが接続を拒否しました（Application IDを確認してください）: {}",
                    text(&payload, "message")
                ))
            }
            Some((PING, payload)) => connection.send(PONG, &payload).map_err(lost)?,
            Some(_) => {}
            None => std::thread::sleep(POLL),
        }
    }
    Err("Discordから応答がありません".into())
}

#[derive(Clone, Default)]
struct Wanted {
    client_id: String,
    activity: Option<Activity>,
    revision: u64,
}

struct Shared {
    stop: AtomicBool,
    wanted: Mutex<Wanted>,
    status: Mutex<String>,
}

impl Shared {
    fn report(&self, text: impl Into<String>) {
        *self.status.lock().unwrap_or_else(|e| e.into_inner()) = text.into();
    }
}

/// The background connection. Dropping it clears the activity, closes the
/// pipe and joins the thread.
pub(super) struct Worker {
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
}

impl Worker {
    pub(super) fn start(prefix: &str) -> io::Result<Self> {
        let shared = Arc::new(Shared {
            stop: AtomicBool::new(false),
            wanted: Mutex::new(Wanted::default()),
            status: Mutex::new("接続を準備中".into()),
        });
        let thread = {
            let shared = Arc::clone(&shared);
            let prefix = prefix.to_string();
            std::thread::Builder::new()
                .name("discord-rpc".into())
                .spawn(move || run(&shared, &prefix))?
        };
        Ok(Self {
            shared,
            thread: Some(thread),
        })
    }

    /// The application to connect as and the activity to show (`None`:
    /// connected but nothing shown). Sent once it differs from the last.
    pub(super) fn set(&self, client_id: &str, activity: Option<Activity>) {
        let mut wanted = self.shared.wanted.lock().unwrap_or_else(|e| e.into_inner());
        if wanted.client_id != client_id || wanted.activity != activity {
            wanted.client_id = client_id.to_string();
            wanted.activity = activity;
            wanted.revision += 1;
        }
    }

    pub(super) fn status(&self) -> String {
        self.shared
            .status
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn run(shared: &Shared, prefix: &str) {
    let pid = std::process::id();
    let mut connection: Option<Connection> = None;
    let mut sent: Option<u64> = None;
    let mut last_send: Option<Instant> = None;
    let mut retry_at = Instant::now();
    let mut nonce = 0u64;
    while !shared.stop.load(Ordering::Acquire) {
        let wanted = shared
            .wanted
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        // Another application: connect again as it.
        if connection
            .as_ref()
            .is_some_and(|c| c.client_id != wanted.client_id)
        {
            connection = None;
            retry_at = Instant::now();
        }
        if connection.is_none() {
            sent = None;
            if wanted.client_id.is_empty() {
                shared.report("Application IDが未設定です");
            } else if Instant::now() >= retry_at {
                match connect(prefix, &wanted.client_id, &shared.stop) {
                    Ok(c) => {
                        connection = Some(c);
                        shared.report("Discordに接続しました");
                    }
                    Err(e) => {
                        shared.report(e);
                        retry_at = Instant::now() + RETRY;
                    }
                }
            }
        }
        if let Some(c) = connection.as_mut() {
            let result = (|| -> Result<(), String> {
                while let Some((op, payload)) = c.poll().map_err(|e| e.to_string())? {
                    match op {
                        PING => c.send(PONG, &payload).map_err(|e| e.to_string())?,
                        CLOSE => return Err(text(&payload, "message")),
                        FRAME if payload["evt"] == "ERROR" => shared.report(format!(
                            "Discordがエラーを返しました: {}",
                            payload["data"]["message"].as_str().unwrap_or_default()
                        )),
                        _ => {}
                    }
                }
                let due = last_send.is_none_or(|t| t.elapsed() >= MIN_UPDATE_GAP);
                if sent != Some(wanted.revision) && due {
                    nonce += 1;
                    c.send(FRAME, &set_activity(pid, nonce, wanted.activity.as_ref()))
                        .map_err(|e| e.to_string())?;
                    sent = Some(wanted.revision);
                    last_send = Some(Instant::now());
                    shared.report(if wanted.activity.is_some() {
                        "Discordに表示中"
                    } else {
                        "Discordに接続中（表示なし）"
                    });
                }
                Ok(())
            })();
            if let Err(e) = result {
                connection = None;
                shared.report(format!("Discordとの接続が切れました（再接続します）: {e}"));
                retry_at = Instant::now() + RETRY;
            }
        }
        std::thread::sleep(POLL);
    }
    // Leave the profile clean (Discord also clears it when the pipe closes).
    if let Some(c) = connection.as_mut() {
        let _ = c.send(FRAME, &set_activity(pid, nonce + 1, None));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::windows::io::FromRawHandle;
    use windows_sys::Win32::Foundation::{
        GetLastError, ERROR_PIPE_CONNECTED, INVALID_HANDLE_VALUE,
    };
    use windows_sys::Win32::Storage::FileSystem::PIPE_ACCESS_DUPLEX;
    use windows_sys::Win32::System::Pipes::{
        ConnectNamedPipe, CreateNamedPipeW, PIPE_READMODE_BYTE, PIPE_TYPE_BYTE, PIPE_WAIT,
    };

    fn activity() -> Activity {
        Activity {
            details: "シングルプレイ".into(),
            state: Some("ネザー".into()),
            start: Some(1_700_000_000),
            large_image: Some("logo".into()),
            large_text: Some("Minecraft Bedrock Edition".into()),
        }
    }

    #[test]
    fn frames_carry_the_opcode_and_length_before_the_json() {
        let frame = encode(FRAME, &json!({ "a": 1 }));
        assert_eq!(&frame[..4], &1u32.to_le_bytes());
        assert_eq!(&frame[4..8], &7u32.to_le_bytes());
        assert_eq!(&frame[8..], br#"{"a":1}"#);
    }

    #[test]
    fn activities_follow_discords_field_rules() {
        let value = set_activity(42, 7, Some(&activity()));
        assert_eq!(value["cmd"], "SET_ACTIVITY");
        assert_eq!(value["nonce"], "7");
        assert_eq!(value["args"]["pid"], 42);
        let a = &value["args"]["activity"];
        assert_eq!(a["details"], "シングルプレイ");
        assert_eq!(a["state"], "ネザー");
        assert_eq!(a["timestamps"]["start"], 1_700_000_000u64);
        assert_eq!(a["assets"]["large_image"], "logo");
        assert_eq!(a["assets"]["large_text"], "Minecraft Bedrock Edition");
        // Texts of one character are left out, long ones cut to 128.
        let short = Activity {
            state: Some("x".into()),
            details: "あ".repeat(200),
            start: None,
            large_image: Some("  ".into()),
            large_text: Some("hover".into()),
        };
        let a = short.to_json();
        assert_eq!(a["details"].as_str().unwrap().chars().count(), 128);
        assert!(a.get("state").is_none());
        assert!(a.get("timestamps").is_none());
        // No image: no assets (a hover text alone shows nothing).
        assert!(a.get("assets").is_none());
        // Clearing sends a null activity.
        assert!(set_activity(1, 1, None)["args"]["activity"].is_null());
    }

    /// A pipe server standing in for Discord, at `name`.
    fn fake_discord(name: &str) -> File {
        let wide: Vec<u16> = name.encode_utf16().chain([0]).collect();
        // SAFETY: a valid NUL-terminated name; the handle is owned by the File.
        unsafe {
            let handle = CreateNamedPipeW(
                wide.as_ptr(),
                PIPE_ACCESS_DUPLEX,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT,
                1,
                65536,
                65536,
                0,
                std::ptr::null(),
            );
            assert_ne!(handle, INVALID_HANDLE_VALUE);
            File::from_raw_handle(handle)
        }
    }

    fn accept(server: &File) {
        // SAFETY: the handle is the server end created above.
        let ok = unsafe { ConnectNamedPipe(server.as_raw_handle(), std::ptr::null_mut()) };
        assert!(ok != 0 || unsafe { GetLastError() } == ERROR_PIPE_CONNECTED);
    }

    fn read_frame(server: &mut File) -> (u32, Value) {
        let mut header = [0u8; 8];
        server.read_exact(&mut header).unwrap();
        let len = u32::from_le_bytes(header[4..].try_into().unwrap()) as usize;
        let mut body = vec![0; len];
        server.read_exact(&mut body).unwrap();
        (
            u32::from_le_bytes(header[..4].try_into().unwrap()),
            serde_json::from_slice(&body).unwrap(),
        )
    }

    fn prefix(test: &str) -> String {
        format!(r"\\.\pipe\mio-test-discord-{test}-{}-", std::process::id())
    }

    fn wait_for(worker: &Worker, part: &str) -> String {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let status = worker.status();
            if status.contains(part) || Instant::now() > deadline {
                return status;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn handshake_activity_and_clearing_on_stop_reach_the_pipe() {
        let prefix = prefix("flow");
        let mut server = fake_discord(&format!("{prefix}0"));
        let worker = Worker::start(&prefix).unwrap();
        worker.set("123456789012345678", Some(activity()));
        accept(&server);
        let (op, hello) = read_frame(&mut server);
        assert_eq!(op, HANDSHAKE);
        assert_eq!(hello, json!({ "v": 1, "client_id": "123456789012345678" }));
        server
            .write_all(&encode(
                FRAME,
                &json!({ "cmd": "DISPATCH", "evt": "READY" }),
            ))
            .unwrap();
        let (op, command) = read_frame(&mut server);
        assert_eq!(op, FRAME);
        assert_eq!(command["cmd"], "SET_ACTIVITY");
        assert_eq!(command["args"]["pid"], std::process::id());
        assert_eq!(command["args"]["activity"]["details"], "シングルプレイ");
        // Discord's ping is answered with the same payload.
        server.write_all(&encode(PING, &json!({ "n": 5 }))).unwrap();
        assert_eq!(read_frame(&mut server), (PONG, json!({ "n": 5 })));
        assert_eq!(wait_for(&worker, "表示中"), "Discordに表示中");
        drop(worker);
        let (op, clear) = read_frame(&mut server);
        assert_eq!(op, FRAME);
        assert!(clear["args"]["activity"].is_null());
    }

    #[test]
    fn a_rejected_application_id_and_a_missing_discord_are_reported() {
        let prefix = prefix("reject");
        let mut server = fake_discord(&format!("{prefix}0"));
        let worker = Worker::start(&prefix).unwrap();
        worker.set("1", None);
        accept(&server);
        read_frame(&mut server);
        server
            .write_all(&encode(
                CLOSE,
                &json!({ "code": 4000, "message": "Invalid Client ID" }),
            ))
            .unwrap();
        let status = wait_for(&worker, "Invalid Client ID");
        assert!(
            status.contains("拒否") && status.contains("Invalid Client ID"),
            "{status}"
        );
        drop(worker);

        let worker = Worker::start(&self::prefix("absent")).unwrap();
        worker.set("1", None);
        assert!(wait_for(&worker, "起動していません").contains("起動していません"));
        // Stopping does not wait for the 15-second retry.
        let start = Instant::now();
        drop(worker);
        assert!(start.elapsed() < Duration::from_secs(2));
    }
}
