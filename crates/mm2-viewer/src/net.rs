//! Network play. The game played over DirectPlay (`NetProcs.cpp`), which
//! the port does not have, so it has its own: a host's session is found
//! by a broadcast on the local network (or at an address in `MM2_JOIN`),
//! each player keeps one TCP connection to the host, and the host passes
//! on what each sends. In a race each machine sends its own bikes as the
//! game's own whole-bike packets (type 1, `0x493620`, the same as on a
//! race tape) and the host sends who is "it" in Moto-Tag (type 0x87),
//! which a player asks the host for (0x87 + 0x8c) on touching IT.
//!
//! There is one session at a time, kept here for the lobby and the race
//! to share.

use std::io::{self, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream, UdpSocket};
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// The host's TCP port, and the UDP port sessions are found on.
pub const PORT: u16 = 28_220;
/// What a finder asks and a host answers, before the session's name.
const ASK: &[u8] = b"MM2?";
const ANSWER: &[u8] = b"MM2!";
/// The most players in a session (the lobby's eight names).
pub const MAX_PLAYERS: usize = 8;

/// A player's bike and outfit, as the menus give them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RideInfo {
    pub model: String,
    pub texture: String,
    pub outfit: String,
    pub cc: u32,
    pub plate: u32,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Player {
    pub id: u8,
    pub name: String,
    pub ready: bool,
    pub ride: RideInfo,
}

/// The host's Event and Options pages.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct GameInfo {
    pub event: u8,
    pub mode: u8,
    pub laps: u32,
    /// Computer riders the host adds.
    pub computers: u32,
    /// The track's file, by name (`IFFENDIC`), its folder from the event.
    pub track: String,
    pub course: u8,
    pub level: u8,
    pub tag_ball: bool,
    pub tag_stunts: bool,
    pub collisions: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Msg {
    /// A new player's name and ride, to the host.
    Hello {
        name: String,
        ride: RideInfo,
    },
    /// The host's answer: the player's number.
    Welcome {
        id: u8,
    },
    /// Everyone in the session, from the host.
    Players(Vec<Player>),
    /// A player's Ready button; a player's new ride.
    Ready(bool),
    Ride(RideInfo),
    /// A line of the chat, already with the speaker's name.
    Chat(String),
    Game(GameInfo),
    /// The race begins: the game, and the players in racer order.
    Start {
        game: GameInfo,
        players: Vec<Player>,
    },
    /// One bike in a race: whose racer it is, the sender's race clock,
    /// and the game's packet (its type and data).
    Bike {
        racer: u8,
        time: f32,
        kind: i32,
        data: Vec<u8>,
    },
    /// A racer has finished, or the race has ended for everyone.
    Finished {
        racer: u8,
        time: f32,
    },
    Leave,
}

/// Little-endian fields, strings with a length first.
struct W(Vec<u8>);

impl W {
    fn u8(&mut self, v: u8) -> &mut Self {
        self.0.push(v);
        self
    }
    fn u32(&mut self, v: u32) -> &mut Self {
        self.0.extend(v.to_le_bytes());
        self
    }
    fn f32(&mut self, v: f32) -> &mut Self {
        self.0.extend(v.to_le_bytes());
        self
    }
    fn bytes(&mut self, b: &[u8]) -> &mut Self {
        self.u32(b.len() as u32);
        self.0.extend(b);
        self
    }
    fn str(&mut self, s: &str) -> &mut Self {
        self.bytes(s.as_bytes())
    }
    fn ride(&mut self, r: &RideInfo) -> &mut Self {
        self.str(&r.model)
            .str(&r.texture)
            .str(&r.outfit)
            .u32(r.cc)
            .u32(r.plate)
    }
    fn player(&mut self, p: &Player) -> &mut Self {
        self.u8(p.id).str(&p.name).u8(p.ready as u8).ride(&p.ride)
    }
    fn players(&mut self, ps: &[Player]) -> &mut Self {
        self.u32(ps.len() as u32);
        for p in ps {
            self.player(p);
        }
        self
    }
    fn game(&mut self, g: &GameInfo) -> &mut Self {
        self.u8(g.event)
            .u8(g.mode)
            .u32(g.laps)
            .u32(g.computers)
            .str(&g.track)
            .u8(g.course)
            .u8(g.level)
            .u8(g.tag_ball as u8)
            .u8(g.tag_stunts as u8)
            .u8(g.collisions as u8)
    }
}

