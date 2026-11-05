//! Network play. The game played over DirectPlay (`NetProcs.cpp`), which
//! the port does not have, so it has its own: a host's session is found
//! by a broadcast on the local network (or at an address in `MM2_JOIN`),
//! each player keeps one TCP connection to the host, and the host passes
//! on what each sends. In a race each machine sends its own bikes as the
//! game's own whole-bike packets (type 1, `0x493620`, the same as on a
//! race tape) and the host sends who is "it" in Moto-Tag (type 0x87),
//! which a player asks the host for (0x87 + 0x8c) on touching IT.
//!
//! In the browser there are no sockets, so the same session runs over a
//! WebSocket relay that the page holds (`mm2Net*` in `mp.js`): a room's
//! host is the session's host, and each other player is a guest the relay
//! numbers. Rooms are found by a code, or in the page's list of public
//! rooms, where the local network's broadcast was.
//!
//! There is one session at a time, kept here for the lobby and the race
//! to share.

use std::io;
#[cfg(not(target_arch = "wasm32"))]
use std::io::{Read, Write};
#[cfg(not(target_arch = "wasm32"))]
use std::net::{SocketAddr, TcpListener, TcpStream, UdpSocket};
#[cfg(not(target_arch = "wasm32"))]
use std::sync::atomic::AtomicU8;
use std::sync::atomic::{AtomicBool, Ordering};
#[cfg(not(target_arch = "wasm32"))]
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
#[cfg(not(target_arch = "wasm32"))]
use std::time::{Duration, Instant};

/// Where a session is: an address on the local network, or in the
/// browser a relay room's code.
#[cfg(not(target_arch = "wasm32"))]
pub type Addr = SocketAddr;
#[cfg(target_arch = "wasm32")]
pub type Addr = String;

/// The host's TCP port, and the UDP port sessions are found on.
#[cfg(not(target_arch = "wasm32"))]
pub const PORT: u16 = 28_220;
/// What a finder asks and a host answers, before the session's name.
#[cfg(not(target_arch = "wasm32"))]
const ASK: &[u8] = b"MM2?";
#[cfg(not(target_arch = "wasm32"))]
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
    /// The host is about to start this game: everyone gets the track first.
    Prepare(GameInfo),
    /// A player has the track (true), or could not get it (false); to the host.
    Loaded(bool),
    /// The host could not get the track: the start is off.
    Abort,
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
            Msg::Prepare(g) => w.u8(12).game(g),
            Msg::Loaded(ok) => w.u8(13).u8(*ok as u8),
            Msg::Abort => w.u8(14),
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
            12 => Msg::Prepare(r.game()?),
            13 => Msg::Loaded(r.u8()? != 0),
            14 => Msg::Abort,
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "unknown message",
                ))
            }
        })
    }
}

/// One connection: a TCP stream, or in the browser the relay's pipe to
/// that player (the page holds the socket).
#[cfg(not(target_arch = "wasm32"))]
type Conn = TcpStream;
#[cfg(target_arch = "wasm32")]
struct Conn;

/// Sends one message to the player `id` (a guest only has the host, 0):
/// on a socket its length, then it; over the relay each frame is a message.
#[cfg(not(target_arch = "wasm32"))]
fn encode_frame(m: &Msg) -> Vec<u8> {
    let body = m.encode();
    let mut frame = Vec::with_capacity(4 + body.len());
    frame.extend_from_slice(&(body.len() as u32).to_le_bytes());
    frame.extend(body);
    frame
}

#[cfg(not(target_arch = "wasm32"))]
fn send(stream: &mut Conn, _id: u8, m: &Msg) -> io::Result<()> {
    stream.write_all(&encode_frame(m))
}

#[cfg(target_arch = "wasm32")]
fn send(_: &mut Conn, id: u8, m: &Msg) -> io::Result<()> {
    if web::send_bytes(id, &m.encode()) {
        Ok(())
    } else {
        Err(io::Error::new(io::ErrorKind::BrokenPipe, "relay closed"))
    }
}

