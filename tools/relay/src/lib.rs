//! WebSocket relay for Motocross Madness 2 network play (the "LAN party").
//!
//! The game's own network code (`net.rs` in the engine) is a star: one host, and
//! every other player keeps one connection to the host. A browser cannot listen
//! for connections or broadcast, so this relay stands in for the LAN. A room is a
//! host plus up to seven guests, found by a short room code. The relay only
//! passes frames; it does not look inside them.
//!
//! Control messages are JSON text frames, game frames are binary:
//!
//!   client -> relay   `{"t":"host","name":"..","public":true,"info":"..","parked":false}`
//!   relay  -> host    `{"t":"hosted","code":"K7QM2","max":8,"token":"..","entered":true,"ttl":300}`
//!   client -> relay   `{"t":"join","code":"K7QM2"}`
//!   relay  -> guest   `{"t":"joined","code":"K7QM2","id":3,"name":"..","info":".."}`
//!   relay  -> host    `{"t":"peer","id":3,"up":true}`  (and `up:false` when they go)
//!   host   -> relay   `{"t":"info","info":"..","public":true,"name":".."}`
//!   relay  -> client  `{"t":"error","reason":"no_room|not_open|full|busy|rate|bad_request"}`
//!
//! A room is open (joinable, listed) only while its host is in the game's lobby. A room made
//! on the start page is "parked": it exists, but has no players, cannot be joined and is not
//! listed, until the host says so:
//!
//!   host   -> relay   `{"t":"enter"}`   the host is in the game's lobby: the room opens
//!   host   -> relay   `{"t":"park"}`    the host left the lobby: guests are sent out, the room
//!                                       closes to others but is kept
//!   host   -> relay   `{"t":"close"}`   delete the room
//!   client -> relay   `{"t":"resume","code":"K7QM2","token":".."}`   a host coming back (the page
//!                                       was reloaded, or the game was left) takes the room again
//!
//! Binary game frames:
//!
//!   guest -> relay -> host   payload            arrives at the host as: `id` byte + payload
//!   host  -> relay -> guest  `id` byte + payload  arrives at that guest as: payload
//!                            (`id = 255` goes to every guest; `id = 0x80 | k` goes to every
//!                            guest except `k`)

use bytes::{Bytes, BytesMut};
use futures_util::{SinkExt, StreamExt};
use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, oneshot};
use tokio_tungstenite::tungstenite::handshake::derive_accept_key;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tokio_tungstenite::tungstenite::protocol::{CloseFrame, Role, WebSocketConfig};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::WebSocketStream;

/// The game's lobby holds eight names.
pub const MAX_PLAYERS: usize = 8;
/// Host -> relay: send to every guest.
pub const ALL: u8 = 255;
/// Host -> relay: `ALL_EXCEPT | id` (`0x81..=0x87`) sends to every guest except `id`.
pub const ALL_EXCEPT: u8 = 0x80;
const CODE_ALPHABET: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789"; // no O/0/I/1
const CODE_LEN: usize = 5;
pub const MAX_FRAME: usize = 64 * 1024;

pub const CLOSE_HOST_LEFT: u16 = 4000;
pub const CLOSE_FULL: u16 = 4001;
pub const CLOSE_REPLACED: u16 = 4002;
pub const CLOSE_EXPIRED: u16 = 4003;

/// Per-client cap on queued binary frames so a stalled socket cannot bloat memory.
const MAX_QUEUED_BINARY_FRAMES: usize = 512;

/// Caps that keep an open relay from being used up. Tests shrink them.
#[derive(Debug, Clone)]
pub struct Limits {
    pub rooms: usize,
    pub connections: usize,
    pub connections_per_ip: usize,
    pub rooms_per_ip: usize,
    pub park_seconds: f64,
    pub bad_joins_per_minute: usize,
    pub first_message_seconds: f64,
    pub host_rate: (f64, f64),
    pub host_bytes: (f64, f64),
    pub guest_rate: (f64, f64),
    pub guest_bytes: (f64, f64),
}

impl Default for Limits {
    fn default() -> Self {
        let park_seconds = std::env::var("PARK_SECONDS")
            .ok()
            .and_then(|s| s.parse::<f64>().ok())
            .unwrap_or(300.0);
        Self {
            rooms: 500,
            connections: 2000,
            connections_per_ip: 8,
            rooms_per_ip: 3,
            park_seconds,
            bad_joins_per_minute: 12,
            first_message_seconds: 10.0,
            host_rate: (4000.0, 8000.0),
            host_bytes: (1_000_000.0, 2_000_000.0),
            guest_rate: (300.0, 600.0),
            guest_bytes: (128_000.0, 256_000.0),
        }
    }
}

struct Bucket {
    rate: f64,
    burst: f64,
    level: f64,
    at: Instant,
}

impl Bucket {
    fn new(rate: f64, burst: f64) -> Self {
        Self {
            rate,
            burst,
            level: burst,
            at: Instant::now(),
        }
    }

    fn take(&mut self, n: f64) -> bool {
        let now = Instant::now();
        let elapsed = now.duration_since(self.at).as_secs_f64();
        self.level = (self.level + elapsed * self.rate).min(self.burst);
        self.at = now;
        if self.level < n {
            false
        } else {
            self.level -= n;
            true
        }
    }
}

#[derive(Clone)]
pub struct ClientHandle {
    pub conn_id: u64,
    pub ip: String,
    ctrl_tx: mpsc::UnboundedSender<Message>,
    bin_tx: mpsc::Sender<Message>,
}

impl ClientHandle {
    fn send_text(&self, text: String) {
        let _ = self.ctrl_tx.send(Message::Text(text.into()));
    }

    fn send_binary(&self, data: Bytes) {
        // Non-blocking send: if a client's TCP socket is completely stalled, drop the
        // excess binary frame rather than blocking the relay or growing memory unboundedly.
        let _ = self.bin_tx.try_send(Message::Binary(data));
    }

    fn close(&self, code: u16, reason: &'static str) {
        let frame = CloseFrame {
            code: CloseCode::from(code),
            reason: reason.into(),
        };
        let _ = self.ctrl_tx.send(Message::Close(Some(frame)));
    }
}

pub struct Room {
    pub code: String,
    pub host: Option<ClientHandle>,
    pub name: String,
    pub public: bool,
    pub info: String,
    pub token: String,
    pub ip: String,
    pub guests: HashMap<u8, ClientHandle>,
    pub created: f64,
    pub entered: bool,
    pub parked_since: Option<f64>,
}

impl Room {
    /// Joinable and listed: the host is connected and in the lobby.
    pub fn open(&self) -> bool {
        self.host.is_some() && self.entered
    }

    pub fn players(&self) -> usize {
        1 + self.guests.len()
    }

    fn free_id(&self) -> Option<u8> {
        (1..MAX_PLAYERS as u8).find(|i| !self.guests.contains_key(i))
    }
}

struct ConnInfo {
    ip: String,
    room: Option<String>,
    id: u8,
    frames: Option<Bucket>,
    bytes: Option<Bucket>,
}

impl ConnInfo {
    fn set_role(&mut self, host: bool, limits: &Limits) {
        let (fr, fb) = if host { limits.host_rate } else { limits.guest_rate };
        let (br, bb) = if host { limits.host_bytes } else { limits.guest_bytes };
        self.frames = Some(Bucket::new(fr, fb));
        self.bytes = Some(Bucket::new(br, bb));
    }

    fn allow(&mut self, size: usize) -> bool {
        match (&mut self.frames, &mut self.bytes) {
            (Some(f), Some(b)) => f.take(1.0) && b.take(size as f64),
            _ => true,
        }
    }
}

pub struct RelayState {
    pub rooms: HashMap<String, Room>,
    conns: HashMap<u64, ConnInfo>,
    pub per_ip: HashMap<String, usize>,
    pub bad_joins: HashMap<String, (usize, f64)>,
    pub limits: Limits,
    pub trust_proxy: bool,
    pub allowed_origins: HashSet<String>,
}