struct R<'a>(&'a [u8]);

fn short() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "message cut short")
}

impl R<'_> {
    fn take(&mut self, n: usize) -> io::Result<&[u8]> {
        if self.0.len() < n {
            return Err(short());
        }
        let (a, b) = self.0.split_at(n);
        self.0 = b;
        Ok(a)
    }
    fn u8(&mut self) -> io::Result<u8> {
        Ok(self.take(1)?[0])
    }
    fn u32(&mut self) -> io::Result<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn f32(&mut self) -> io::Result<f32> {
        Ok(f32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn bytes(&mut self) -> io::Result<Vec<u8>> {
        let n = self.u32()? as usize;
        Ok(self.take(n)?.to_vec())
    }
    fn str(&mut self) -> io::Result<String> {
        Ok(String::from_utf8_lossy(&self.bytes()?).into_owned())
    }
    fn ride(&mut self) -> io::Result<RideInfo> {
        Ok(RideInfo {
            model: self.str()?,
            texture: self.str()?,
            outfit: self.str()?,
            cc: self.u32()?,
            plate: self.u32()?,
        })
    }
    fn player(&mut self) -> io::Result<Player> {
        Ok(Player {
            id: self.u8()?,
            name: self.str()?,
            ready: self.u8()? != 0,
            ride: self.ride()?,
        })
    }
    fn players(&mut self) -> io::Result<Vec<Player>> {
        let n = (self.u32()? as usize).min(64);
        (0..n).map(|_| self.player()).collect()
    }
    fn game(&mut self) -> io::Result<GameInfo> {
        Ok(GameInfo {
            event: self.u8()?,
            mode: self.u8()?,
            laps: self.u32()?,
            computers: self.u32()?,
            track: self.str()?,
            course: self.u8()?,
            level: self.u8()?,
            tag_ball: self.u8()? != 0,
            tag_stunts: self.u8()? != 0,
            collisions: self.u8()? != 0,
        })
    }
}

impl Msg {
    pub fn encode(&self) -> Vec<u8> {
        let mut w = W(Vec::new());
        match self {
            Msg::Hello { name, ride } => w.u8(1).str(name).ride(ride),
            Msg::Welcome { id } => w.u8(2).u8(*id),
            Msg::Players(ps) => w.u8(3).players(ps),
            Msg::Ready(r) => w.u8(4).u8(*r as u8),
            Msg::Ride(r) => w.u8(5).ride(r),
            Msg::Chat(t) => w.u8(6).str(t),
            Msg::Game(g) => w.u8(7).game(g),
            Msg::Start { game, players } => w.u8(8).game(game).players(players),
            Msg::Bike {
                racer,
                time,
                kind,
                data,
            } => w.u8(9).u8(*racer).f32(*time).u32(*kind as u32).bytes(data),
            Msg::Finished { racer, time } => w.u8(10).u8(*racer).f32(*time),
            Msg::Leave => w.u8(11),
        };
        w.0
    }

    pub fn decode(b: &[u8]) -> io::Result<Msg> {
        let mut r = R(b);
        Ok(match r.u8()? {
            1 => Msg::Hello {
                name: r.str()?,
                ride: r.ride()?,
            },
            2 => Msg::Welcome { id: r.u8()? },
            3 => Msg::Players(r.players()?),
            4 => Msg::Ready(r.u8()? != 0),
            5 => Msg::Ride(r.ride()?),
            6 => Msg::Chat(r.str()?),
            7 => Msg::Game(r.game()?),
            8 => Msg::Start {
                game: r.game()?,
                players: r.players()?,
            },
            9 => Msg::Bike {
                racer: r.u8()?,
                time: r.f32()?,
                kind: r.u32()? as i32,
                data: r.bytes()?,
            },
            10 => Msg::Finished {
                racer: r.u8()?,
                time: r.f32()?,
            },
            11 => Msg::Leave,
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "unknown message",
                ))
            }
        })
    }
}