#[cfg(not(target_arch = "wasm32"))]
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
    Joined(u8, Conn),
    Got(u8, Msg),
    Gone(u8),
    /// The relay dropped this player.
    #[cfg(target_arch = "wasm32")]
    Lost,
}

/// Reads a connection until it closes.
#[cfg(not(target_arch = "wasm32"))]
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
    /// The host will start this game once everyone has the track.
    Prepare(GameInfo),
    /// (Host) a player has the track, or could not get it.
    Loaded { from: u8, ok: bool },
    /// The host called the start off.
    Abort,
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
    peers: Vec<(u8, Conn)>,
    #[cfg(not(target_arch = "wasm32"))]
    rx: Receiver<In>,
    stop: Arc<AtomicBool>,
    #[cfg(target_arch = "wasm32")]
    web: web::Link,
}

impl Session {
    /// [`Session::host`]; the desktop game has no server browser, so
    /// `public` means nothing here.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn host_as(name: &str, me: Player, _public: bool) -> io::Result<Session> {
        Session::host(name, me)
    }

    /// Hosts a session called `name` on [`PORT`] (or the next free port),
    /// answering finders on the local network.
    #[cfg(not(target_arch = "wasm32"))]
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
    #[cfg(not(target_arch = "wasm32"))]
    pub fn join(addr: Addr, me: Player) -> io::Result<Session> {
        let mut stream = TcpStream::connect_timeout(&addr, Duration::from_secs(3))?;
        stream.set_nodelay(true)?;
        let (tx, rx) = channel();
        reader(0, stream.try_clone()?, tx);
        send(
            &mut stream,
            0,
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

    /// Sends to everyone but `except`. A TCP connection that fails is dropped; over the
    /// relay a host sends one broadcast frame (`255` for every guest, or `0x80 | id` for
    /// every guest except `id`) so the relay fans it out instead of the host uploading N
    /// copies.
    fn broadcast(&mut self, m: &Msg, except: Option<u8>) {
        if !self.peers.iter().any(|(id, _)| Some(*id) != except) {
            return;
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let frame = encode_frame(m);
            let mut gone = Vec::new();
            for (id, s) in &mut self.peers {
                if Some(*id) != except && s.write_all(&frame).is_err() {
                    gone.push(*id);
                }
            }
            self.peers.retain(|(id, _)| !gone.contains(id));
        }
        #[cfg(target_arch = "wasm32")]
        {
            let target = if !self.host {
                0
            } else if let Some(from) = except {
                0x80 | from
            } else {
                255
            };
            let _ = web::send_bytes(target, &m.encode());
        }
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
        #[cfg(target_arch = "wasm32")]
        self.web_status(&mut out);
        for i in self.incoming() {
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
                #[cfg(target_arch = "wasm32")]
                In::Lost => out.push(Heard::Lost),
            }
        }
        out
    }

    /// What the connections' threads have passed on since last time.
    #[cfg(not(target_arch = "wasm32"))]
    fn incoming(&mut self) -> Vec<In> {
        self.rx.try_iter().collect()
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
                        let _ = send(s, from, &Msg::Welcome { id: from });
                        if let Some(g) = &self.game {
                            let _ = send(s, from, &Msg::Game(g.clone()));
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
                    // The sender has already shown its own line.
                    self.broadcast(&Msg::Chat(t.clone()), Some(from));
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
                Msg::Loaded(ok) => out.push(Heard::Loaded { from, ok }),
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
            Msg::Prepare(g) => out.push(Heard::Prepare(g)),
            Msg::Abort => out.push(Heard::Abort),
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
        // The room is the page's: a guest leaves it, but a host only leaves the lobby, and the
        // room waits (parked) for it to come back.
        #[cfg(target_arch = "wasm32")]
        if self.host {
            web::park();
        } else {
            web::leave();
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.leave();
    }
}

/// Answers finders' broadcasts with the session's name and port.
#[cfg(not(target_arch = "wasm32"))]
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
    pub addr: Addr,
}

/// Looks for sessions: asks the local network, and any addresses in
/// `MM2_JOIN` (comma separated), every second.
#[cfg(not(target_arch = "wasm32"))]
pub struct Finder {
    sock: Option<UdpSocket>,
    asked: Option<Instant>,
    pub found: Vec<Found>,
    /// Hosts asked directly, by address or name, with or without a port:
    /// `MM2_JOIN`'s and the TCP/IP box's.
    pub hosts: Vec<String>,
}

#[cfg(not(target_arch = "wasm32"))]
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

/// The browser's side: the page (`mp.js`) keeps the relay's WebSocket, so a
/// room made on the start page is still open when the game starts. These
/// are its global functions (`mm2Net*`).
#[cfg(target_arch = "wasm32")]
mod web {
    use super::*;

    #[wasm_bindgen::prelude::wasm_bindgen]
    extern "C" {
        /// "idle", "connecting", "hosting" or "joined".
        #[wasm_bindgen(js_name = mm2NetState)]
        fn state() -> String;
        /// The room's code, or nothing.
        #[wasm_bindgen(js_name = mm2NetCode)]
        fn code() -> String;
        /// Why the last attempt failed, or nothing.
        #[wasm_bindgen(js_name = mm2NetError)]
        fn error() -> String;
        /// Opens a room as its host, or asks to join one; the answer comes
        /// by `state`.
        #[wasm_bindgen(js_name = mm2NetHost)]
        fn open(name: &str, public: bool);
        #[wasm_bindgen(js_name = mm2NetJoin)]
        fn join(code: &str);
        #[wasm_bindgen(js_name = mm2NetLeave)]
        pub fn leave();
        /// The host is in the game's lobby: its room opens to others (the page
        /// takes the room back first if it must). `park` is the host leaving the
        /// lobby: the room closes to others but is kept.
        #[wasm_bindgen(js_name = mm2NetEnter)]
        fn enter();
        #[wasm_bindgen(js_name = mm2NetPark)]
        pub fn park();
        /// Lets go of the host's room nobody has entered (it lapses on the relay), so the
        /// player can join or make another.
        #[wasm_bindgen(js_name = mm2NetDetach)]
        fn detach();
        /// Hosting, but not in the lobby yet: no players, nobody can join.
        #[wasm_bindgen(js_name = mm2NetParked)]
        fn parked() -> bool;
        /// Whether this is the room the browser keeps for its host.
        #[wasm_bindgen(js_name = mm2NetOwns)]
        fn owns_room(code: &str) -> bool;
        /// True once if the start page asked for the lobby straight away.
        #[wasm_bindgen(js_name = mm2NetWantLobby)]
        fn want_lobby_page() -> bool;
        /// The room code Join was pressed on at the start page (once): the lobby
        /// joins it, so nobody is counted before they are in the lobby.
        #[wasm_bindgen(js_name = mm2NetPendingJoin)]
        fn pending_join_page() -> String;
        /// A frame to guest `id` (a guest's goes to the host); false once
        /// the relay is gone.
        #[wasm_bindgen(js_name = mm2NetSend)]
        fn send_frame(id: u8, data: &[u8]) -> bool;
        /// The next thing heard, or nothing: a kind byte (1 a player came,
        /// 2 went, 3 a frame, 4 the room is gone), the player's number, and
        /// a frame's bytes.
        #[wasm_bindgen(js_name = mm2NetNext)]
        fn next() -> Vec<u8>;
        /// The public rooms, a line each: code, tab, name, tab, notes, tab,
        /// players.
        #[wasm_bindgen(js_name = mm2NetRooms)]
        fn rooms() -> String;
    }

    pub fn send_bytes(id: u8, body: &[u8]) -> bool {
        send_frame(id, body)
    }

    /// Where this session stands with the page's room.
    #[derive(Default)]
    pub struct Link {
        /// The host has told the lobby the room's code.
        announced: bool,
        /// A guest's Hello, sent once the room is joined.
        hello: Option<Msg>,
        /// A failed attempt has been reported.
        failed: bool,
    }

    /// Whether the start page already put the player in a room: as its
    /// host (true) or a guest (false).
    static WANT: AtomicBool = AtomicBool::new(false);

    /// The start page's "Enter lobby" was pressed: the lobby opens at once.
    pub fn want_lobby() -> bool {
        let want = want_lobby_page();
        if want {
            WANT.store(true, Ordering::Relaxed);
        }
        want
    }

    /// The room the start page's Join was pressed on, once.
    pub fn pending_join() -> Option<String> {
        Some(pending_join_page()).filter(|c| !c.is_empty())
    }

    /// Whether this is the room the browser keeps for its host.
    pub fn owns(code: &str) -> bool {
        owns_room(code)
    }

    /// Whether the start page already put the player in a room, and the
    /// game opens it: as its host (true) or a guest (false). A host's room
    /// the page has not opened yet (parked) is entered only when the start
    /// page asked for it; otherwise it is the host's to pick in the list.
    pub fn in_room() -> Option<bool> {
        let want = WANT.swap(false, Ordering::Relaxed);
        match state().as_str() {
            "hosting" if want || !parked() => Some(true),
            "joined" => Some(false),
            _ => None,
        }
    }

    pub fn room_code() -> String {
        code()
    }

    impl Session {
        /// Hosts a room: the page's, if it made one, or a new private room.
        pub fn host(name: &str, me: Player) -> io::Result<Session> {
            Self::host_as(name, me, false)
        }

        /// Hosts a room, listed in the server browser when `public`: the
        /// page's, if it made one, or a new one.
        pub fn host_as(name: &str, me: Player, public: bool) -> io::Result<Session> {
            match state().as_str() {
                "idle" => open(name, public),
                // The page's room (no name given: the start page made it): it opens now.
                "hosting" if name.is_empty() => {
                    if parked() {
                        enter();
                    }
                }
                // A new room asked for by name, while an unentered one is kept: that one is
                // let go, and this one is made as asked (name, public or private).
                "hosting" if parked() => {
                    detach();
                    open(name, public);
                }
                "connecting" => {}
                _ => {
                    return Err(io::Error::new(io::ErrorKind::AlreadyExists, "in a room"));
                }
            }
            Ok(Session::hosting(me))
        }

        /// Hosts the room the browser keeps for its host (its code and settings are
        /// still there): the page takes it back if it must, and it opens.
        pub fn host_resume(me: Player) -> io::Result<Session> {
            match state().as_str() {
                "idle" | "hosting" | "connecting" => enter(),
                _ => {
                    return Err(io::Error::new(io::ErrorKind::AlreadyExists, "in a room"));
                }
            }
            Ok(Session::hosting(me))
        }

        fn hosting(me: Player) -> Session {
            Session {
                host: true,
                me: 0,
                players: vec![Player { id: 0, ..me }],
                game: None,
                peers: Vec::new(),
                stop: Arc::new(AtomicBool::new(false)),
                web: Link::default(),
            }
        }

        /// Joins the room with this code, or the page's if it joined one.
        pub fn join(addr: Addr, me: Player) -> io::Result<Session> {
            match state().as_str() {
                "idle" => join(&addr),
                // The player's own room nobody has entered: let it go, and join this one.
                "hosting" if parked() => {
                    detach();
                    join(&addr);
                }
                "joined" | "connecting" => {}
                _ => {
                    return Err(io::Error::new(io::ErrorKind::AlreadyExists, "in a room"));
                }
            }
            Ok(Session {
                host: false,
                me: u8::MAX,
                players: Vec::new(),
                game: None,
                peers: vec![(0, Conn)],
                stop: Arc::new(AtomicBool::new(false)),
                web: Link {
                    hello: Some(Msg::Hello {
                        name: me.name.clone(),
                        ride: me.ride.clone(),
                    }),
                    ..Link::default()
                },
            })
        }

        /// The room's progress: its code to the lobby's chat, a guest's
        /// Hello once in, and a failure as the session being lost.
        pub(super) fn web_status(&mut self, out: &mut Vec<Heard>) {
            let state = state();
            if state == "idle" {
                // It never opened, or closed under the lobby. The reason is said once; Lost
                // every time, since a race (which drops it) may be the one listening.
                if !self.web.failed {
                    self.web.failed = true;
                    let why = error();
                    if !why.is_empty() {
                        out.push(Heard::Chat(why));
                    }
                }
                out.push(Heard::Lost);
                return;
            }
            if self.host && !self.web.announced && state == "hosting" {
                self.web.announced = true;
                out.push(Heard::Chat(format!("Room code: {}", code())));
            }
            if !self.host && state == "joined" {
                if let Some(hello) = self.web.hello.take() {
                    let _ = send(&mut Conn, 0, &hello);
                }
            }
        }

        /// The frames and players the relay has passed on.
        pub(super) fn incoming(&mut self) -> Vec<In> {
            let mut out = Vec::new();
            loop {
                let e = next();
                let Some((&kind, rest)) = e.split_first() else {
                    break;
                };
                let id = rest.first().copied().unwrap_or(0);
                match kind {
                    1 => out.push(In::Joined(id, Conn)),
                    2 => out.push(In::Gone(id)),
                    3 => {
                        if let Ok(m) = Msg::decode(rest.get(1..).unwrap_or(&[])) {
                            out.push(In::Got(id, m));
                        }
                    }
                    4 => out.push(if self.host { In::Lost } else { In::Gone(0) }),
                    _ => {}
                }
            }
            out
        }
    }

    /// Looks for rooms: the page's list of public rooms, and any codes
    /// typed in the TCP/IP box.
    pub struct Finder {
        pub found: Vec<Found>,
        /// Room codes typed in.
        pub hosts: Vec<String>,
        seen: String,
    }

    impl Finder {
        pub fn new() -> Finder {
            Finder {
                found: Vec::new(),
                hosts: Vec::new(),
                seen: String::new(),
            }
        }

        /// Whether the list changed.
        pub fn tick(&mut self) -> bool {
            let listed = rooms();
            let key = format!("{listed}\n{}", self.hosts.join(","));
            if key == self.seen {
                return false;
            }
            self.seen = key;
            let mut found: Vec<Found> = self
                .hosts
                .iter()
                .map(|c| {
                    let code: String = c
                        .chars()
                        .filter(|c| c.is_ascii_alphanumeric())
                        .collect::<String>()
                        .to_uppercase();
                    Found {
                        name: format!("Room {code}"),
                        addr: code,
                    }
                })
                .collect();
            for line in listed.lines() {
                let mut f = line.split('\t');
                let (Some(code), Some(name)) = (f.next(), f.next()) else {
                    continue;
                };
                let players = f.nth(1).unwrap_or("");
                if found.iter().any(|r| r.addr == code) {
                    continue;
                }
                found.push(Found {
                    name: if players.is_empty() {
                        name.to_string()
                    } else {
                        format!("{name} ({players})")
                    },
                    addr: code.to_string(),
                });
            }
            self.found = found;
            true
        }
    }
}

#[cfg(target_arch = "wasm32")]
pub use web::{in_room, owns, pending_join, room_code, want_lobby, Finder};

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

#[cfg(all(test, not(target_arch = "wasm32")))]
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
            Msg::Prepare(game.clone()),
            Msg::Loaded(true),
            Msg::Loaded(false),
            Msg::Abort,
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
