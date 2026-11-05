//! Development file server: serves a local Motocross Madness 2 install (and an
//! optional built web page) to the browser build over HTTP/1.1 keep-alive.
//!
//!     cargo run --release --bin serve-game -- "<game folder>" [--port 8081] [--with TERAFORM/...] [--page dist]
//!
//! `GET /manifest.json` lists the files to load up front, `GET /remote.json` the
//! `(path, size)` pairs of the track files the page fetches when an event is
//! opened; any other path serves that file from the install, read-only. Only
//! `127.0.0.1` is bound.

use std::collections::BTreeSet;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Component, Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

const SKIP: &[&str] = &[
    "pdf", "hlp", "exe", "dll", "rtf", "txt", "icd", "cur", "016", "256", "sem",
];
const BASE: &[&str] = &["UI", "RES", "BIKES", "RIDERS"];
/// The menus' text lives in the string tables of these two libraries.
const ROOT_FILES: &[&str] = &["UILANG.DLL", "LANG.DLL"];
/// Fetched on demand, by event.
const REMOTE: &[&str] = &["TERAFORM"];

const USAGE: &str = "\
usage:
  serve-game <game-dir> [--port 8081] [--with <folder>]... [--page <dist-dir>]";

#[derive(Debug, Default)]
pub struct Config {
    pub game: PathBuf,
    pub port: u16,
    pub extra: Vec<String>,
    pub page: Option<PathBuf>,
}

pub fn parse_args(args: &[String]) -> Result<Config, String> {
    let mut game: Option<PathBuf> = None;
    let mut port: u16 = 8081;
    let mut extra = Vec::new();
    let mut page = None;
    let mut i = 0;
    while i < args.len() {
        let a = args[i].as_str();
        match a {
            "-h" | "--help" => return Err(USAGE.to_string()),
            "--port" => {
                i += 1;
                let val = args.get(i).ok_or("--port requires a number")?;
                port = val.parse().map_err(|_| format!("invalid port: {val}"))?;
            }
            "--with" => {
                i += 1;
                let val = args.get(i).ok_or("--with requires a folder")?;
                extra.push(val.clone());
            }
            "--page" => {
                i += 1;
                let val = args.get(i).ok_or("--page requires a folder")?;
                page = Some(PathBuf::from(val));
            }
            _ if a.starts_with("--port=") => {
                let val = &a["--port=".len()..];
                port = val.parse().map_err(|_| format!("invalid port: {val}"))?;
            }
            _ if a.starts_with("--with=") => {
                extra.push(a["--with=".len()..].to_string());
            }
            _ if a.starts_with("--page=") => {
                page = Some(PathBuf::from(&a["--page=".len()..]));
            }
            _ if a.starts_with('-') => {
                return Err(format!("unknown option: {a}\n{USAGE}"));
            }
            _ => {
                if game.is_some() {
                    return Err(format!("unexpected argument: {a}\n{USAGE}"));
                }
                game = Some(PathBuf::from(a));
            }
        }
        i += 1;
    }
    let game = game.ok_or_else(|| USAGE.to_string())?;
    Ok(Config {
        game,
        port,
        extra,
        page,
    })
}

fn should_skip(file_name: &str) -> bool {
    if file_name.eq_ignore_ascii_case("PCNAMES.TXT") {
        return false;
    }
    match Path::new(file_name).extension().and_then(|e| e.to_str()) {
        Some(ext) => SKIP.iter().any(|s| s.eq_ignore_ascii_case(ext)),
        None => false,
    }
}

fn should_skip_remote(file_name: &str) -> bool {
    match Path::new(file_name).extension().and_then(|e| e.to_str()) {
        Some(ext) => SKIP.iter().any(|s| s.eq_ignore_ascii_case(ext)),
        None => false,
    }
}

/// Resolves `rel` inside `base` case-insensitively so installs work on
/// case-sensitive filesystems as well as Windows.
fn resolve_case_insensitive(base: &Path, rel: &str) -> Option<PathBuf> {
    let direct = base.join(rel);
    if direct.exists() {
        return Some(direct);
    }
    let mut cur = base.to_path_buf();
    for part in rel.split(['/', '\\']).filter(|s| !s.is_empty()) {
        let entries = std::fs::read_dir(&cur).ok()?;
        let found = entries
            .filter_map(Result::ok)
            .find(|e| e.file_name().to_string_lossy().eq_ignore_ascii_case(part))?;
        cur = found.path();
    }
    cur.exists().then_some(cur)
}

