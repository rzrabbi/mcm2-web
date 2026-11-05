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
use std::fs::{File, Metadata};
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Component, Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, UNIX_EPOCH};

const MAX_LINE_BYTES: usize = 4096;
const MAX_HEADERS: usize = 64;
const MAX_CONCURRENT_CONNS: usize = 256;

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
        Some("woff2") => "font/woff2",
        Some("ttf") => "font/ttf",
        Some("txt") => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RangeSpec {
    Full,
    Unsatisfiable,
    Partial { start: u64, end: u64 },
}

pub fn parse_range(header: &str, size: u64) -> RangeSpec {
    let trimmed = header.trim();
    let Some(rest) = trimmed.strip_prefix("bytes=") else {
        return RangeSpec::Full;
    };
    if rest.contains(',') {
        return RangeSpec::Full;
    }
    let Some((a, b)) = rest.split_once('-') else {
        return RangeSpec::Full;
    };
    if a.is_empty() && b.is_empty() {
        return RangeSpec::Full;
    }
    if !a.bytes().all(|c| c.is_ascii_digit()) || !b.bytes().all(|c| c.is_ascii_digit()) {
        return RangeSpec::Full;
    }
    if a.is_empty() {
        let Ok(n) = b.parse::<u64>() else {
            return RangeSpec::Full;
        };
        if n == 0 || size == 0 {
            return RangeSpec::Unsatisfiable;
        }
        let start = size.saturating_sub(n);
        return RangeSpec::Partial {
            start,
            end: size - 1,
        };
    }
    let Ok(start) = a.parse::<u64>() else {
        return RangeSpec::Full;
    };
    let end = if b.is_empty() {
        if size == 0 {
            return RangeSpec::Unsatisfiable;
        }
        size - 1
    } else {
        let Ok(parsed_end) = b.parse::<u64>() else {
            return RangeSpec::Full;
        };
        if size == 0 {
            return RangeSpec::Unsatisfiable;
        }
        parsed_end.min(size - 1)
    };
    if start >= size || end < start {
        return RangeSpec::Unsatisfiable;
    }
    RangeSpec::Partial { start, end }
}

fn file_etag(meta: &Metadata) -> String {
    let len = meta.len();
    let (secs, nanos) = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| (d.as_secs(), d.subsec_nanos()))
        .unwrap_or((0, 0));
    format!("\"{len:x}-{secs:x}-{nanos:x}\"")
}

fn etag_matches(if_none_match: &str, etag: &str) -> bool {
    let trimmed = if_none_match.trim();
    if trimmed == "*" {
        return true;
    }
    let clean_target = etag.strip_prefix("W/").unwrap_or(etag);
    trimmed.split(',').any(|part| {
        let p = part.trim();
        let clean_p = p.strip_prefix("W/").unwrap_or(p);
        clean_p == clean_target
    })
}

fn read_bounded_line<R: BufRead>(
    reader: &mut R,
    out: &mut String,
    max_bytes: usize,
) -> std::io::Result<usize> {
    out.clear();
    let mut limited = reader.take((max_bytes + 1) as u64);
    let n = limited.read_line(out)?;
    if n > max_bytes && !out.ends_with('\n') {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "HTTP line exceeds maximum length",
        ));
    }
    Ok(n)
}

struct ServerState {
    root: PathBuf,
    page: Option<PathBuf>,
    manifest_json: Vec<u8>,
    remote_json: Vec<u8>,
    active_conns: AtomicUsize,
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
                active_conns: AtomicUsize::new(0),
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

struct ConnGuard<'a>(&'a AtomicUsize);

impl Drop for ConnGuard<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Relaxed);
    }
}

fn send_response_with_headers(
    stream: &mut TcpStream,
    status: &str,
    ctype: &str,
    extra_headers: &[(&str, &str)],
    body: &[u8],
    head_only: bool,
    keep_alive: bool,
) -> std::io::Result<()> {
    let conn = if keep_alive { "keep-alive" } else { "close" };
    let mut header = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nAccess-Control-Allow-Origin: *\r\n",
        body.len()
    );
    for (k, v) in extra_headers {
        header.push_str(&format!("{k}: {v}\r\n"));
    }
    header.push_str(&format!("Connection: {conn}\r\n\r\n"));
    stream.write_all(header.as_bytes())?;
    if !head_only && !body.is_empty() {
        stream.write_all(body)?;
    }
    stream.flush()
}