/// Sends one message: its length, then it.
fn send(stream: &mut TcpStream, m: &Msg) -> io::Result<()> {
    let body = m.encode();
    let mut frame = (body.len() as u32).to_le_bytes().to_vec();
    frame.extend(body);
    stream.write_all(&frame)
}

fn receive(stream: &mut TcpStream) -> io::Result<Msg> {
    let mut len = [0u8; 4];
    stream.read_exact(&mut len)?;
    let n = u32::from_le_bytes(len) as usize;
    if n > 1 << 20 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "message too long",
        ));
    }
    let mut body = vec![0u8; n];
    stream.read_exact(&mut body)?;
    Msg::decode(&body)
}

/// What the connections' threads tell the session.
enum In {
    Joined(u8, TcpStream),
    Got(u8, Msg),
    Gone(u8),
}

/// Reads a connection until it closes.
fn reader(id: u8, mut stream: TcpStream, tx: Sender<In>) {
    std::thread::spawn(move || {
        while let Ok(m) = receive(&mut stream) {
            if tx.send(In::Got(id, m)).is_err() {
                return;
            }
        }
        let _ = tx.send(In::Gone(id));
    });
}

/// What the session heard, for the lobby and the race.
#[derive(Debug, Clone, PartialEq)]
pub enum Heard {
    /// The players changed (someone came, went, readied or changed bike).
    Players,
    Chat(String),
    Game(GameInfo),
    Start {
        game: GameInfo,
        players: Vec<Player>,
    },
    Bike {
        racer: u8,
        time: f32,
        kind: i32,
        data: Vec<u8>,
    },
    Finished {
        racer: u8,
        time: f32,
    },
    /// The host has gone; the session is over.
    Lost,
}

pub struct Session {
    pub host: bool,
    /// This player's number; the host is 0.
    pub me: u8,
    pub players: Vec<Player>,
    pub game: Option<GameInfo>,
    /// The connections: the host's to each player, a player's to the host
    /// (as 0).
    peers: Vec<(u8, TcpStream)>,
    rx: Receiver<In>,
    stop: Arc<AtomicBool>,
}

impl Session {
    /// Hosts a session called `name` on [`PORT`] (or the next free port),
    /// answering finders on the local network.
    pub fn host(name: &str, me: Player) -> io::Result<Session> {
        let listener = (0..8)
            .find_map(|k| TcpListener::bind(("0.0.0.0", PORT + k)).ok())
            .ok_or_else(|| io::Error::new(io::ErrorKind::AddrInUse, "no free port"))?;
        let port = listener.local_addr()?.port();
        let (tx, rx) = channel();
        let stop = Arc::new(AtomicBool::new(false));
        let next = Arc::new(AtomicU8::new(1));
        {
            let (tx, stop) = (tx.clone(), stop.clone());
            std::thread::spawn(move || {
                for stream in listener.incoming() {
                    if stop.load(Ordering::Relaxed) {
                        return;
                    }
                    let Ok(stream) = stream else { continue };
                    let _ = stream.set_nodelay(true);
                    let id = next.fetch_add(1, Ordering::Relaxed);
                    let Ok(read) = stream.try_clone() else {
                        continue;
                    };
                    reader(id, read, tx.clone());
                    if tx.send(In::Joined(id, stream)).is_err() {
                        return;
                    }
                }
            });
        }
        answer_finders(name.to_string(), port, stop.clone());
        Ok(Session {
            host: true,
            me: 0,
            players: vec![Player { id: 0, ..me }],
            game: None,
            peers: Vec::new(),
            rx,
            stop,
        })
    }

    /// Joins the session at `addr` as `me`.
    pub fn join(addr: SocketAddr, me: Player) -> io::Result<Session> {
        let mut stream = TcpStream::connect_timeout(&addr, Duration::from_secs(3))?;
        stream.set_nodelay(true)?;
        let (tx, rx) = channel();
        reader(0, stream.try_clone()?, tx);
        send(
            &mut stream,
            &Msg::Hello {
                name: me.name.clone(),
                ride: me.ride.clone(),
            },
        )?;
        Ok(Session {
            host: false,
            me: u8::MAX,
            players: Vec::new(),
            game: None,
            peers: vec![(0, stream)],
            rx,
            stop: Arc::new(AtomicBool::new(false)),
        })
    }