fn walk_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut items: Vec<PathBuf> = entries.filter_map(|e| e.ok().map(|e| e.path())).collect();
    items.sort();
    for path in items {
        if path.is_dir() {
            walk_files(&path, out);
        } else if path.is_file() {
            out.push(path);
        }
    }
}

fn rel_slash(root: &Path, path: &Path) -> Option<String> {
    let rel = path.strip_prefix(root).ok()?;
    let parts: Vec<&str> = rel.iter().filter_map(|c| c.to_str()).collect();
    Some(parts.join("/"))
}

pub fn build_manifest_and_remote(root: &Path, extra: &[String]) -> (Vec<String>, Vec<(String, u64)>) {
    let mut eager_set = BTreeSet::new();
    for &f in ROOT_FILES {
        if let Some(p) = resolve_case_insensitive(root, f) {
            if p.is_file() {
                if let Some(r) = rel_slash(root, &p) {
                    eager_set.insert(r);
                }
            }
        }
    }
    let tops: Vec<&str> = BASE
        .iter()
        .copied()
        .chain(extra.iter().map(String::as_str))
        .collect();
    for top in tops {
        let Some(dir) = resolve_case_insensitive(root, top) else {
            continue;
        };
        let mut files = Vec::new();
        walk_files(&dir, &mut files);
        for f in files {
            let Some(name) = f.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if !should_skip(name) {
                if let Some(r) = rel_slash(root, &f) {
                    eager_set.insert(r);
                }
            }
        }
    }

    let mut remote_list = Vec::new();
    for &top in REMOTE {
        let Some(dir) = resolve_case_insensitive(root, top) else {
            continue;
        };
        let mut files = Vec::new();
        walk_files(&dir, &mut files);
        for f in files {
            let Some(name) = f.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if should_skip_remote(name) {
                continue;
            }
            if let Some(r) = rel_slash(root, &f) {
                if !eager_set.contains(&r) {
                    let size = std::fs::metadata(&f).map(|m| m.len()).unwrap_or(0);
                    remote_list.push((r, size));
                }
            }
        }
    }
    remote_list.sort_by(|a, b| a.0.cmp(&b.0));
    remote_list.dedup_by(|a, b| a.0 == b.0);

    (eager_set.into_iter().collect(), remote_list)
}

fn json_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

pub fn encode_manifest_json(manifest: &[String]) -> String {
    let items: Vec<String> = manifest.iter().map(|s| json_escape(s)).collect();
    format!("[{}]", items.join(","))
}

pub fn encode_remote_json(remote: &[(String, u64)]) -> String {
    let items: Vec<String> = remote
        .iter()
        .map(|(p, sz)| format!("[{},{}]", json_escape(p), sz))
        .collect();
    format!("[{}]", items.join(","))
}

