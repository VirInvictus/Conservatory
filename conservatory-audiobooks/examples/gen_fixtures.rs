//! Generate the committed audiobook reader fixtures (Phase 7a-ii).
//!
//! Run once with `cargo run -p conservatory-audiobooks --example gen_fixtures`;
//! the small outputs under `tests/fixtures/` are committed so CI stays hermetic
//! (the `gen_audio_fixtures` precedent). Requires `ffmpeg` on PATH.
//!
//! Three fixtures:
//! - `multi/Test Author/Test Book/` — two sub-second tagged mp3s, a multi-file
//!   book (one chapter per file), no ffprobe needed to read.
//! - `single/book.m4b` — one sub-second m4b with two embedded chapter markers,
//!   for the ffprobe-gated chapter test.
//! - `sidecar/Tagged Folder/` — a tagged mp3 plus a `metadata.opf` that disagrees
//!   with the tags, to prove the sidecar wins the merge.

use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");

    // 1. Multi-file book: Test Author / Test Book / {01,02}.mp3.
    let multi = root.join("multi/Test Author/Test Book");
    std::fs::create_dir_all(&multi).expect("mkdir multi");
    tagged_mp3(
        &multi.join("01.mp3"),
        &[
            ("album", "Test Book"),
            ("artist", "Test Author"),
            ("album_artist", "Test Author"),
            ("composer", "Test Reader"),
            ("track", "1/2"),
            ("title", "Chapter One"),
            ("date", "2021"),
        ],
    );
    tagged_mp3(
        &multi.join("02.mp3"),
        &[
            ("album", "Test Book"),
            ("artist", "Test Author"),
            ("album_artist", "Test Author"),
            ("composer", "Test Reader"),
            ("track", "2/2"),
            ("title", "Chapter Two"),
            ("date", "2021"),
        ],
    );

    // 2. Single M4B with two embedded chapters.
    let single = root.join("single");
    std::fs::create_dir_all(&single).expect("mkdir single");
    m4b_with_chapters(&single.join("book.m4b"));

    // 3. Sidecar-overrides-tags fixture.
    let side = root.join("sidecar/Tagged Folder");
    std::fs::create_dir_all(&side).expect("mkdir sidecar");
    tagged_mp3(
        &side.join("book.mp3"),
        &[
            ("album", "Embedded Title"),
            ("artist", "Embedded Author"),
            ("composer", "Embedded Reader"),
            ("title", "Embedded Title"),
        ],
    );
    std::fs::write(
        side.join("metadata.opf"),
        r#"<?xml version="1.0"?>
<package xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:opf="http://www.idpf.org/2007/opf">
  <metadata>
    <dc:title>Sidecar Title</dc:title>
    <dc:creator opf:role="aut">Sidecar Author</dc:creator>
    <dc:creator opf:role="nrt">Sidecar Reader</dc:creator>
    <dc:language>en</dc:language>
    <meta name="calibre:series" content="Sidecar Series"/>
    <meta name="calibre:series_index" content="2.5"/>
  </metadata>
</package>
"#,
    )
    .expect("write opf");

    println!("done. Commit the files under {}", root.display());
}

/// Synth a sub-second silent mp3 with the given id3 metadata.
fn tagged_mp3(path: &Path, meta: &[(&str, &str)]) {
    let mut cmd = Command::new("ffmpeg");
    cmd.arg("-y")
        .args(["-f", "lavfi", "-i", "anullsrc=r=44100:cl=mono"])
        .args(["-t", "0.3"]);
    for (k, v) in meta {
        cmd.args(["-metadata", &format!("{k}={v}")]);
    }
    cmd.args(["-c:a", "libmp3lame", "-q:a", "9"]).arg(path);
    run(cmd, path);
}

/// Synth a sub-second m4b carrying two embedded chapter markers.
fn m4b_with_chapters(path: &Path) {
    let meta = path.with_extension("ffmeta");
    std::fs::write(
        &meta,
        ";FFMETADATA1\n\
         [CHAPTER]\nTIMEBASE=1/1000\nSTART=0\nEND=2000\ntitle=First Chapter\n\
         [CHAPTER]\nTIMEBASE=1/1000\nSTART=2000\nEND=4000\ntitle=Second Chapter\n",
    )
    .expect("write ffmeta");

    let mut cmd = Command::new("ffmpeg");
    cmd.arg("-y")
        // `-t` limits the anullsrc *input* (it is otherwise infinite); placed
        // before its `-i` so it does not bind to the metadata input.
        .args(["-f", "lavfi", "-t", "4", "-i", "anullsrc=r=22050:cl=mono"])
        .args(["-i"])
        .arg(&meta)
        .args(["-map_metadata", "1", "-map_chapters", "1"])
        .args(["-metadata", "album=Chaptered Book"])
        .args(["-metadata", "artist=Chapter Author"])
        .args(["-c:a", "aac", "-b:a", "32k"])
        .arg(path);
    run(cmd, path);
    let _ = std::fs::remove_file(&meta);
}

fn run(mut cmd: Command, path: &Path) {
    let status = cmd
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .expect("running ffmpeg (is it on PATH?)");
    assert!(status.success(), "ffmpeg failed for {}", path.display());
    println!("wrote {}", path.display());
}