    /// Sends to everyone but `except`.
    fn broadcast(&mut self, m: &Msg, except: Option<u8>) {
        let mut gone = Vec::new();
        for (id, s) in &mut self.peers {
            if Some(*id) != except && send(s, m).is_err() {
                gone.push(*id);
            }
        }
        self.peers.retain(|(id, _)| !gone.contains(id));
    }

    /// What this player does: to the host, or from the host to everyone.
    pub fn say(&mut self, m: Msg) {
        if self.host {
            match &m {
                Msg::Ready(r) => {
                    if let Some(p) = self.players.iter_mut().find(|p| p.id == 0) {
                        p.ready = *r;
                    }
                    let ps = Msg::Players(self.players.clone());
                    self.broadcast(&ps, None);
                    return;
                }
                Msg::Ride(r) => {
                    if let Some(p) = self.players.iter_mut().find(|p| p.id == 0) {
                        p.ride = r.clone();
                    }
                    let ps = Msg::Players(self.players.clone());
                    self.broadcast(&ps, None);
                    return;
                }
                Msg::Game(g) => self.game = Some(g.clone()),
                _ => {}
            }
            self.broadcast(&m, None);
        } else {
            self.broadcast(&m, None);
        }
    }

    /// Everything heard since last time; the host also answers and passes
    /// messages on.
    pub fn poll(&mut self) -> Vec<Heard> {
        let mut out = Vec::new();
        while let Ok(i) = self.rx.try_recv() {
            match i {
                In::Joined(id, stream) => {
                    if self.players.len() >= MAX_PLAYERS {
                        continue;
                    }
                    self.peers.push((id, stream));
                }
                In::Gone(id) => {
                    self.peers.retain(|(p, _)| *p != id);
                    if !self.host {
                        out.push(Heard::Lost);
                        continue;
                    }
                    self.players.retain(|p| p.id != id);
                    let ps = Msg::Players(self.players.clone());
                    self.broadcast(&ps, None);
                    out.push(Heard::Players);
                }
                In::Got(from, m) => self.got(from, m, &mut out),
            }
        }
        out
    }

    fn got(&mut self, from: u8, m: Msg, out: &mut Vec<Heard>) {
        if self.host {
            match m {
                Msg::Hello { name, ride } => {
                    self.players.push(Player {
                        id: from,
                        name,
                        ready: false,
                        ride,
                    });
                    if let Some((_, s)) = self.peers.iter_mut().find(|(id, _)| *id == from) {
                        let _ = send(s, &Msg::Welcome { id: from });
                        if let Some(g) = &self.game {
                            let _ = send(s, &Msg::Game(g.clone()));
                        }
                    }
                    let ps = Msg::Players(self.players.clone());
                    self.broadcast(&ps, None);
                    out.push(Heard::Players);
                }
                Msg::Ready(r) => {
                    if let Some(p) = self.players.iter_mut().find(|p| p.id == from) {
                        p.ready = r;
                    }
                    let ps = Msg::Players(self.players.clone());
                    self.broadcast(&ps, None);
                    out.push(Heard::Players);
                }
                Msg::Ride(r) => {
                    if let Some(p) = self.players.iter_mut().find(|p| p.id == from) {
                        p.ride = r;
                    }
                    let ps = Msg::Players(self.players.clone());
                    self.broadcast(&ps, None);
                    out.push(Heard::Players);
                }
                Msg::Chat(t) => {
                    self.broadcast(&Msg::Chat(t.clone()), None);
                    out.push(Heard::Chat(t));
                }
                Msg::Bike {
                    racer,
                    time,
                    kind,
                    data,
                } => {
                    let m = Msg::Bike {
                        racer,
                        time,
                        kind,
                        data,
                    };
                    self.broadcast(&m, Some(from));
                    if let Msg::Bike {
                        racer,
                        time,
                        kind,
                        data,
                    } = m
                    {
                        out.push(Heard::Bike {
                            racer,
                            time,
                            kind,
                            data,
                        });
                    }
                }
                Msg::Finished { racer, time } => {
                    self.broadcast(&Msg::Finished { racer, time }, Some(from));
                    out.push(Heard::Finished { racer, time });
                }
                Msg::Leave => {
                    self.peers.retain(|(p, _)| *p != from);
                    self.players.retain(|p| p.id != from);
                    let ps = Msg::Players(self.players.clone());
                    self.broadcast(&ps, None);
                    out.push(Heard::Players);
                }
                _ => {}
            }
            return;
        }
        match m {
            Msg::Welcome { id } => self.me = id,
            Msg::Players(ps) => {
                self.players = ps;
                out.push(Heard::Players);
            }
            Msg::Chat(t) => out.push(Heard::Chat(t)),
            Msg::Game(g) => {
                self.game = Some(g.clone());
                out.push(Heard::Game(g));
            }
            Msg::Start { game, players } => out.push(Heard::Start { game, players }),
            Msg::Bike {
                racer,
                time,
                kind,
                data,
            } => out.push(Heard::Bike {
                racer,
                time,
                kind,
                data,
            }),
            Msg::Finished { racer, time } => out.push(Heard::Finished { racer, time }),
            Msg::Leave => out.push(Heard::Lost),
            _ => {}
        }
    }