pub fn percent_decode(input: &str) -> Option<String> {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            if i + 2 >= bytes.len() {
                return None;
            }
            let hi = (bytes[i + 1] as char).to_digit(16)?;
            let lo = (bytes[i + 2] as char).to_digit(16)?;
            out.push(((hi << 4) | lo) as u8);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// Cleans a URL path into a safe relative path without parent traversal.
fn clean_relative_path(url_path: &str) -> Option<PathBuf> {
    let path_only = url_path.split('?').next().unwrap_or(url_path);
    let decoded = percent_decode(path_only)?;
    let trimmed = decoded.trim_start_matches('/');
    let mut out = PathBuf::new();
    for comp in Path::new(trimmed).components() {
        match comp {
            Component::Normal(s) => out.push(s),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => return None,
        }
    }
    Some(out)
}

fn content_type(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .as_deref()
    {
        Some("html" | "htm") => "text/html; charset=utf-8",
        Some("js" | "mjs") => "text/javascript; charset=utf-8",
        Some("wasm") => "application/wasm",
        Some("css") => "text/css; charset=utf-8",
        Some("json") => "application/json",
        Some("png") => "image/png",
        Some("svg") => "image/svg+xml",
        Some("ico") => "image/x-icon",
        Some("ttf") => "font/ttf",
        Some("txt") => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

struct ServerState {
    root: PathBuf,
    page: Option<PathBuf>,
    manifest_json: Vec<u8>,
    remote_json: Vec<u8>,
}

impl ServerState {
    fn new(cfg: &Config) -> Result<(Self, usize), String> {
        let root = std::fs::canonicalize(&cfg.game)
            .map_err(|e| format!("{}: {e}", cfg.game.display()))?;
        let page = match &cfg.page {
            Some(p) => Some(
                std::fs::canonicalize(p).map_err(|e| format!("{}: {e}", p.display()))?,
            ),
            None => None,
        };
        let (manifest, remote) = build_manifest_and_remote(&root, &cfg.extra);
        let count = manifest.len();
        let manifest_json = encode_manifest_json(&manifest).into_bytes();
        let remote_json = encode_remote_json(&remote).into_bytes();
        Ok((
            Self {
                root,
                page,
                manifest_json,
                remote_json,
            },
            count,
        ))
    }

    fn resolve_file(&self, rel: &Path) -> Option<PathBuf> {
        let rel_str = rel.to_string_lossy();
        // The built page wins over the install, which has no index of its own.
        if let Some(page_root) = &self.page {
            let candidate = page_root.join(rel);
            if candidate.is_file() {
                return Some(candidate);
            }
            if candidate.is_dir() {
                let idx = candidate.join("index.html");
                if idx.is_file() {
                    return Some(idx);
                }
            }
        }
        if rel_str.is_empty() {
            return None;
        }
        let target = resolve_case_insensitive(&self.root, &rel_str)?;
        target.is_file().then_some(target)
    }
}

fn send_response(
    stream: &mut TcpStream,
    status: &str,
    ctype: &str,
    body: &[u8],
    head_only: bool,
    keep_alive: bool,
) -> std::io::Result<()> {
    let conn = if keep_alive { "keep-alive" } else { "close" };
    let header = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nAccess-Control-Allow-Origin: *\r\nConnection: {conn}\r\n\r\n",
        body.len()
    );
    stream.write_all(header.as_bytes())?;
    if !head_only && !body.is_empty() {
        stream.write_all(body)?;
    }
    stream.flush()
}

fn handle_client(mut stream: TcpStream, state: &ServerState) {
    let _ = stream.set_nodelay(true);
    let _ = stream.set_read_timeout(Some(Duration::from_secs(30)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(30)));

    let Ok(reader_stream) = stream.try_clone() else {
        return;
    };
    let mut reader = BufReader::new(reader_stream);
    let mut line = String::new();

    loop {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 2 {
            break;
        }
        let method = parts[0];
        let raw_path = parts[1];
        let version = parts.get(2).copied().unwrap_or("HTTP/1.0");
        let mut keep_alive = version.eq_ignore_ascii_case("HTTP/1.1");

        loop {
            let mut hdr = String::new();
            match reader.read_line(&mut hdr) {
                Ok(0) | Err(_) => return,
                Ok(_) => {
                    if hdr == "\r\n" || hdr == "\n" {
                        break;
                    }
                    if let Some((k, v)) = hdr.split_once(':') {
                        if k.trim().eq_ignore_ascii_case("Connection") {
                            let v = v.trim();
                            if v.eq_ignore_ascii_case("close") {
                                keep_alive = false;
                            } else if v.eq_ignore_ascii_case("keep-alive") {
                                keep_alive = true;
                            }
                        }
                    }
                }
            }
        }

        let head_only = method.eq_ignore_ascii_case("HEAD");
        if !method.eq_ignore_ascii_case("GET") && !head_only {
            let _ = send_response(&mut stream, "405 Method Not Allowed", "text/plain", b"method not allowed\n", head_only, false);
            break;
        }

        let path_only = raw_path.split('?').next().unwrap_or(raw_path);
        let res = if let Some(name) = path_only.strip_prefix("/__fonts/") {
            // The four Arial faces the game's text uses, from this machine's
            // Windows fonts; for local development only.
            let allowed = matches!(
                name.to_ascii_lowercase().as_str(),
                "arial.ttf" | "arialbd.ttf" | "arialbi.ttf" | "ariali.ttf"
            );
            let windir = std::env::var("WINDIR").unwrap_or_else(|_| "C:\\Windows".to_string());
            let font_path = Path::new(&windir).join("Fonts").join(name);
            if allowed && font_path.is_file() {
                match std::fs::read(&font_path) {
                    Ok(bytes) => send_response(&mut stream, "200 OK", "font/ttf", &bytes, head_only, keep_alive),
                    Err(_) => send_response(&mut stream, "404 Not Found", "text/plain", b"not found\n", head_only, keep_alive),
                }
            } else {
                send_response(&mut stream, "404 Not Found", "text/plain", b"not found\n", head_only, keep_alive)
            }
        } else if path_only == "/manifest.json" {
            send_response(
                &mut stream,
                "200 OK",
                "application/json",
                &state.manifest_json,
                head_only,
                keep_alive,
            )
        } else if path_only == "/remote.json" {
            send_response(
                &mut stream,
                "200 OK",
                "application/json",
                &state.remote_json,
                head_only,
                keep_alive,
            )
        } else if let Some(rel) = clean_relative_path(path_only) {
            if let Some(file_path) = state.resolve_file(&rel) {
                match std::fs::read(&file_path) {
                    Ok(bytes) => send_response(
                        &mut stream,
                        "200 OK",
                        content_type(&file_path),
                        &bytes,
                        head_only,
                        keep_alive,
                    ),
                    Err(_) => send_response(&mut stream, "404 Not Found", "text/plain", b"not found\n", head_only, keep_alive),
                }
            } else {
                send_response(&mut stream, "404 Not Found", "text/plain", b"not found\n", head_only, keep_alive)
            }
        } else {
            send_response(&mut stream, "400 Bad Request", "text/plain", b"bad request\n", head_only, keep_alive)
        };

        if res.is_err() || !keep_alive {
            break;
        }
    }
}

pub fn run(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let cfg = parse_args(args)?;
    let (state, count) = ServerState::new(&cfg)?;
    let listener = TcpListener::bind(("127.0.0.1", cfg.port))?;
    println!(
        "serving {} on http://127.0.0.1:{} ({count} files)",
        state.root.display(),
        cfg.port
    );
    let state = Arc::new(state);
    for conn in listener.incoming().flatten() {
        let st = Arc::clone(&state);
        std::thread::spawn(move || handle_client(conn, &st));
    }
    Ok(())
}

#[allow(dead_code)]
fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    #[test]
    fn manifest_and_remote_filtering_and_keep_alive() {
        let tmp = std::env::temp_dir().join(format!("mm2_serve_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(tmp.join("UI")).unwrap();
        std::fs::create_dir_all(tmp.join("RES")).unwrap();
        std::fs::create_dir_all(tmp.join("TERAFORM/NAT")).unwrap();

        std::fs::write(tmp.join("UILANG.DLL"), b"dll-ok").unwrap();
        std::fs::write(tmp.join("MCM2.EXE"), b"skip-exe").unwrap();
        std::fs::write(tmp.join("UI/MENU.DAT"), b"menu").unwrap();
        std::fs::write(tmp.join("UI/PCNAMES.TXT"), b"names").unwrap();
        std::fs::write(tmp.join("UI/README.TXT"), b"skip-txt").unwrap();
        std::fs::write(tmp.join("TERAFORM/NAT/TRACK.ENV"), b"trackdata").unwrap();
        std::fs::write(tmp.join("TERAFORM/NAT/SKIP.EXE"), b"skip").unwrap();

        let (manifest, remote) = build_manifest_and_remote(&tmp, &[]);
        assert_eq!(manifest, vec!["UI/MENU.DAT", "UI/PCNAMES.TXT", "UILANG.DLL"]);
        assert_eq!(remote, vec![("TERAFORM/NAT/TRACK.ENV".to_string(), 9)]);

        // With --with TERAFORM/NAT, TRACK.ENV moves to manifest and leaves remote.
        let (manifest2, remote2) = build_manifest_and_remote(&tmp, &["TERAFORM/NAT".to_string()]);
        assert!(manifest2.contains(&"TERAFORM/NAT/TRACK.ENV".to_string()));
        assert!(remote2.is_empty());

        // Test HTTP/1.1 keep-alive serving multiple requests over a single socket.
        let cfg = Config {
            game: tmp.clone(),
            port: 0,
            extra: Vec::new(),
            page: None,
        };
        let (state, _) = ServerState::new(&cfg).unwrap();
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let state = Arc::new(state);
        std::thread::spawn(move || {
            if let Ok((stream, _)) = listener.accept() {
                handle_client(stream, &state);
            }
        });

        let mut client = TcpStream::connect(("127.0.0.1", port)).unwrap();
        client
            .write_all(
                b"GET /manifest.json HTTP/1.1\r\nHost: localhost\r\n\r\nGET /UI/MENU.DAT HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
            )
            .unwrap();
        let mut resp = String::new();
        client.read_to_string(&mut resp).unwrap();
        assert!(resp.contains("[\"UI/MENU.DAT\",\"UI/PCNAMES.TXT\",\"UILANG.DLL\"]"));
        assert!(resp.ends_with("menu"));

        let _ = std::fs::remove_dir_all(&tmp);
    }
}