fn send_response(
    stream: &mut TcpStream,
    status: &str,
    ctype: &str,
    body: &[u8],
    head_only: bool,
    keep_alive: bool,
) -> std::io::Result<()> {
    send_response_with_headers(stream, status, ctype, &[], body, head_only, keep_alive)
}

fn stream_file_slice(
    stream: &mut TcpStream,
    status: &str,
    ctype: &str,
    extra_headers: &[(&str, String)],
    mut file: File,
    start: u64,
    length: u64,
    head_only: bool,
    keep_alive: bool,
) -> std::io::Result<()> {
    if start > 0 {
        file.seek(SeekFrom::Start(start))?;
    }
    let conn = if keep_alive { "keep-alive" } else { "close" };
    let mut header = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {ctype}\r\nContent-Length: {length}\r\nAccess-Control-Allow-Origin: *\r\n"
    );
    for (k, v) in extra_headers {
        header.push_str(&format!("{k}: {v}\r\n"));
    }
    header.push_str(&format!("Connection: {conn}\r\n\r\n"));
    stream.write_all(header.as_bytes())?;
    if !head_only && length > 0 {
        let mut take = file.take(length);
        let mut buf = [0u8; 65536];
        loop {
            let n = take.read(&mut buf)?;
            if n == 0 {
                break;
            }
            stream.write_all(&buf[..n])?;
        }
    }
    stream.flush()
}