pub struct Relay {
    state: Mutex<RelayState>,
    next_conn_id: AtomicU64,
}

pub fn now_secs() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}

fn random_token_urlsafe_16() -> String {
    const URLSAFE: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut raw = [0u8; 16];
    let _ = getrandom::fill(&mut raw);
    // Base64url-encode 16 bytes -> 22 chars without padding.
    let mut out = String::with_capacity(22);
    let mut i = 0;
    while i + 3 <= 16 {
        let n = ((raw[i] as u32) << 16) | ((raw[i + 1] as u32) << 8) | (raw[i + 2] as u32);
        out.push(URLSAFE[((n >> 18) & 63) as usize] as char);
        out.push(URLSAFE[((n >> 12) & 63) as usize] as char);
        out.push(URLSAFE[((n >> 6) & 63) as usize] as char);
        out.push(URLSAFE[(n & 63) as usize] as char);
        i += 3;
    }
    // Remaining 1 byte (raw[15])
    let n = (raw[15] as u32) << 16;
    out.push(URLSAFE[((n >> 18) & 63) as usize] as char);
    out.push(URLSAFE[((n >> 12) & 63) as usize] as char);
    out
}

fn constant_time_eq(a: &str, b: &str) -> bool {
    let ab = a.as_bytes();
    let bb = b.as_bytes();
    if ab.len() != bb.len() {
        return false;
    }
    let mut diff = 0u8;
    for (&x, &y) in ab.iter().zip(bb.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

impl Relay {
    pub fn new(trust_proxy: bool, allowed_origins: impl IntoIterator<Item = String>) -> Arc<Self> {
        let origins = allowed_origins
            .into_iter()
            .map(|o| o.trim().trim_end_matches('/').to_ascii_lowercase())
            .filter(|o| !o.is_empty())
            .collect();
        Arc::new(Self {
            state: Mutex::new(RelayState {
                rooms: HashMap::new(),
                conns: HashMap::new(),
                per_ip: HashMap::new(),
                bad_joins: HashMap::new(),
                limits: Limits::default(),
                trust_proxy,
                allowed_origins: origins,
            }),
            next_conn_id: AtomicU64::new(1),
        })
    }

    pub fn with_state<R>(&self, f: impl FnOnce(&mut RelayState) -> R) -> R {
        let mut guard = self.state.lock().unwrap();
        f(&mut guard)
    }

    /// A name or note from a client: printable characters only, shortened.
    pub fn clean_text(value: &str, limit: usize) -> String {
        let cleaned: String = value
            .chars()
            .filter(|&ch| !ch.is_control())
            .collect::<String>();
        let trimmed = cleaned.trim();
        trimmed.chars().take(limit).collect()
    }

    pub fn normal_code(value: &str) -> String {
        value
            .chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .map(|c| c.to_ascii_uppercase())
            .collect()
    }

    pub fn origin_ok(&self, origin_hdr: Option<&str>, host_hdr: Option<&str>) -> bool {
        let Some(origin) = origin_hdr else {
            return true; // not a browser; a page cannot omit it
        };
        let origin = origin.trim().trim_end_matches('/').to_ascii_lowercase();
        let netloc = origin
            .split_once("://")
            .map(|(_, rest)| rest.split('/').next().unwrap_or(rest))
            .unwrap_or("");
        let hostname = if netloc.starts_with('[') {
            netloc
                .find(']')
                .map(|i| &netloc[..=i])
                .unwrap_or(netloc)
        } else {
            netloc.split(':').next().unwrap_or(netloc)
        };
        if matches!(hostname, "localhost" | "127.0.0.1" | "[::1]" | "::1") {
            return true;
        }
        let st = self.state.lock().unwrap();
        if st.allowed_origins.contains(&origin) {
            return true;
        }
        let host = host_hdr.unwrap_or("").trim().to_ascii_lowercase();
        !netloc.is_empty() && netloc == host
    }

    pub fn stats_json(&self) -> String {
        let st = self.state.lock().unwrap();
        let mut public_rooms = 0usize;
        let mut public_players = 0usize;
        let mut rooms = 0usize;
        let mut players = 0usize;
        for r in st.rooms.values() {
            if r.open() {
                rooms += 1;
                let p = r.players();
                players += p;
                if r.public {
                    public_rooms += 1;
                    public_players += p;
                }
            }
        }
        format!(
            "{{\"public_rooms\":{public_rooms},\"public_players\":{public_players},\"rooms\":{rooms},\"players\":{players}}}"
        )
    }

    pub fn listing_json(&self) -> String {
        let now = now_secs();
        let st = self.state.lock().unwrap();
        let mut open_public: Vec<&Room> = st
            .rooms
            .values()
            .filter(|r| r.public && r.open())
            .collect();
        open_public.sort_by(|a, b| a.created.partial_cmp(&b.created).unwrap_or(std::cmp::Ordering::Equal));
        let items: Vec<String> = open_public
            .into_iter()
            .map(|r| {
                let age = (now - r.created).max(0.0) as i64;
                format!(
                    "{{\"code\":{},\"name\":{},\"players\":{},\"max\":{},\"info\":{},\"age\":{age}}}",
                    json_str(&r.code),
                    json_str(&r.name),
                    r.players(),
                    MAX_PLAYERS,
                    json_str(&r.info),
                )
            })
            .collect();
        format!("{{\"rooms\":[{}]}}", items.join(","))
    }

    /// Deletes the rooms whose host has not been in the lobby for `park_seconds`.
    pub fn sweep(&self, now: Option<f64>) {
        let now = now.unwrap_or_else(now_secs);
        let mut st = self.state.lock().unwrap();
        let park_seconds = st.limits.park_seconds;
        let expired_codes: Vec<String> = st
            .rooms
            .values()
            .filter(|r| {
                !r.entered
                    && r.parked_since
                        .is_some_and(|since| now - since > park_seconds)
            })
            .map(|r| r.code.clone())
            .collect();
        for code in expired_codes {
            let host = st.rooms.get(&code).and_then(|r| r.host.clone());
            Self::delete_room_locked(&mut st, &code);
            if let Some(h) = host {
                h.close(CLOSE_EXPIRED, "expired");
            }
        }
    }

    fn delete_room_locked(st: &mut RelayState, code: &str) {
        let Some(mut room) = st.rooms.remove(code) else {
            return;
        };
        Self::kick_guests_locked(st, &mut room);
        if let Some(host) = room.host.take() {
            if let Some(ci) = st.conns.get_mut(&host.conn_id) {
                ci.room = None;
            }
        }
    }

    fn kick_guests_locked(st: &mut RelayState, room: &mut Room) {
        for (_, guest) in room.guests.drain() {
            if let Some(ci) = st.conns.get_mut(&guest.conn_id) {
                ci.room = None;
            }
            guest.close(CLOSE_HOST_LEFT, "host_left");
        }
    }
}

impl RelayState {
    fn new_code(&self) -> Option<String> {
        let mut buf = [0u8; CODE_LEN];
        for _ in 0..50 {
            let _ = getrandom::fill(&mut buf);
            let code: String = buf
                .iter()
                .map(|&b| CODE_ALPHABET[(b as usize) % CODE_ALPHABET.len()] as char)
                .collect();
            if !self.rooms.contains_key(&code) {
                return Some(code);
            }
        }
        None
    }

    fn too_many_bad_joins(&mut self, ip: &str) -> bool {
        let now = now_secs();
        let (mut count, mut start) = self.bad_joins.get(ip).copied().unwrap_or((0, now));
        if now - start > 60.0 {
            count = 0;
            start = now;
        }
        self.bad_joins.insert(ip.to_string(), (count, start));
        count >= self.limits.bad_joins_per_minute
    }

    fn note_bad_join(&mut self, ip: &str) {
        let now = now_secs();
        let (count, start) = self.bad_joins.get(ip).copied().unwrap_or((0, now));
        self.bad_joins.insert(ip.to_string(), (count + 1, start));
    }

    fn hosted_reply(&self, room: &Room) -> String {
        let ttl = self.limits.park_seconds as i64;
        format!(
            "{{\"t\":\"hosted\",\"code\":{},\"max\":{},\"token\":{},\"entered\":{},\"name\":{},\"public\":{},\"ttl\":{ttl}}}",
            json_str(&room.code),
            MAX_PLAYERS,
            json_str(&room.token),
            room.entered,
            json_str(&room.name),
            room.public
        )
    }
}

fn error_json(reason: &str) -> String {
    format!("{{\"t\":\"error\",\"reason\":{}}}", json_str(reason))
}

fn json_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

// ---- Minimal RFC 8259 JSON object parser for control frames -------------------------

#[derive(Debug, Clone, PartialEq)]
enum JsonVal {
    Str(String),
    Bool(bool),
    Num(f64),
    Null,
    Other,
}

impl JsonVal {
    fn as_str_lossy(&self) -> String {
        match self {
            JsonVal::Str(s) => s.clone(),
            JsonVal::Bool(b) => b.to_string(),
            JsonVal::Num(n) => {
                if n.fract() == 0.0 {
                    (*n as i64).to_string()
                } else {
                    n.to_string()
                }
            }
            JsonVal::Null => String::new(),
            JsonVal::Other => String::new(),
        }
    }

    fn is_truthy(&self) -> bool {
        match self {
            JsonVal::Bool(b) => *b,
            JsonVal::Str(s) => !s.is_empty(),
            JsonVal::Num(n) => *n != 0.0,
            JsonVal::Null | JsonVal::Other => false,
        }
    }
}

struct JsonParser<'a> {
    s: &'a [u8],
    pos: usize,
}

impl<'a> JsonParser<'a> {
    fn new(input: &'a str) -> Self {
        Self {
            s: input.as_bytes(),
            pos: 0,
        }
    }

    fn skip_ws(&mut self) {
        while self.pos < self.s.len() && matches!(self.s[self.pos], b' ' | b'\t' | b'\r' | b'\n') {
            self.pos += 1;
        }
    }

    fn peek(&mut self) -> Option<u8> {
        self.skip_ws();
        self.s.get(self.pos).copied()
    }

    fn parse_object(&mut self) -> Option<HashMap<String, JsonVal>> {
        self.skip_ws();
        if self.s.get(self.pos) != Some(&b'{') {
            return None;
        }
        self.pos += 1;
        let mut map = HashMap::new();
        if self.peek() == Some(b'}') {
            self.pos += 1;
            self.skip_ws();
            return (self.pos == self.s.len()).then_some(map);
        }
        loop {
            self.skip_ws();
            let key = self.parse_string()?;
            self.skip_ws();
            if self.s.get(self.pos) != Some(&b':') {
                return None;
            }
            self.pos += 1;
            let val = self.parse_value()?;
            map.insert(key, val);
            self.skip_ws();
            match self.s.get(self.pos) {
                Some(b',') => self.pos += 1,
                Some(b'}') => {
                    self.pos += 1;
                    break;
                }
                _ => return None,
            }
        }
        self.skip_ws();
        (self.pos == self.s.len()).then_some(map)
    }

    fn parse_string(&mut self) -> Option<String> {
        self.skip_ws();
        if self.s.get(self.pos) != Some(&b'"') {
            return None;
        }
        self.pos += 1;
        let mut out = String::new();
        while self.pos < self.s.len() {
            let b = self.s[self.pos];
            if b == b'"' {
                self.pos += 1;
                return Some(out);
            } else if b == b'\\' {
                self.pos += 1;
                let esc = *self.s.get(self.pos)?;
                self.pos += 1;
                match esc {
                    b'"' => out.push('"'),
                    b'\\' => out.push('\\'),
                    b'/' => out.push('/'),
                    b'b' => out.push('\u{0008}'),
                    b'f' => out.push('\u{000c}'),
                    b'n' => out.push('\n'),
                    b'r' => out.push('\r'),
                    b't' => out.push('\t'),
                    b'u' => {
                        if self.pos + 4 > self.s.len() {
                            return None;
                        }
                        let hex = std::str::from_utf8(&self.s[self.pos..self.pos + 4]).ok()?;
                        let code = u32::from_str_radix(hex, 16).ok()?;
                        self.pos += 4;
                        if let Some(ch) = char::from_u32(code) {
                            out.push(ch);
                        }
                    }
                    _ => return None,
                }
            } else {
                let rest = std::str::from_utf8(&self.s[self.pos..]).ok()?;
                let ch = rest.chars().next()?;
                out.push(ch);
                self.pos += ch.len_utf8();
            }
        }
        None
    }

    fn parse_value(&mut self) -> Option<JsonVal> {
        let b = self.peek()?;
        match b {
            b'"' => self.parse_string().map(JsonVal::Str),
            b't' if self.s[self.pos..].starts_with(b"true") => {
                self.pos += 4;
                Some(JsonVal::Bool(true))
            }
            b'f' if self.s[self.pos..].starts_with(b"false") => {
                self.pos += 5;
                Some(JsonVal::Bool(false))
            }
            b'n' if self.s[self.pos..].starts_with(b"null") => {
                self.pos += 4;
                Some(JsonVal::Null)
            }
            b'-' | b'0'..=b'9' => {
                let start = self.pos;
                while self.pos < self.s.len()
                    && matches!(self.s[self.pos], b'0'..=b'9' | b'-' | b'+' | b'.' | b'e' | b'E')
                {
                    self.pos += 1;
                }
                let s = std::str::from_utf8(&self.s[start..self.pos]).ok()?;
                let n = s.parse::<f64>().ok()?;
                Some(JsonVal::Num(n))
            }
            b'[' => {
                self.pos += 1;
                if self.peek() == Some(b']') {
                    self.pos += 1;
                    return Some(JsonVal::Other);
                }
                loop {
                    self.parse_value()?;
                    match self.peek()? {
                        b',' => self.pos += 1,
                        b']' => {
                            self.pos += 1;
                            return Some(JsonVal::Other);
                        }
                        _ => return None,
                    }
                }
            }
            b'{' => {
                self.parse_object()?;
                Some(JsonVal::Other)
            }
            _ => None,
        }
    }
}

// ---- Control & binary message handling ----------------------------------------------

impl Relay {
    fn handle_control(&self, client: &ClientHandle, raw: &str) {
        let Some(msg) = JsonParser::new(raw).parse_object() else {
            return client.send_text(error_json("bad_request"));
        };
        let Some(JsonVal::Str(kind)) = msg.get("t") else {
            return client.send_text(error_json("bad_request"));
        };
        let mut st = self.state.lock().unwrap();
        let Some(ci) = st.conns.get(&client.conn_id) else {
            return;
        };
        let in_room = ci.room.clone();
        let cid = ci.id;

        match kind.as_str() {
            "host" if in_room.is_none() => self.cmd_host(&mut st, client, &msg),
            "join" if in_room.is_none() => self.cmd_join(&mut st, client, &msg),
            "resume" if in_room.is_none() => self.cmd_resume(&mut st, client, &msg),
            "enter" if in_room.is_some() && cid == 0 => {
                let code = in_room.unwrap();
                if let Some(room) = st.rooms.get_mut(&code) {
                    room.entered = true;
                    room.parked_since = None;
                }
                client.send_text("{\"t\":\"entered\"}".to_string());
            }
            "park" if in_room.is_some() && cid == 0 => {
                let code = in_room.unwrap();
                let mut guests_to_kick = Vec::new();
                if let Some(room) = st.rooms.get_mut(&code) {
                    guests_to_kick.extend(room.guests.drain().map(|(_, g)| g));
                    room.entered = false;
                    room.parked_since = Some(now_secs());
                }
                for g in guests_to_kick {
                    if let Some(gci) = st.conns.get_mut(&g.conn_id) {
                        gci.room = None;
                    }
                    g.close(CLOSE_HOST_LEFT, "host_left");
                }
                client.send_text("{\"t\":\"parked\"}".to_string());
            }
            "close" if in_room.is_some() && cid == 0 => {
                let code = in_room.unwrap();
                Self::delete_room_locked(&mut st, &code);
                if let Some(ci) = st.conns.get_mut(&client.conn_id) {
                    ci.room = None;
                }
            }
            "info" if in_room.is_some() && cid == 0 => {
                let code = in_room.unwrap();
                if let Some(room) = st.rooms.get_mut(&code) {
                    if let Some(v) = msg.get("info") {
                        room.info = Self::clean_text(&v.as_str_lossy(), 80);
                    }
                    if let Some(v) = msg.get("name") {
                        let cleaned = Self::clean_text(&v.as_str_lossy(), 32);
                        if !cleaned.is_empty() {
                            room.name = cleaned;
                        }
                    }
                    if let Some(v) = msg.get("public") {
                        room.public = v.is_truthy();
                    }
                }
            }
            "ping" => client.send_text("{\"t\":\"pong\"}".to_string()),
            _ => client.send_text(error_json("bad_request")),
        }
    }

    fn cmd_host(&self, st: &mut RelayState, client: &ClientHandle, msg: &HashMap<String, JsonVal>) {
        if st.rooms.len() >= st.limits.rooms {
            return client.send_text(error_json("busy"));
        }
        let mut mine_count = st.rooms.values().filter(|r| r.ip == client.ip).count();
        if mine_count >= st.limits.rooms_per_ip {
            // Rooms this address made whose host is gone only wait for their time to run out:
            // the oldest give way to a new one.
            let mut gone: Vec<(String, f64)> = st
                .rooms
                .values()
                .filter(|r| r.ip == client.ip && r.host.is_none())
                .map(|r| (r.code.clone(), r.parked_since.unwrap_or(0.0)))
                .collect();
            gone.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
            for (code, _) in gone {
                if mine_count < st.limits.rooms_per_ip {
                    break;
                }
                Self::delete_room_locked(st, &code);
                mine_count -= 1;
            }
        }
        if mine_count >= st.limits.rooms_per_ip {
            return client.send_text(error_json("busy"));
        }
        let Some(code) = st.new_code() else {
            return client.send_text(error_json("busy"));
        };
        let raw_name = msg.get("name").map(JsonVal::as_str_lossy).unwrap_or_default();
        let cleaned_name = Self::clean_text(&raw_name, 32);
        let name = if cleaned_name.is_empty() {
            "Room".to_string()
        } else {
            cleaned_name
        };
        let public = msg.get("public").is_some_and(JsonVal::is_truthy);
        let raw_info = msg.get("info").map(JsonVal::as_str_lossy).unwrap_or_default();
        let info = Self::clean_text(&raw_info, 80);
        let entered = !msg.get("parked").is_some_and(JsonVal::is_truthy);
        let now = now_secs();
        let room = Room {
            code: code.clone(),
            host: Some(client.clone()),
            name,
            public,
            info,
            token: random_token_urlsafe_16(),
            ip: client.ip.clone(),
            guests: HashMap::new(),
            created: now,
            entered,
            parked_since: (!entered).then_some(now),
        };
        let reply = st.hosted_reply(&room);
        st.rooms.insert(code.clone(), room);
        let limits = st.limits.clone();
        if let Some(ci) = st.conns.get_mut(&client.conn_id) {
            ci.room = Some(code);
            ci.id = 0;
            ci.set_role(true, &limits);
        }
        client.send_text(reply);
    }

    fn cmd_resume(&self, st: &mut RelayState, client: &ClientHandle, msg: &HashMap<String, JsonVal>) {
        if st.too_many_bad_joins(&client.ip) {
            return client.send_text(error_json("rate"));
        }
        let code = Self::normal_code(&msg.get("code").map(JsonVal::as_str_lossy).unwrap_or_default());
        let token = msg.get("token").map(JsonVal::as_str_lossy).unwrap_or_default();
        let valid = st
            .rooms
            .get(&code)
            .is_some_and(|r| constant_time_eq(&r.token, &token));
        if !valid {
            st.note_bad_join(&client.ip);
            return client.send_text(error_json("no_room"));
        }
        let mut old_host: Option<ClientHandle> = None;
        let mut guests_to_kick = Vec::new();
        if let Some(room) = st.rooms.get_mut(&code) {
            if let Some(old) = room.host.take() {
                if old.conn_id != client.conn_id {
                    guests_to_kick.extend(room.guests.drain().map(|(_, g)| g));
                    old_host = Some(old);
                }
            }
            room.host = Some(client.clone());
            room.entered = false;
            room.parked_since = Some(now_secs());
        }
        if let Some(old) = old_host {
            if let Some(oci) = st.conns.get_mut(&old.conn_id) {
                oci.room = None;
            }
            for g in guests_to_kick {
                if let Some(gci) = st.conns.get_mut(&g.conn_id) {
                    gci.room = None;
                }
                g.close(CLOSE_HOST_LEFT, "host_left");
            }
            old.close(CLOSE_REPLACED, "replaced");
        }
        let limits = st.limits.clone();
        if let Some(ci) = st.conns.get_mut(&client.conn_id) {
            ci.room = Some(code.clone());
            ci.id = 0;
            ci.set_role(true, &limits);
        }
        if let Some(room) = st.rooms.get(&code) {
            client.send_text(st.hosted_reply(room));
        }
    }

    fn cmd_join(&self, st: &mut RelayState, client: &ClientHandle, msg: &HashMap<String, JsonVal>) {
        if st.too_many_bad_joins(&client.ip) {
            return client.send_text(error_json("rate"));
        }
        let code = Self::normal_code(&msg.get("code").map(JsonVal::as_str_lossy).unwrap_or_default());
        let Some(room) = st.rooms.get(&code) else {
            st.note_bad_join(&client.ip);
            return client.send_text(error_json("no_room"));
        };
        if !room.open() {
            st.note_bad_join(&client.ip);
            return client.send_text(error_json("not_open"));
        }
        let Some(gid) = room.free_id() else {
            return client.send_text(error_json("full"));
        };
        let room = st.rooms.get_mut(&code).unwrap();
        let host = room.host.clone();
        let joined_reply = format!(
            "{{\"t\":\"joined\",\"code\":{},\"id\":{gid},\"name\":{},\"info\":{}}}",
            json_str(&room.code),
            json_str(&room.name),
            json_str(&room.info)
        );
        room.guests.insert(gid, client.clone());
        let limits = st.limits.clone();
        if let Some(ci) = st.conns.get_mut(&client.conn_id) {
            ci.room = Some(code);
            ci.id = gid;
            ci.set_role(false, &limits);
        }
        client.send_text(joined_reply);
        if let Some(h) = host {
            h.send_text(format!("{{\"t\":\"peer\",\"id\":{gid},\"up\":true}}"));
        }
    }

    fn handle_binary(&self, conn_id: u64, data: Bytes) {
        if data.is_empty() {
            return;
        }
        let st = self.state.lock().unwrap();
        let Some(ci) = st.conns.get(&conn_id) else {
            return;
        };
        let Some(code) = &ci.room else {
            return;
        };
        let Some(room) = st.rooms.get(code) else {
            return;
        };
        let Some(host) = &room.host else {
            return;
        };

        if ci.id == 0 {
            let target = data[0];
            let payload = data.slice(1..);
            if target == ALL {
                for g in room.guests.values() {
                    g.send_binary(payload.clone());
                }
            } else if (target & ALL_EXCEPT) != 0 {
                let except_id = target & 0x7F;
                for (&gid, g) in &room.guests {
                    if gid != except_id {
                        g.send_binary(payload.clone());
                    }
                }
            } else if let Some(g) = room.guests.get(&target) {
                g.send_binary(payload);
            }
        } else {
            let mut tagged = BytesMut::with_capacity(1 + data.len());
            tagged.extend_from_slice(&[ci.id]);
            tagged.extend_from_slice(&data);
            host.send_binary(tagged.freeze());
        }
    }

    fn handle_disconnect(&self, conn_id: u64) {
        let mut st = self.state.lock().unwrap();
        let Some(ci) = st.conns.remove(&conn_id) else {
            return;
        };
        if let Some(cnt) = st.per_ip.get_mut(&ci.ip) {
            *cnt = cnt.saturating_sub(1);
            if *cnt == 0 {
                st.per_ip.remove(&ci.ip);
            }
        }
        let Some(code) = ci.room else {
            return;
        };
        if ci.id == 0 {
            let mut guests_to_kick = Vec::new();
            if let Some(room) = st.rooms.get_mut(&code) {
                if room.host.as_ref().is_some_and(|h| h.conn_id == conn_id) {
                    room.host = None;
                    guests_to_kick.extend(room.guests.drain().map(|(_, g)| g));
                    room.entered = false;
                    room.parked_since = Some(now_secs());
                }
            }
            for g in guests_to_kick {
                if let Some(gci) = st.conns.get_mut(&g.conn_id) {
                    gci.room = None;
                }
                g.close(CLOSE_HOST_LEFT, "host_left");
            }
        } else if let Some(room) = st.rooms.get_mut(&code) {
            if room.guests.get(&ci.id).is_some_and(|g| g.conn_id == conn_id) {
                room.guests.remove(&ci.id);
                if let Some(host) = &room.host {
                    host.send_text(format!("{{\"t\":\"peer\",\"id\":{},\"up\":false}}", ci.id));
                }
            }
        }
    }
}

// ---- HTTP & WebSocket transport -----------------------------------------------------

struct ParsedRequest {
    method: String,
    path: String,
    version: String,
    upgrade_ws: bool,
    sec_ws_key: Option<String>,
    origin: Option<String>,
    host: Option<String>,
    forwarded_for: Vec<String>,
    keep_alive: bool,
}

fn parse_http_header(raw: &[u8]) -> Option<ParsedRequest> {
    let text = std::str::from_utf8(raw).ok()?;
    let mut lines = text.split("\r\n");
    let first = lines.next()?;
    let mut parts = first.split_whitespace();
    let method = parts.next()?.to_string();
    let raw_target = parts.next()?;
    let version = parts.next().unwrap_or("HTTP/1.1").to_string();
    let path = raw_target.split('?').next().unwrap_or(raw_target).to_string();

    let mut upgrade_ws = false;
    let mut sec_ws_key = None;
    let mut origin = None;
    let mut host = None;
    let mut forwarded_for = Vec::new();
    let mut keep_alive = version.eq_ignore_ascii_case("HTTP/1.1");

    for line in lines {
        if line.is_empty() {
            break;
        }
        let Some((k, v)) = line.split_once(':') else {
            continue;
        };
        let k = k.trim();
        let v = v.trim();
        if k.eq_ignore_ascii_case("Upgrade") && v.eq_ignore_ascii_case("websocket") {
            upgrade_ws = true;
        } else if k.eq_ignore_ascii_case("Sec-WebSocket-Key") {
            sec_ws_key = Some(v.to_string());
        } else if k.eq_ignore_ascii_case("Origin") {
            origin = Some(v.to_string());
        } else if k.eq_ignore_ascii_case("Host") {
            host = Some(v.to_string());
        } else if k.eq_ignore_ascii_case("X-Forwarded-For") && !v.is_empty() {
            forwarded_for.push(v.to_string());
        } else if k.eq_ignore_ascii_case("Connection") {
            if v.split(',').any(|p| p.trim().eq_ignore_ascii_case("close")) {
                keep_alive = false;
            }
        }
    }

    Some(ParsedRequest {
        method,
        path,
        version,
        upgrade_ws,
        sec_ws_key,
        origin,
        host,
        forwarded_for,
        keep_alive,
    })
}

async fn write_http_response(
    stream: &mut TcpStream,
    status: &str,
    content_type: &str,
    body: &[u8],
    cors_origin: Option<&str>,
    keep_alive: bool,
) -> std::io::Result<()> {
    let conn = if keep_alive { "keep-alive" } else { "close" };
    let mut hdr = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: {conn}\r\n",
        body.len()
    );
    if let Some(orig) = cors_origin {
        hdr.push_str(&format!("Access-Control-Allow-Origin: {orig}\r\nVary: Origin\r\n"));
    }
    hdr.push_str("\r\n");
    stream.write_all(hdr.as_bytes()).await?;
    if !body.is_empty() {
        stream.write_all(body).await?;
    }
    stream.flush().await
}