    /// Whether every player has pressed Ready.
    pub fn all_ready(&self) -> bool {
        self.players.iter().all(|p| p.ready)
    }

    pub fn leave(&mut self) {
        self.broadcast(&Msg::Leave, None);
        self.stop.store(true, Ordering::Relaxed);
        self.peers.clear();
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.leave();
    }
}

/// Answers finders' broadcasts with the session's name and port.
fn answer_finders(name: String, port: u16, stop: Arc<AtomicBool>) {
    let Ok(sock) = UdpSocket::bind(("0.0.0.0", PORT)) else {
        eprintln!("cannot answer on UDP {PORT}; players must join by address");
        return;
    };
    let _ = sock.set_read_timeout(Some(Duration::from_millis(500)));
    std::thread::spawn(move || {
        let mut buf = [0u8; 64];
        while !stop.load(Ordering::Relaxed) {
            if let Ok((n, from)) = sock.recv_from(&mut buf) {
                if &buf[..n] == ASK {
                    let mut reply = ANSWER.to_vec();
                    reply.extend(port.to_le_bytes());
                    reply.extend(name.as_bytes());
                    let _ = sock.send_to(&reply, from);
                }
            }
        }
    });
}

/// A session a finder heard of.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    pub name: String,
    pub addr: SocketAddr,
}

/// Looks for sessions: asks the local network, and any addresses in
/// `MM2_JOIN` (comma separated), every second.
pub struct Finder {
    sock: Option<UdpSocket>,
    asked: Option<Instant>,
    pub found: Vec<Found>,
    /// Hosts asked directly, by address or name, with or without a port:
    /// `MM2_JOIN`'s and the TCP/IP box's.
    pub hosts: Vec<String>,
}

impl Finder {
    pub fn new() -> Finder {
        let sock = UdpSocket::bind(("0.0.0.0", 0)).ok();
        if let Some(s) = &sock {
            let _ = s.set_broadcast(true);
            let _ = s.set_nonblocking(true);
        }
        let hosts = std::env::var("MM2_JOIN")
            .unwrap_or_default()
            .split(',')
            .map(|a| a.trim().to_string())
            .filter(|a| !a.is_empty())
            .collect();
        Finder {
            sock,
            asked: None,
            found: Vec::new(),
            hosts,
        }
    }

    /// Asks again if a second has passed, and takes the answers. Whether
    /// the list changed.
    pub fn tick(&mut self) -> bool {
        let Some(sock) = &self.sock else {
            return false;
        };
        if self
            .asked
            .is_none_or(|t| t.elapsed() > Duration::from_secs(1))
        {
            self.asked = Some(Instant::now());
            let _ = sock.send_to(ASK, ("255.255.255.255", PORT));
            let _ = sock.send_to(ASK, ("127.0.0.1", PORT));
            for a in &self.hosts {
                let a = if a.contains(':') {
                    a.to_string()
                } else {
                    format!("{a}:{PORT}")
                };
                let _ = sock.send_to(ASK, a.as_str());
            }
        }
        let mut changed = false;
        let mut buf = [0u8; 128];
        while let Ok((n, from)) = sock.recv_from(&mut buf) {
            if n < 6 || &buf[..4] != ANSWER {
                continue;
            }
            let port = u16::from_le_bytes([buf[4], buf[5]]);
            let name = String::from_utf8_lossy(&buf[6..n]).into_owned();
            let addr = SocketAddr::new(from.ip(), port);
            if !self.found.iter().any(|f| f.addr == addr) {
                self.found.push(Found { name, addr });
                changed = true;
            }
        }
        changed
    }
}

