//! Writes a movie's sound as raw interleaved 16-bit samples.
fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let bytes = std::fs::read(&args[0]).unwrap();
    let avi = mm2_formats::avi::parse(&bytes).unwrap();
    let a = avi.audio.expect("the movie has sound");
    eprintln!(
        "format {} {} ch {} Hz, {} chunks",
        a.format,
        a.channels,
        a.rate,
        a.chunks.len()
    );
    let s: Vec<u8> = a.samples().iter().flat_map(|v| v.to_le_bytes()).collect();
    std::fs::write(&args[1], s).unwrap();
}
