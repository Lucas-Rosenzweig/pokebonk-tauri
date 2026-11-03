//! Discord Rich Presence: «Jugando a PokéBonk» on the player's profile, with what the game tells us.
//!
//! Discord's local IPC, as in the Electron client: a Unix socket `discord-ipc-N`, frames of an 8-byte header
//! (opcode, length, little-endian) and JSON. Nothing goes over the network from here.
//! Discord may be closed or opened after the game: we retry every 20 s while the option is on.
//! Discord takes about 5 updates per 20 s; we send at most one every 4 s (the last one wins).
//!
//! Windows (named pipe) is not done yet: there the presence stays off.

use serde_json::{json, Value};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

pub const DEFAULT_APP_ID: &str = "1559293304611410001";
const REMOTE: &str = "https://raw.githubusercontent.com/CharcherGit/pokebonk-app/main/client.json";

const OP_HANDSHAKE: i32 = 0;
const OP_FRAME: i32 = 1;
const OP_CLOSE: i32 = 2;
const OP_PING: i32 = 3;
const OP_PONG: i32 = 4;

const RETRY: Duration = Duration::from_secs(20);
const MIN_GAP: Duration = Duration::from_secs(4);

enum Msg {
    Set(Option<Value>),
    Enable(bool),
    UseApp(String),
    /// A frame read from the connection of generation `gen`.
    Frame(u64, i32, Value),
    Closed(u64),
}

#[derive(Clone)]
pub struct Presence {
    tx: Sender<Msg>,
}

impl Presence {
    pub fn new(app_id: String) -> Self {
        let (tx, rx) = mpsc::channel();
        let me = Self { tx: tx.clone() };
        std::thread::Builder::new()
            .name("discord-rpc".into())
            .spawn(move || Worker::new(app_id, tx).run(rx))
            .expect("spawn discord thread");
        me
    }
    /// The activity in Discord's shape (see `lib.rs`, `discord_activity`); None clears it.
    pub fn set(&self, activity: Option<Value>) {
        let _ = self.tx.send(Msg::Set(activity));
    }
    pub fn enable(&self, on: bool) {
        let _ = self.tx.send(Msg::Enable(on));
    }
    pub fn use_app(&self, id: String) {
        let _ = self.tx.send(Msg::UseApp(id));
    }
}

/// `{ "discordAppId": "…" }` from the Electron client's downloads repo, so the name can change without a new client.
pub fn remote_app_id() -> Option<String> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(6)))
        .build()
        .into();
    let text = agent.get(REMOTE).call().ok()?.body_mut().read_to_string().ok()?;
    let id = serde_json::from_str::<Value>(&text).ok()?.get("discordAppId")?.as_str()?.to_string();
    (id.len() >= 17 && id.len() <= 20 && id.bytes().all(|b| b.is_ascii_digit())).then_some(id)
}

#[cfg(unix)]
type Conn = std::os::unix::net::UnixStream;

struct Worker {
    app_id: String,
    tx: Sender<Msg>,
    on: bool,
    want: Option<Value>,
    dirty: bool,
    #[cfg(unix)]
    conn: Option<Conn>,
    gen: u64,
    ready: bool,
    last_sent: Option<Instant>,
    next_try: Option<Instant>,
}

impl Worker {
    fn new(app_id: String, tx: Sender<Msg>) -> Self {
        Self {
            app_id,
            tx,
            on: false,
            want: None,
            dirty: false,
            #[cfg(unix)]
            conn: None,
            gen: 0,
            ready: false,
            last_sent: None,
            next_try: None,
        }
    }

    fn run(mut self, rx: Receiver<Msg>) {
        loop {
            match rx.recv_timeout(Duration::from_millis(250)) {
                Ok(m) => self.handle(m),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return,
            }
            self.tick();
        }
    }

    fn handle(&mut self, m: Msg) {
        match m {
            Msg::Set(a) => {
                self.want = a;
                self.dirty = true;
            }
            Msg::Enable(on) if on != self.on => {
                self.on = on;
                if on {
                    self.next_try = Some(Instant::now());
                } else {
                    if self.ready {
                        self.write_activity(None);
                    }
                    self.close(false);
                }
            }
            Msg::Enable(_) => {}
            Msg::UseApp(id) if id != self.app_id => {
                self.app_id = id;
                if self.on {
                    self.close(false);
                    self.next_try = Some(Instant::now());
                }
            }
            Msg::UseApp(_) => {}
            Msg::Frame(gen, op, msg) if gen == self.gen => match op {
                OP_PING => self.send(OP_PONG, &msg),
                OP_CLOSE => self.close(true),
                OP_FRAME if msg.get("evt").and_then(Value::as_str) == Some("READY") => {
                    self.ready = true;
                    self.last_sent = None;
                    self.dirty = true;
                }
                _ => {}
            },
            Msg::Closed(gen) if gen == self.gen => self.close(true),
            _ => {}
        }
    }