/// The session the lobby made, kept for the race.
pub static SESSION: Mutex<Option<Session>> = Mutex::new(None);

/// Runs `f` on the session, if there is one.
pub fn with<T>(f: impl FnOnce(&mut Session) -> T) -> Option<T> {
    SESSION.lock().ok()?.as_mut().map(f)
}

pub fn set(s: Option<Session>) {
    if let Ok(mut g) = SESSION.lock() {
        *g = s;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_go_both_ways() {
        let ride = RideInfo {
            model: "honda.mcf".into(),
            texture: "x.tga".into(),
            outfit: "o.tga".into(),
            cc: 250,
            plate: 107,
        };
        let player = Player {
            id: 3,
            name: "Mike".into(),
            ready: true,
            ride: ride.clone(),
        };
        let game = GameInfo {
            event: 4,
            mode: 1,
            laps: 5,
            computers: 2,
            track: "TAG01".into(),
            course: 1,
            level: 2,
            tag_ball: true,
            tag_stunts: false,
            collisions: true,
        };
        for m in [
            Msg::Hello {
                name: "Mike".into(),
                ride: ride.clone(),
            },
            Msg::Welcome { id: 2 },
            Msg::Players(vec![player.clone(), Player::default()]),
            Msg::Ready(true),
            Msg::Ride(ride),
            Msg::Chat("hi".into()),
            Msg::Game(game.clone()),
            Msg::Start {
                game,
                players: vec![player],
            },
            Msg::Bike {
                racer: 1,
                time: 3.5,
                kind: 1,
                data: vec![1, 2, 3],
            },
            Msg::Finished {
                racer: 2,
                time: 99.0,
            },
            Msg::Leave,
        ] {
            assert_eq!(Msg::decode(&m.encode()).unwrap(), m);
        }
    }

    /// A host and a player on this machine: the player joins, both see
    /// each other, chat goes round and a bike packet is passed on.
    #[test]
    fn a_session_on_this_machine() {
        let me = |name: &str| Player {
            name: name.into(),
            ..Player::default()
        };
        let mut host = Session::host("test", me("Host")).unwrap();
        let port = {
            // The host took the first free port from PORT.
            (PORT..PORT + 8)
                .find(|&p| TcpStream::connect(("127.0.0.1", p)).is_ok())
                .unwrap()
        };
        // That probe connected and left; the host forgets it.
        let addr: SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();
        let mut guest = Session::join(addr, me("Guest")).unwrap();
        let wait = |f: &mut dyn FnMut() -> bool| {
            let start = Instant::now();
            while !f() {
                assert!(start.elapsed() < Duration::from_secs(5), "timed out");
                std::thread::sleep(Duration::from_millis(10));
            }
        };
        wait(&mut || {
            host.poll();
            guest.poll();
            guest.players.iter().any(|p| p.name == "Host") && guest.me != u8::MAX
        });
        assert!(host.players.iter().any(|p| p.name == "Guest"));
        guest.say(Msg::Chat("Guest: hello".into()));
        let mut heard = Vec::new();
        wait(&mut || {
            heard.extend(host.poll());
            heard.contains(&Heard::Chat("Guest: hello".into()))
        });
        host.say(Msg::Bike {
            racer: 0,
            time: 1.0,
            kind: 1,
            data: vec![9; 88],
        });
        let mut got = Vec::new();
        wait(&mut || {
            got.extend(guest.poll());
            got.iter()
                .any(|h| matches!(h, Heard::Bike { racer: 0, .. }))
        });
        guest.say(Msg::Ready(true));
        wait(&mut || {
            host.poll();
            host.players.iter().any(|p| p.name == "Guest" && p.ready)
        });
    }
}
