//! Decodes a movie's video and writes chosen frames as raw YUV 4:1:0
//! planes (`frames MOVIE.AVI OUT_PREFIX N...`), or the whole stream to one
//! file with `all`, for checking against another decoder.

use std::io::Write;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let bytes = std::fs::read(&args[0]).expect("read the movie");
    let avi = mm2_formats::avi::parse(&bytes).expect("parse the AVI");
    eprintln!(
        "{}x{} {:.2} fps, {} frames, codec {}",
        avi.width,
        avi.height,
        avi.fps,
        avi.frames.len(),
        String::from_utf8_lossy(&avi.codec)
    );
    let mut dec = mm2_indeo::Decoder::new(avi.width as usize, avi.height as usize);
    let mut last: Option<mm2_indeo::Frame> = None;
    let mut out = std::fs::File::create(&args[1]).expect("create the output");
    let all = args.get(2).map(String::as_str) == Some("all");
    let wanted: Vec<usize> = args[2..].iter().filter_map(|a| a.parse().ok()).collect();
    for (i, data) in avi.frames.iter().enumerate() {
        if !data.is_empty() {
            match dec.decode(data) {
                Ok(Some(f)) => last = Some(f),
                Ok(None) => {}
                Err(e) => eprintln!("frame {i}: {e}"),
            }
        }
        if all || wanted.contains(&i) {
            if let Some(f) = &last {
                out.write_all(&f.y).unwrap();
                out.write_all(&f.u).unwrap();
                out.write_all(&f.v).unwrap();
            }
        }
    }
}