    fn tick(&mut self) {
        if !self.on {
            return;
        }
        if !self.connected() && self.next_try.is_some_and(|t| Instant::now() >= t) {
            self.connect();
        }
        if self.ready && self.dirty && self.last_sent.is_none_or(|t| t.elapsed() >= MIN_GAP) {
            self.dirty = false;
            self.write_activity(self.want.clone());
        }
    }

    fn write_activity(&mut self, activity: Option<Value>) {
        self.last_sent = Some(Instant::now());
        let nonce = format!("{}-{}", now_ms(), rand::random::<u32>());
        let payload = json!({
            "cmd": "SET_ACTIVITY",
            "args": { "pid": std::process::id(), "activity": activity },
            "nonce": nonce,
        });
        self.send(OP_FRAME, &payload);
    }

    fn close(&mut self, again: bool) {
        self.ready = false;
        self.gen += 1;
        #[cfg(unix)]
        if let Some(c) = self.conn.take() {
            let _ = c.shutdown(std::net::Shutdown::Both);
        }
        self.next_try = if again && self.on { Some(Instant::now() + RETRY) } else { None };
    }

    #[cfg(unix)]
    fn connected(&self) -> bool {
        self.conn.is_some()
    }
    #[cfg(not(unix))]
    fn connected(&self) -> bool {
        false
    }

    #[cfg(unix)]
    fn send(&mut self, op: i32, data: &Value) {
        use std::io::Write;
        let Some(c) = self.conn.as_mut() else { return };
        if c.write_all(&frame(op, data)).is_err() {
            self.close(true);
        }
    }
    #[cfg(not(unix))]
    fn send(&mut self, _op: i32, _data: &Value) {}

    #[cfg(unix)]
    fn connect(&mut self) {
        self.next_try = Some(Instant::now() + RETRY);
        let Some(stream) = socket_paths().into_iter().find_map(|p| Conn::connect(p).ok()) else { return };
        let Ok(reader) = stream.try_clone() else { return };
        self.gen += 1;
        let (gen, tx) = (self.gen, self.tx.clone());
        std::thread::spawn(move || read_frames(reader, gen, tx));
        self.conn = Some(stream);
        self.next_try = None;
        let hello = json!({ "v": 1, "client_id": self.app_id });
        self.send(OP_HANDSHAKE, &hello);
    }
    #[cfg(not(unix))]
    fn connect(&mut self) {
        self.next_try = None;
    }
}

#[cfg(unix)]
fn read_frames(mut s: Conn, gen: u64, tx: Sender<Msg>) {
    use std::io::Read;
    let mut head = [0u8; 8];
    loop {
        if s.read_exact(&mut head).is_err() {
            break;
        }
        let op = i32::from_le_bytes(head[0..4].try_into().unwrap());
        let len = i32::from_le_bytes(head[4..8].try_into().unwrap()).clamp(0, 1 << 20) as usize;
        let mut body = vec![0u8; len];
        if s.read_exact(&mut body).is_err() {
            break;
        }
        let msg = serde_json::from_slice(&body).unwrap_or(Value::Null);
        if std::env::var_os("POKEBONK_RPC_DEBUG").is_some() {
            eprintln!("[discord] {}", msg.to_string().chars().take(400).collect::<String>());
        }
        if tx.send(Msg::Frame(gen, op, msg)).is_err() {
            return;
        }
    }
    let _ = tx.send(Msg::Closed(gen));
}

fn frame(op: i32, data: &Value) -> Vec<u8> {
    let body = data.to_string().into_bytes();
    let mut out = Vec::with_capacity(8 + body.len());
    out.extend_from_slice(&op.to_le_bytes());
    out.extend_from_slice(&(body.len() as i32).to_le_bytes());
    out.extend_from_slice(&body);
    out
}

/// Where Discord listens: the temp folders, and Flatpak/Snap folders on Linux (Steam Deck: Flatpak).
#[cfg(unix)]
fn socket_paths() -> Vec<std::path::PathBuf> {
    let mut bases: Vec<std::path::PathBuf> = ["XDG_RUNTIME_DIR", "TMPDIR", "TMP", "TEMP"]
        .iter()
        .filter_map(|k| std::env::var_os(k).map(Into::into))
        .collect();
    bases.push("/tmp".into());
    let subs = [
        "",
        "app/com.discordapp.Discord",
        "app/com.discordapp.DiscordCanary",
        "snap.discord",
        "snap.discord-canary",
        ".flatpak/dev.vencord.Vesktop/xdg-run",
    ];
    let mut out = Vec::new();
    for i in 0..10 {
        for b in &bases {
            for s in subs {
                let p = b.join(s).join(format!("discord-ipc-{i}"));
                if !out.contains(&p) {
                    out.push(p);
                }
            }
        }
    }
    out
}

pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}
