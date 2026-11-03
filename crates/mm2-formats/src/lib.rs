//! Readers for Motocross Madness 2 data files.
//!
//! The game ships its data in two container formats, told apart by the
//! first four bytes:
//!
//! * `RS2\0` – an uncompressed archive of named files ([`rs2`]).
//! * `FAOE` – an RS2 archive behind a simple byte cipher, used by most
//!   `.RES` packs and every `.ENV` track ([`faoe`]).

pub mod anim;
pub mod avi;
pub mod col;
pub mod cub;
pub mod dtm;
pub mod esb;
pub mod faoe;

pub mod font;
pub mod hires;
pub mod lzw;
pub mod pe;
mod reader;
pub mod rs2;
pub mod scn;
pub mod slb;
pub mod slt;
pub mod tdf;
pub mod texture;
pub mod tga;
pub mod trn;
pub mod vcr;
pub mod wav;

use std::fmt;

/// The container format of a data file, detected from its magic bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Container {
    Rs2,
    Faoe,
    Unknown,
}

impl Container {
    pub fn detect(bytes: &[u8]) -> Self {
        match bytes.get(..4) {
            Some(rs2::MAGIC) => Container::Rs2,
            Some(faoe::MAGIC) => Container::Faoe,
            _ => Container::Unknown,
        }
    }
}

impl fmt::Display for Container {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Container::Rs2 => "RS2",
            Container::Faoe => "FAOE",
            Container::Unknown => "unknown",
        })
    }
}

/// Errors from parsing a data file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    BadMagic,
    Truncated {
        what: &'static str,
        offset: usize,
    },
    EntryOutOfBounds {
        name: String,
        offset: u32,
        size: u32,
    },
    BadName {
        offset: usize,
    },
    Corrupt(&'static str),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::BadMagic => write!(f, "file does not start with the expected magic"),
            Error::Truncated { what, offset } => {
                write!(
                    f,
                    "file ends early while reading {what} at offset {offset:#x}"
                )
            }
            Error::EntryOutOfBounds { name, offset, size } => write!(
                f,
                "entry {name:?} ({size} bytes at {offset:#x}) runs past the end of the file"
            ),
            Error::BadName { offset } => write!(f, "entry name at offset {offset:#x} is not valid"),
            Error::Corrupt(what) => write!(f, "corrupt data: {what}"),
        }
    }
}

impl std::error::Error for Error {}

pub type Result<T> = std::result::Result<T, Error>;
