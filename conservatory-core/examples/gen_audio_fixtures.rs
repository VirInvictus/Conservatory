//! Regenerate the committed audio test fixtures (dev-only, NOT run in CI).
//!
//! Shells `ffmpeg` to synthesize a sub-second silent container per format, then
//! uses lofty's write API to apply an identical, known tag set plus an embedded
//! solid-red cover. The committed outputs under `tests/fixtures/audio/` are what
//! the test suite reads; this binary is just the reproducible recipe.
//!
//! Run: `cargo run -p conservatory-core --example gen_audio_fixtures`
//! Requires `ffmpeg` (with libmp3lame, libopus, and the aac encoder) on PATH.

use std::path::{Path, PathBuf};
use std::process::Command;

use image::{ImageFormat, Rgb, RgbImage};
use lofty::config::WriteOptions;
use lofty::picture::{MimeType, Picture, PictureType};
use lofty::prelude::{Accessor, ItemKey, TagExt};
use lofty::tag::{ItemValue, Tag, TagItem, TagType};

/// (filename, lofty tag type, ffmpeg codec args).
const TARGETS: &[(&str, TagType, &[&str])] = &[
    ("sample.flac", TagType::VorbisComments, &["-c:a", "flac"]),
    (
        "sample.mp3",
        TagType::Id3v2,
        &["-c:a", "libmp3lame", "-b:a", "128k"],
    ),
    ("sample.opus", TagType::VorbisComments, &["-c:a", "libopus"]),
    (
        "sample.m4a",
        TagType::Mp4Ilst,
        &["-c:a", "aac", "-b:a", "128k"],
    ),
];

fn main() {
    let out_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/audio");
    std::fs::create_dir_all(&out_dir).expect("creating fixtures dir");

    let cover = solid_red_png();

    for (name, tag_type, codec) in TARGETS {
        let path = out_dir.join(name);
        synth_silence(&path, codec);
        write_tags(&path, *tag_type, cover.clone());
        println!("wrote {}", path.display());
    }
    println!("done. Commit the files under {}", out_dir.display());
}

/// 0.3s of stereo silence at 44.1 kHz in the requested codec.
fn synth_silence(path: &Path, codec: &[&str]) {
    let status = Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-y"])
        .args([
            "-f",
            "lavfi",
            "-i",
            "anullsrc=r=44100:cl=stereo",
            "-t",
            "0.3",
        ])
        .args(codec)
        .arg(path)
        .status()
        .expect("running ffmpeg (is it on PATH?)");
    assert!(status.success(), "ffmpeg failed for {}", path.display());
}

fn write_tags(path: &Path, tag_type: TagType, cover: Vec<u8>) {
    let mut tag = Tag::new(tag_type);
    tag.set_title("Test Title".to_string());
    tag.set_artist("Test Artist".to_string());
    tag.set_album("Test Album".to_string());
    tag.set_track(3);
    tag.set_track_total(12);
    tag.set_disk(1);
    tag.set_disk_total(1);
    tag.set_year(2021);

    tag.insert(TagItem::new(
        ItemKey::AlbumArtist,
        ItemValue::Text("Test Album Artist".to_string()),
    ));
    // Two real genre values to exercise the reader's multi-value path.
    tag.push(TagItem::new(
        ItemKey::Genre,
        ItemValue::Text("Electronic".to_string()),
    ));
    tag.push(TagItem::new(
        ItemKey::Genre,
        ItemValue::Text("Ambient".to_string()),
    ));
    tag.insert(TagItem::new(
        ItemKey::ReplayGainTrackGain,
        ItemValue::Text("-7.50 dB".to_string()),
    ));
    tag.insert(TagItem::new(
        ItemKey::ReplayGainAlbumGain,
        ItemValue::Text("-7.20 dB".to_string()),
    ));

    tag.push_picture(Picture::new_unchecked(
        PictureType::CoverFront,
        Some(MimeType::Png),
        None,
        cover,
    ));

    tag.save_to_path(path, WriteOptions::default())
        .unwrap_or_else(|e| panic!("tagging {}: {e}", path.display()));
}

fn solid_red_png() -> Vec<u8> {
    let img = RgbImage::from_pixel(32, 32, Rgb([0xFF, 0x00, 0x00]));
    let mut buf = Vec::new();
    img.write_to(&mut std::io::Cursor::new(&mut buf), ImageFormat::Png)
        .expect("encoding cover png");
    buf
}