fn find_header_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n").map(|p| p + 4)
}

async fn handle_socket(mut stream: TcpStream, peer: SocketAddr, relay: Arc<Relay>) {
    let _ = stream.set_nodelay(true);
    let mut peek_buf = vec![0u8; 8192];

    loop {
        // Read up to the end of the HTTP header without consuming any pipelined WebSocket frame bytes.
        let header_len = loop {
            let Ok(Ok(n)) = tokio::time::timeout(Duration::from_secs(10), stream.peek(&mut peek_buf)).await else {
                return;
            };
            if n == 0 {
                return;
            }
            if let Some(end) = find_header_end(&peek_buf[..n]) {
                break end;
            }
            if n == peek_buf.len() {
                return;
            }
            // Wait briefly for the rest of the header to arrive.
            tokio::time::sleep(Duration::from_millis(2)).await;
        };

        let mut hdr_bytes = vec![0u8; header_len];
        if stream.read_exact(&mut hdr_bytes).await.is_err() {
            return;
        }
        let Some(req) = parse_http_header(&hdr_bytes) else {
            return;
        };
        let _ = &req.method;
        let _ = &req.version;

        let norm_path = if req.path == "/mp" {
            "/"
        } else if let Some(stripped) = req.path.strip_prefix("/mp/") {
            if stripped.is_empty() {
                "/"
            } else {
                &req.path[3..]
            }
        } else {
            req.path.as_str()
        };

        let origin_allowed = relay.origin_ok(req.origin.as_deref(), req.host.as_deref());

        if !req.upgrade_ws {
            let cors = req.origin.as_deref().filter(|_| origin_allowed);
            let res = match norm_path {
                "/stats" => {
                    let body = relay.stats_json();
                    write_http_response(&mut stream, "200 OK", "application/json", body.as_bytes(), cors, req.keep_alive).await
                }
                "/rooms" => {
                    let body = relay.listing_json();
                    write_http_response(&mut stream, "200 OK", "application/json", body.as_bytes(), cors, req.keep_alive).await
                }
                "/health" => {
                    write_http_response(&mut stream, "200 OK", "text/plain", b"ok", cors, req.keep_alive).await
                }
                _ => {
                    write_http_response(&mut stream, "404 Not Found", "text/plain", b"not found\n", cors, false).await
                }
            };
            if res.is_err() || !req.keep_alive || !matches!(norm_path, "/stats" | "/rooms" | "/health") {
                return;
            }
            continue;
        }

        // WebSocket upgrade path
        if !matches!(norm_path, "/" | "/ws") {
            let _ = write_http_response(&mut stream, "404 Not Found", "text/plain", b"not found\n", None, false).await;
            return;
        }
        if !origin_allowed {
            let _ = write_http_response(&mut stream, "403 Forbidden", "text/plain", b"origin not allowed\n", None, false).await;
            return;
        }
        let Some(key) = req.sec_ws_key else {
            let _ = write_http_response(&mut stream, "400 Bad Request", "text/plain", b"bad request\n", None, false).await;
            return;
        };

        let accept = derive_accept_key(key.as_bytes());
        let hs = format!(
            "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {accept}\r\n\r\n"
        );
        if stream.write_all(hs.as_bytes()).await.is_err() || stream.flush().await.is_err() {
            return;
        }

        let ip = {
            let st = relay.state.lock().unwrap();
            if st.trust_proxy && !req.forwarded_for.is_empty() {
                let joined = req.forwarded_for.join(",");
                joined
                    .split(',')
                    .rev()
                    .map(str::trim)
                    .find(|s| !s.is_empty())
                    .map(ToString::to_string)
                    .unwrap_or_else(|| peer.ip().to_string())
            } else {
                peer.ip().to_string()
            }
        };

        let mut ws_cfg = WebSocketConfig::default();
        ws_cfg.max_message_size = Some(MAX_FRAME);
        ws_cfg.max_frame_size = Some(MAX_FRAME);
        let ws = WebSocketStream::from_raw_socket(stream, Role::Server, Some(ws_cfg)).await;
        handle_ws(ws, ip, relay).await;
        return;
    }
}