fn handle_client(mut stream: TcpStream, state: &ServerState) {
    let _ = stream.set_nodelay(true);
    let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(30)));

    let prev = state.active_conns.fetch_add(1, Ordering::Relaxed);
    let _guard = ConnGuard(&state.active_conns);
    if prev >= MAX_CONCURRENT_CONNS {
        let _ = send_response(
            &mut stream,
            "503 Service Unavailable",
            "text/plain",
            b"server busy\n",
            false,
            false,
        );
        return;
    }

    let Ok(reader_stream) = stream.try_clone() else {
        return;
    };
    let mut reader = BufReader::new(reader_stream);
    let mut line = String::new();
    let mut hdr = String::new();

    loop {
        match read_bounded_line(&mut reader, &mut line, MAX_LINE_BYTES) {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 2 {
            break;
        }
        let method = parts[0].to_string();
        let raw_path = parts[1].to_string();
        let version = parts.get(2).copied().unwrap_or("HTTP/1.0");
        let mut keep_alive = version.eq_ignore_ascii_case("HTTP/1.1");
        let mut range_header: Option<String> = None;
        let mut if_none_match: Option<String> = None;
        let mut if_range: Option<String> = None;

        let mut hdr_count = 0usize;
        loop {
            match read_bounded_line(&mut reader, &mut hdr, MAX_LINE_BYTES) {
                Ok(0) | Err(_) => return,
                Ok(_) => {
                    if hdr == "\r\n" || hdr == "\n" {
                        break;
                    }
                    hdr_count += 1;
                    if hdr_count > MAX_HEADERS {
                        let _ = send_response(
                            &mut stream,
                            "431 Request Header Fields Too Large",
                            "text/plain",
                            b"too many headers\n",
                            false,
                            false,
                        );
                        return;
                    }
                    if let Some((k, v)) = hdr.split_once(':') {
                        let k = k.trim();
                        let v = v.trim();
                        if k.eq_ignore_ascii_case("Connection") {
                            if v.eq_ignore_ascii_case("close") {
                                keep_alive = false;
                            } else if v.eq_ignore_ascii_case("keep-alive") {
                                keep_alive = true;
                            }
                        } else if k.eq_ignore_ascii_case("Range") {
                            range_header = Some(v.to_string());
                        } else if k.eq_ignore_ascii_case("If-None-Match") {
                            if_none_match = Some(v.to_string());
                        } else if k.eq_ignore_ascii_case("If-Range") {
                            if_range = Some(v.to_string());
                        }
                    }
                }
            }
        }

        let head_only = method.eq_ignore_ascii_case("HEAD");
        if !method.eq_ignore_ascii_case("GET") && !head_only {
            let _ = send_response(
                &mut stream,
                "405 Method Not Allowed",
                "text/plain",
                b"method not allowed\n",
                head_only,
                false,
            );
            break;
        }

        let path_only = raw_path.split('?').next().unwrap_or(&raw_path);
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
                let ctype = content_type(&file_path);
                match File::open(&file_path).and_then(|f| {
                    let meta = f.metadata()?;
                    Ok((f, meta))
                }) {
                    Ok((file, meta)) => {
                        let size = meta.len();
                        let etag = file_etag(&meta);
                        let effective_range = match (&range_header, &if_range) {
                            (Some(r), Some(ir)) if !etag_matches(ir, &etag) => None,
                            (Some(r), _) => Some(r.as_str()),
                            (None, _) => None,
                        };
                        if effective_range.is_none()
                            && if_none_match
                                .as_deref()
                                .is_some_and(|inm| etag_matches(inm, &etag))
                        {
                            send_response_with_headers(
                                &mut stream,
                                "304 Not Modified",
                                ctype,
                                &[("ETag", &etag)],
                                b"",
                                true,
                                keep_alive,
                            )
                        } else {
                            match effective_range.map(|h| parse_range(h, size)) {
                                Some(RangeSpec::Partial { start, end }) => {
                                    let length = end - start + 1;
                                    stream_file_slice(
                                        &mut stream,
                                        "206 Partial Content",
                                        ctype,
                                        &[
                                            ("Content-Range", format!("bytes {start}-{end}/{size}")),
                                            ("Accept-Ranges", "bytes".to_string()),
                                            ("ETag", etag),
                                        ],
                                        file,
                                        start,
                                        length,
                                        head_only,
                                        keep_alive,
                                    )
                                }
                                Some(RangeSpec::Unsatisfiable) => send_response_with_headers(
                                    &mut stream,
                                    "416 Range Not Satisfiable",
                                    ctype,
                                    &[("Content-Range", &format!("bytes */{size}"))],
                                    b"",
                                    true,
                                    keep_alive,
                                ),
                                None | Some(RangeSpec::Full) => stream_file_slice(
                                    &mut stream,
                                    "200 OK",
                                    ctype,
                                    &[
                                        ("Accept-Ranges", "bytes".to_string()),
                                        ("ETag", etag),
                                    ],
                                    file,
                                    0,
                                    size,
                                    head_only,
                                    keep_alive,
                                ),
                            }
                        }
                    }
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
        let _ = std::thread::Builder::new()
            .name("mm2-serve".to_string())
            .stack_size(256 * 1024)
            .spawn(move || handle_client(conn, &st));
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

    #[test]
    fn range_and_etag_revalidation() {
        let tmp = std::env::temp_dir().join(format!("mm2_serve_range_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(tmp.join("TERAFORM/SX")).unwrap();
        let data: Vec<u8> = (0..=255u8).cycle().take(4096).collect();
        std::fs::write(tmp.join("TERAFORM/SX/SX01.ENV"), &data).unwrap();

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
            for conn in listener.incoming().flatten() {
                let st = Arc::clone(&state);
                std::thread::spawn(move || handle_client(conn, &st));
            }
        });

        let get = |hdrs: &str| -> (String, Vec<u8>) {
            let mut c = TcpStream::connect(("127.0.0.1", port)).unwrap();
            let req = format!(
                "GET /TERAFORM/SX/SX01.ENV HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n{hdrs}\r\n"
            );
            c.write_all(req.as_bytes()).unwrap();
            let mut raw = Vec::new();
            c.read_to_end(&mut raw).unwrap();
            let sep = raw.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
            (
                String::from_utf8_lossy(&raw[..sep]).to_string(),
                raw[sep + 4..].to_vec(),
            )
        };

        let (hdr, body) = get("Range: bytes=10-19\r\n");
        assert!(hdr.starts_with("HTTP/1.1 206 Partial Content"));
        assert!(hdr.contains("Content-Range: bytes 10-19/4096"));
        assert_eq!(body, &data[10..20]);

        let (hdr416, _) = get("Range: bytes=9000-9010\r\n");
        assert!(hdr416.starts_with("HTTP/1.1 416 Range Not Satisfiable"));

        let (hdr200, _) = get("");
        let etag = hdr200
            .lines()
            .find_map(|l| l.strip_prefix("ETag: "))
            .unwrap()
            .trim()
            .to_string();
        let (hdr304, body304) = get(&format!("If-None-Match: {etag}\r\n"));
        assert!(hdr304.starts_with("HTTP/1.1 304 Not Modified"));
        assert!(body304.is_empty());

        let _ = std::fs::remove_dir_all(&tmp);
    }
}

