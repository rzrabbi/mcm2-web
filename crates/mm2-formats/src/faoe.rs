//! `FAOE` containers.
//!
//! Most `.RES` packs and every `.ENV` track are an RS2 archive run through a
//! byte-stream cipher (the game's `ResourceManager` file class):
//!
//! ```text
//! "FAOE"
//! ciphertext   // decodes to an RS2 archive; offsets count from here
//! ```
//!
//! Each plaintext byte is `key ^ c`, after which the key advances to
//! `key + c` (wrapping), where `c` is the ciphertext byte. The starting key
//! is a hash of the file's name; see [`key_for_name`].
//!
//! The game resets the key to the starting key whenever it seeks, so the
//! archive header decodes as one stream from the start, and each entry
//! decodes as its own stream from its offset.

use crate::{rs2, Error, Result};

pub const MAGIC: &[u8] = b"FAOE";

/// Starting key for a file, hashed from its name the way the game does.
///
/// The game passes lower-case names such as `bike.res` or `tag01.env`. The
/// hash works out to depend only on the extension, but this follows the
/// original over the whole name.
pub fn key_for_name(file_name: &str) -> u8 {
    file_name
        .to_ascii_lowercase()
        .bytes()
        .fold(0xfa, |k: u8, c| (k.wrapping_add(3) ^ c).wrapping_add(k))
}

/// Decodes a `FAOE` file into the RS2 archive it holds. `file_name` is
/// the file's name without directories, e.g. `BIKE.RES`.
pub fn decode(bytes: &[u8], file_name: &str) -> Result<Vec<u8>> {
    let body = bytes.strip_prefix(MAGIC).ok_or(Error::BadMagic)?;
    let key = key_for_name(file_name);
    let mut plain = body.to_vec();
    cipher(&mut plain, key);
    let ranges: Vec<_> = rs2::Archive::parse(&plain)?
        .entries()
        .iter()
        .map(|e| e.offset as usize..(e.offset + e.size) as usize)
        .collect();
    for range in ranges {
        plain[range.clone()].copy_from_slice(&body[range.clone()]);
        cipher(&mut plain[range], key);
    }
    Ok(plain)
}

/// Decodes `buf` in place as one stream starting from `key`.
fn cipher(buf: &mut [u8], mut key: u8) {
    for b in buf {
        let c = *b;
        *b = key ^ c;
        key = key.wrapping_add(c);
    }
}

/// Encodes an RS2 archive as a `FAOE` file. The inverse of [`decode`].
pub fn encode(plain: &[u8], file_name: &str) -> Result<Vec<u8>> {
    let key = key_for_name(file_name);
    let encode_stream = |out: &mut [u8], plain: &[u8]| {
        let mut k = key;
        for (o, &p) in out.iter_mut().zip(plain) {
            *o = k ^ p;
            k = k.wrapping_add(*o);
        }
    };
    let mut body = vec![0; plain.len()];
    encode_stream(&mut body, plain);
    for e in rs2::Archive::parse(plain)?.entries() {
        let range = e.offset as usize..(e.offset + e.size) as usize;
        encode_stream(&mut body[range.clone()], &plain[range]);
    }
    let mut out = MAGIC.to_vec();
    out.extend_from_slice(&body);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_match_the_shipped_files() {
        assert_eq!(key_for_name("uires.res"), 0x80);
        assert_eq!(key_for_name("BIKE.RES"), 0x80);
        assert_eq!(key_for_name("tag01.env"), 0x73);
    }

    #[test]
    fn round_trip() {
        let plain = rs2::build(&[("a.ini", b"[x]\n"), ("b.ini", b"[x]\n")]);
        let enc = encode(&plain, "x.res").unwrap();
        assert!(enc.starts_with(MAGIC));
        assert_ne!(&enc[4..], &plain[..]);
        assert_eq!(decode(&enc, "X.RES").unwrap(), plain);
        // Each entry is its own stream, so equal entries encode equally.
        let ar = rs2::Archive::parse(&plain).unwrap();
        let [a, b] = ar.entries() else { panic!() };
        let at = |e: &rs2::Entry| &enc[4 + e.offset as usize..][..e.size as usize];
        assert_eq!(at(a), at(b));
    }

    #[test]
    fn rejects_wrong_magic() {
        assert_eq!(decode(b"RS2\0", "a.res").unwrap_err(), Error::BadMagic);
    }
}