async fn handle_ws(mut ws: WebSocketStream<TcpStream>, ip: String, relay: Arc<Relay>) {
    let conn_id = relay.next_conn_id.fetch_add(1, Ordering::Relaxed);
    let (busy, first_wait) = {
        let mut st = relay.state.lock().unwrap();
        let ip_cnt = st.per_ip.get(&ip).copied().unwrap_or(0);
        if st.conns.len() >= st.limits.connections || ip_cnt >= st.limits.connections_per_ip {
            (true, Duration::from_secs(10))
        } else {
            st.conns.insert(
                conn_id,
                ConnInfo {
                    ip: ip.clone(),
                    room: None,
                    id: 0,
                    frames: None,
                    bytes: None,
                },
            );
            st.per_ip.insert(ip.clone(), ip_cnt + 1);
            (false, Duration::from_secs_f64(st.limits.first_message_seconds))
        }
    };

    if busy {
        let _ = ws
            .close(Some(CloseFrame {
                code: CloseCode::from(1013),
                reason: "busy".into(),
            }))
            .await;
        return;
    }

    let (mut ws_tx, mut ws_rx) = ws.split();
    let (ctrl_tx, mut ctrl_rx) = mpsc::unbounded_channel::<Message>();
    let (bin_tx, mut bin_rx) = mpsc::channel::<Message>(MAX_QUEUED_BINARY_FRAMES);

    let client = ClientHandle {
        conn_id,
        ip,
        ctrl_tx,
        bin_tx,
    };

    let writer = tokio::spawn(async move {
        let mut ping_interval = tokio::time::interval(Duration::from_secs(20));
        ping_interval.tick().await;
        loop {
            tokio::select! {
                biased;
                ctrl = ctrl_rx.recv() => {
                    match ctrl {
                        Some(msg) => {
                            let is_close = matches!(msg, Message::Close(_));
                            if ws_tx.send(msg).await.is_err() || is_close {
                                break;
                            }
                        }
                        None => break,
                    }
                }
                bin = bin_rx.recv() => {
                    match bin {
                        Some(msg) => {
                            if ws_tx.send(msg).await.is_err() {
                                break;
                            }
                        }
                        None => break,
                    }
                }
                _ = ping_interval.tick() => {
                    if ws_tx.send(Message::Ping(Bytes::new())).await.is_err() {
                        break;
                    }
                }
            }
        }
        let _ = ws_tx.close().await;
    });

    let mut has_room = false;
    loop {
        let next_msg = if !has_room {
            match tokio::time::timeout(first_wait, ws_rx.next()).await {
                Ok(m) => m,
                Err(_) => {
                    client.close(1008, "idle");
                    break;
                }
            }
        } else {
            ws_rx.next().await
        };

        let Some(Ok(msg)) = next_msg else {
            break;
        };

        match msg {
            Message::Text(text) => {
                let allowed = relay.with_state(|st| {
                    st.conns
                        .get_mut(&conn_id)
                        .is_some_and(|ci| ci.allow(text.len()))
                });
                if !allowed {
                    client.close(1008, "rate");
                    break;
                }
                relay.handle_control(&client, &text);
                has_room = relay.with_state(|st| {
                    st.conns
                        .get(&conn_id)
                        .is_some_and(|ci| ci.room.is_some())
                });
            }
            Message::Binary(data) => {
                let allowed = relay.with_state(|st| {
                    st.conns
                        .get_mut(&conn_id)
                        .is_some_and(|ci| ci.allow(data.len()))
                });
                if !allowed {
                    client.close(1008, "rate");
                    break;
                }
                relay.handle_binary(conn_id, data);
            }
            Message::Ping(payload) => {
                let _ = client.ctrl_tx.send(Message::Pong(payload));
            }
            Message::Pong(_) => {}
            Message::Close(_) => break,
            Message::Frame(_) => {}
        }
    }

    relay.handle_disconnect(conn_id);
    drop(client);
    let _ = tokio::time::timeout(Duration::from_millis(200), writer).await;
}

pub struct ServerHandle {
    pub addr: SocketAddr,
    pub relay: Arc<Relay>,
    stop_tx: Option<oneshot::Sender<()>>,
}

impl ServerHandle {
    pub fn close(&mut self) {
        if let Some(tx) = self.stop_tx.take() {
            let _ = tx.send(());
        }
    }
}

pub async fn start(host: &str, port: u16, relay: Option<Arc<Relay>>) -> std::io::Result<ServerHandle> {
    let relay = relay.unwrap_or_else(|| Relay::new(false, std::iter::empty()));
    let listener = TcpListener::bind((host, port)).await?;
    let addr = listener.local_addr()?;
    let (stop_tx, mut stop_rx) = oneshot::channel::<()>();
    let relay_loop = Arc::clone(&relay);

    tokio::spawn(async move {
        loop {
            tokio::select! {
                res = listener.accept() => {
                    let Ok((stream, peer)) = res else { continue };
                    let r = Arc::clone(&relay_loop);
                    tokio::spawn(handle_socket(stream, peer, r));
                }
                _ = &mut stop_rx => break,
            }
        }
    });

    Ok(ServerHandle {
        addr,
        relay,
        stop_tx: Some(stop_tx),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;

    type WsClient = WebSocketStream<tokio_tungstenite::MaybeTlsStream<TcpStream>>;

    async fn connect_ws(port: u16, path: &str) -> WsClient {
        let (ws, _) = tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{port}{path}"))
            .await
            .unwrap();
        ws
    }

    async fn ask(ws: &mut WsClient, msg: &str) -> HashMap<String, JsonVal> {
        ws.send(Message::Text(msg.to_string().into())).await.unwrap();
        let reply = tokio::time::timeout(Duration::from_secs(3), ws.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let Message::Text(text) = reply else {
            panic!("expected text frame, got {reply:?}");
        };
        JsonParser::new(&text).parse_object().unwrap()
    }

    async fn recv_json(ws: &mut WsClient) -> HashMap<String, JsonVal> {
        let reply = tokio::time::timeout(Duration::from_secs(3), ws.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let Message::Text(text) = reply else {
            panic!("expected text frame, got {reply:?}");
        };
        JsonParser::new(&text).parse_object().unwrap()
    }

    async fn recv_bytes(ws: &mut WsClient) -> Vec<u8> {
        let reply = tokio::time::timeout(Duration::from_secs(3), ws.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let Message::Binary(b) = reply else {
            panic!("expected binary frame, got {reply:?}");
        };
        b.to_vec()
    }

    async fn recv_close_code(ws: &mut WsClient) -> u16 {
        while let Ok(Some(res)) = tokio::time::timeout(Duration::from_secs(3), ws.next()).await {
            match res {
                Ok(Message::Close(Some(frame))) => return frame.code.into(),
                Ok(Message::Close(None)) | Err(_) => return 1006,
                _ => continue,
            }
        }
        panic!("timed out waiting for close");
    }

    async fn http_get(port: u16, path: &str) -> (u16, String) {
        let mut s = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        let req = format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n");
        s.write_all(req.as_bytes()).await.unwrap();
        let mut raw = Vec::new();
        s.read_to_end(&mut raw).await.unwrap();
        let text = String::from_utf8_lossy(&raw);
        let (hdr, body) = text.split_once("\r\n\r\n").unwrap();
        let status: u16 = hdr
            .lines()
            .next()
            .unwrap()
            .split_whitespace()
            .nth(1)
            .unwrap()
            .parse()
            .unwrap();
        (status, body.to_string())
    }

    #[tokio::test]
    async fn join_and_pass_frames_and_broadcast_except() {
        let mut srv = start("127.0.0.1", 0, None).await.unwrap();
        let port = srv.addr.port();

        let mut host = connect_ws(port, "/mp/ws").await;
        let reply = ask(&mut host, r#"{"t":"host","name":"Mike's","public":true}"#).await;
        assert_eq!(reply["t"].as_str_lossy(), "hosted");
        let code = reply["code"].as_str_lossy();
        assert_eq!(code.len(), 5);

        let mut guest = connect_ws(port, "/mp/ws").await;
        let j_reply = ask(
            &mut guest,
            &format!(r#"{{"t":"join","code":"{}"}}"#, code.to_ascii_lowercase()),
        )
        .await;
        assert_eq!(j_reply["t"].as_str_lossy(), "joined");
        assert_eq!(j_reply["id"].as_str_lossy(), "1");
        assert_eq!(j_reply["name"].as_str_lossy(), "Mike's");

        let peer = recv_json(&mut host).await;
        assert_eq!(peer["t"].as_str_lossy(), "peer");
        assert_eq!(peer["id"].as_str_lossy(), "1");
        assert_eq!(peer["up"], JsonVal::Bool(true));

        guest.send(Message::Binary(b"hello"[..].into())).await.unwrap();
        assert_eq!(recv_bytes(&mut host).await, b"\x01hello");

        host.send(Message::Binary(b"\x01back"[..].into())).await.unwrap();
        assert_eq!(recv_bytes(&mut guest).await, b"back");

        let mut guest2 = connect_ws(port, "/mp/ws").await;
        let j2 = ask(&mut guest2, &format!(r#"{{"t":"join","code":"{code}"}}"#)).await;
        assert_eq!(j2["id"].as_str_lossy(), "2");
        let _ = recv_json(&mut host).await;

        // Broadcast to ALL (255)
        host.send(Message::Binary(b"\xffall"[..].into())).await.unwrap();
        assert_eq!(recv_bytes(&mut guest).await, b"all");
        assert_eq!(recv_bytes(&mut guest2).await, b"all");

        // Broadcast to ALL_EXCEPT | 1 (0x81): goes only to guest 2, not guest 1
        host.send(Message::Binary(b"\x81from1"[..].into())).await.unwrap();
        assert_eq!(recv_bytes(&mut guest2).await, b"from1");
        host.send(Message::Binary(b"\x01only1"[..].into())).await.unwrap();
        assert_eq!(recv_bytes(&mut guest).await, b"only1");

        srv.close();
    }

    #[tokio::test]
    async fn guest_cannot_reach_other_guests() {
        let mut srv = start("127.0.0.1", 0, None).await.unwrap();
        let port = srv.addr.port();

        let mut host = connect_ws(port, "/mp/ws").await;
        let code = ask(&mut host, r#"{"t":"host"}"#).await["code"].as_str_lossy();
        let mut a = connect_ws(port, "/mp/ws").await;
        let mut b = connect_ws(port, "/mp/ws").await;
        ask(&mut a, &format!(r#"{{"t":"join","code":"{code}"}}"#)).await;
        ask(&mut b, &format!(r#"{{"t":"join","code":"{code}"}}"#)).await;
        a.send(Message::Binary(b"x"[..].into())).await.unwrap();
        host.send(Message::Binary(b"\x02for b"[..].into())).await.unwrap();
        assert_eq!(recv_bytes(&mut b).await, b"for b");

        srv.close();
    }

    #[tokio::test]
    async fn guest_leaves_and_host_leaves() {
        let mut srv = start("127.0.0.1", 0, None).await.unwrap();
        let port = srv.addr.port();

        let mut host = connect_ws(port, "/mp/ws").await;
        let code = ask(&mut host, r#"{"t":"host"}"#).await["code"].as_str_lossy();
        let mut guest = connect_ws(port, "/mp/ws").await;
        ask(&mut guest, &format!(r#"{{"t":"join","code":"{code}"}}"#)).await;
        let _ = recv_json(&mut host).await;
        let _ = guest.close(None).await;
        let peer_down = recv_json(&mut host).await;
        assert_eq!(peer_down["id"].as_str_lossy(), "1");
        assert_eq!(peer_down["up"], JsonVal::Bool(false));

        let mut guest = connect_ws(port, "/mp/ws").await;
        let reused = ask(&mut guest, &format!(r#"{{"t":"join","code":"{code}"}}"#)).await;
        assert_eq!(reused["id"].as_str_lossy(), "1");
        let _ = recv_json(&mut host).await;

        let _ = host.close(None).await;
        assert_eq!(recv_close_code(&mut guest).await, CLOSE_HOST_LEFT);

        let mut late = connect_ws(port, "/mp/ws").await;
        let reply = ask(&mut late, &format!(r#"{{"t":"join","code":"{code}"}}"#)).await;
        assert_eq!(reply["reason"].as_str_lossy(), "not_open");

        srv.close();
    }

    #[tokio::test]
    async fn room_full_and_browser_lists_only_public_rooms() {
        let mut srv = start("127.0.0.1", 0, None).await.unwrap();
        srv.relay.with_state(|st| st.limits.connections_per_ip = 20);
        let port = srv.addr.port();

        let mut host = connect_ws(port, "/mp/ws").await;
        let code = ask(
            &mut host,
            r#"{"t":"host","name":"Open","public":true,"info":"Stadium"}"#,
        )
        .await["code"]
            .as_str_lossy();

        let mut _secret = connect_ws(port, "/mp/ws").await;
        ask(&mut _secret, r#"{"t":"host","name":"Secret","public":false}"#).await;

        let (st_code, rooms_body) = http_get(port, "/mp/rooms").await;
        assert_eq!(st_code, 200);
        assert!(rooms_body.contains("\"name\":\"Open\""));
        assert!(!rooms_body.contains("Secret"));

        let (_, stats_body) = http_get(port, "/mp/stats").await;
        assert_eq!(
            stats_body,
            r#"{"public_rooms":1,"public_players":1,"rooms":2,"players":2}"#
        );
        assert_eq!(http_get(port, "/stats").await.0, 200);
        assert_eq!(http_get(port, "/mp/health").await.1, "ok");
        assert_eq!(http_get(port, "/mp/nothing").await.0, 404);

        let mut held = Vec::new();
        for i in 1..MAX_PLAYERS {
            let mut g = connect_ws(port, "/mp/ws").await;
            let r = ask(&mut g, &format!(r#"{{"t":"join","code":"{code}"}}"#)).await;
            assert_eq!(r["id"].as_str_lossy(), i.to_string());
            held.push(g);
        }
        let mut extra = connect_ws(port, "/mp/ws").await;
        let r = ask(&mut extra, &format!(r#"{{"t":"join","code":"{code}"}}"#)).await;
        assert_eq!(r["reason"].as_str_lossy(), "full");

        srv.close();
    }

    #[tokio::test]
    async fn parked_enter_park_resume_and_sweep() {
        let mut srv = start("127.0.0.1", 0, None).await.unwrap();
        srv.relay.with_state(|st| {
            st.limits.park_seconds = 60.0;
            st.limits.connections_per_ip = 20;
        });
        let port = srv.addr.port();

        let mut host = connect_ws(port, "/mp/ws").await;
        let hosted = ask(
            &mut host,
            r#"{"t":"host","parked":true,"name":"Keep","public":true}"#,
        )
        .await;
        assert_eq!(hosted["entered"], JsonVal::Bool(false));
        let code = hosted["code"].as_str_lossy();
        let token = hosted["token"].as_str_lossy();
        assert!(!token.is_empty());

        // Hidden while parked
        assert_eq!(http_get(port, "/mp/rooms").await.1, r#"{"rooms":[]}"#);
        let mut g0 = connect_ws(port, "/mp/ws").await;
        assert_eq!(
            ask(&mut g0, &format!(r#"{{"t":"join","code":"{code}"}}"#)).await["reason"].as_str_lossy(),
            "not_open"
        );

        // Enter opens it
        assert_eq!(ask(&mut host, r#"{"t":"enter"}"#).await["t"].as_str_lossy(), "entered");
        assert!(http_get(port, "/mp/rooms").await.1.contains("\"name\":\"Keep\""));

        let mut guest = connect_ws(port, "/mp/ws").await;
        assert_eq!(
            ask(&mut guest, &format!(r#"{{"t":"join","code":"{code}"}}"#)).await["t"].as_str_lossy(),
            "joined"
        );
        let _ = recv_json(&mut host).await;

        // Park kicks guests and hides room
        assert_eq!(ask(&mut host, r#"{"t":"park"}"#).await["t"].as_str_lossy(), "parked");
        assert_eq!(recv_close_code(&mut guest).await, CLOSE_HOST_LEFT);
        assert_eq!(http_get(port, "/mp/rooms").await.1, r#"{"rooms":[]}"#);

        // Resume with wrong token fails; right token replaces stale host connection
        let mut bad = connect_ws(port, "/mp/ws").await;
        assert_eq!(
            ask(&mut bad, &format!(r#"{{"t":"resume","code":"{code}","token":"wrong"}}"#)).await["reason"]
                .as_str_lossy(),
            "no_room"
        );

        let mut newer = connect_ws(port, "/mp/ws").await;
        let resumed = ask(
            &mut newer,
            &format!(r#"{{"t":"resume","code":"{}","token":"{token}"}}"#, code.to_ascii_lowercase()),
        )
        .await;
        assert_eq!(resumed["t"].as_str_lossy(), "hosted");
        assert_eq!(resumed["entered"], JsonVal::Bool(false));
        assert_eq!(recv_close_code(&mut host).await, CLOSE_REPLACED);

        // Sweep expires parked room after park_seconds
        srv.relay.sweep(Some(now_secs() + 30.0));
        assert!(srv.relay.with_state(|st| st.rooms.contains_key(&code)));
        srv.relay.sweep(Some(now_secs() + 120.0));
        assert!(!srv.relay.with_state(|st| st.rooms.contains_key(&code)));
        assert_eq!(recv_close_code(&mut newer).await, CLOSE_EXPIRED);

        srv.close();
    }

    #[tokio::test]
    async fn limits_flood_origin_and_forwarded_for() {
        let relay = Relay::new(true, ["https://allowed.example".to_string()]);
        relay.with_state(|st| {
            st.limits.rooms_per_ip = 1;
            st.limits.bad_joins_per_minute = 2;
            st.limits.guest_rate = (1.0, 5.0);
        });
        let mut srv = start("127.0.0.1", 0, Some(relay)).await.unwrap();
        let port = srv.addr.port();

        // Foreign origin refused (403), allowed origin accepted
        let mut bad_req = format!("ws://127.0.0.1:{port}/mp/ws")
            .into_client_request()
            .unwrap();
        bad_req
            .headers_mut()
            .insert("Origin", "https://evil.example".parse().unwrap());
        assert!(tokio_tungstenite::connect_async(bad_req).await.is_err());

        let mut ok_req = format!("ws://127.0.0.1:{port}/mp/ws")
            .into_client_request()
            .unwrap();
        ok_req
            .headers_mut()
            .insert("Origin", "https://allowed.example".parse().unwrap());
        ok_req
            .headers_mut()
            .append("X-Forwarded-For", "1.1.1.1".parse().unwrap());
        ok_req
            .headers_mut()
            .append("X-Forwarded-For", "2.2.2.2, 203.0.113.9".parse().unwrap());
        let (mut host, _) = tokio_tungstenite::connect_async(ok_req).await.unwrap();
        let hosted = ask(&mut host, r#"{"t":"host"}"#).await;
        let code = hosted["code"].as_str_lossy();
        assert_eq!(
            srv.relay.with_state(|st| st.rooms[&code].ip.clone()),
            "203.0.113.9"
        );

        // Guest flood cut
        let mut guest = connect_ws(port, "/mp/ws").await;
        ask(&mut guest, &format!(r#"{{"t":"join","code":"{code}"}}"#)).await;
        for _ in 0..20 {
            let _ = guest.send(Message::Binary(b"x"[..].into())).await;
        }
        assert_eq!(recv_close_code(&mut guest).await, 1008);

        // Bad joins per minute rate limit
        let mut g1 = connect_ws(port, "/mp/ws").await;
        assert_eq!(ask(&mut g1, r#"{"t":"join","code":"AAAAA"}"#).await["reason"].as_str_lossy(), "no_room");
        assert_eq!(ask(&mut g1, r#"{"t":"join","code":"AAAAA"}"#).await["reason"].as_str_lossy(), "no_room");
        assert_eq!(ask(&mut g1, r#"{"t":"join","code":"AAAAA"}"#).await["reason"].as_str_lossy(), "rate");

        srv.close();
    }

    #[tokio::test]
    async fn host_updates_listing_close_and_abandoned_replacement() {
        let mut srv = start("127.0.0.1", 0, None).await.unwrap();
        srv.relay.with_state(|st| {
            st.limits.rooms_per_ip = 2;
            st.limits.connections_per_ip = 20;
        });
        let port = srv.addr.port();

        let mut host = connect_ws(port, "/mp/ws").await;
        let code1 = ask(&mut host, r#"{"t":"host","name":"A","public":false}"#).await["code"].as_str_lossy();
        host.send(Message::Text(r#"{"t":"info","public":true,"info":"Track 4","name":"B\u0000"}"#.into()))
            .await
            .unwrap();
        let mut listed = false;
        for _ in 0..30 {
            let body = http_get(port, "/mp/rooms").await.1;
            if body.contains("\"name\":\"B\"") && body.contains("\"info\":\"Track 4\"") {
                listed = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(listed);

        let mut host2 = connect_ws(port, "/mp/ws").await;
        let code2 = ask(&mut host2, r#"{"t":"host","parked":true}"#).await["code"].as_str_lossy();

        // Both hosts are live: third room from same IP is refused with "busy"
        let mut host3 = connect_ws(port, "/mp/ws").await;
        assert_eq!(ask(&mut host3, r#"{"t":"host"}"#).await["reason"].as_str_lossy(), "busy");

        // First host disconnects (abandoned room); new host replaces it
        let _ = host.close(None).await;
        for _ in 0..40 {
            if srv.relay.with_state(|st| st.rooms.get(&code1).is_some_and(|r| r.host.is_none())) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert_eq!(ask(&mut host3, r#"{"t":"host"}"#).await["t"].as_str_lossy(), "hosted");
        assert!(!srv.relay.with_state(|st| st.rooms.contains_key(&code1)));
        assert!(srv.relay.with_state(|st| st.rooms.contains_key(&code2)));

        // "close" deletes room immediately
        host2.send(Message::Text(r#"{"t":"close"}"#.into())).await.unwrap();
        for _ in 0..40 {
            if !srv.relay.with_state(|st| st.rooms.contains_key(&code2)) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(!srv.relay.with_state(|st| st.rooms.contains_key(&code2)));

        srv.close();
    }

    #[tokio::test]
    async fn bad_requests_and_silent_connection_dropped() {
        let mut srv = start("127.0.0.1", 0, None).await.unwrap();
        let port = srv.addr.port();

        let mut ws = connect_ws(port, "/mp/ws").await;
        assert_eq!(ask(&mut ws, "not json").await["reason"].as_str_lossy(), "bad_request");
        assert_eq!(ask(&mut ws, r#"{"t":"nope"}"#).await["reason"].as_str_lossy(), "bad_request");
        assert_eq!(ask(&mut ws, "[1]").await["reason"].as_str_lossy(), "bad_request");

        srv.relay.with_state(|st| st.limits.first_message_seconds = 0.15);
        let mut silent = connect_ws(port, "/mp/ws").await;
        assert_eq!(recv_close_code(&mut silent).await, 1008);

        srv.close();
    }
}
