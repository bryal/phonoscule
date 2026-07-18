//! Benchmark of the full library scan over a synthetic corpus.
//!
//! The corpus is generated (once, cached) under the system temp dir: valid-but-silent audio files
//! and procedural cover images. Nothing outside the repo is read, so results are reproducible.
//!
//! The audio generators are duplicated from `phonoscule/benches/parse.rs`; keep them in sync (or
//! promote them to a shared dev crate if they grow).

use criterion::{Criterion, criterion_group, criterion_main};
use futures::StreamExt;
use phonoscule_gui::library::{self, Album, ScanEvent};
use std::path::PathBuf;

const N_ALBUMS: usize = 10;
const TRACKS_PER_ALBUM: usize = 8;
/// Bump when changing the generators to invalidate cached corpora.
const CORPUS_VERSION: u32 = 1;

/// Builds a valid Ogg Opus stream: OpusHead, OpusTags with the given comments, and `n_packets`
/// dummy audio packets (never decoded by the scan).
fn opus_bytes(title: &str, artist: &str, album: &str, n_packets: usize) -> Vec<u8> {
    let mut head = Vec::new();
    head.extend(b"OpusHead");
    head.push(1); // version
    head.push(2); // channels
    head.extend(312u16.to_le_bytes()); // pre-skip
    head.extend(48000u32.to_le_bytes()); // input sample rate
    head.extend(0u16.to_le_bytes()); // output gain
    head.push(0); // channel mapping family 0

    let mut tags = Vec::new();
    tags.extend(b"OpusTags");
    let vendor = b"phonoscule-bench";
    tags.extend((vendor.len() as u32).to_le_bytes());
    tags.extend(vendor);
    let comments = [format!("TITLE={title}"), format!("ARTIST={artist}"), format!("ALBUM={album}")];
    tags.extend((comments.len() as u32).to_le_bytes());
    for comment in &comments {
        tags.extend((comment.len() as u32).to_le_bytes());
        tags.extend(comment.as_bytes());
    }

    let serial = 0x5eed;
    let mut out = Vec::new();
    let mut writer = ogg::PacketWriter::new(std::io::Cursor::new(&mut out));
    writer.write_packet(head, serial, ogg::PacketWriteEndInfo::EndPage, 0).unwrap();
    writer.write_packet(tags, serial, ogg::PacketWriteEndInfo::EndPage, 0).unwrap();
    for i in 0..n_packets {
        let end = if i + 1 == n_packets { ogg::PacketWriteEndInfo::EndStream } else { ogg::PacketWriteEndInfo::NormalPacket };
        writer.write_packet(vec![0xf8, 0xff, 0xfe], serial, end, (i as u64 + 1) * 960).unwrap();
    }
    drop(writer);
    out
}

/// Builds a valid 48 kHz stereo 16-bit PCM WAV with a LIST-INFO tag chunk and `n_samples` of
/// silence.
fn wav_bytes(title: &str, artist: &str, album: &str, n_samples: u32) -> Vec<u8> {
    fn chunk(id: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(8 + body.len() + 1);
        out.extend(id);
        out.extend((body.len() as u32).to_le_bytes());
        out.extend(body);
        if body.len() % 2 == 1 {
            out.push(0); // chunks are padded to even sizes
        }
        out
    }
    fn info(id: &[u8; 4], value: &str) -> Vec<u8> {
        chunk(id, &[value.as_bytes(), b"\0"].concat())
    }

    let mut fmt = Vec::new();
    fmt.extend(1u16.to_le_bytes()); // WAVE_FORMAT_PCM
    fmt.extend(2u16.to_le_bytes()); // channels
    fmt.extend(48000u32.to_le_bytes()); // blocks per second
    fmt.extend((48000u32 * 4).to_le_bytes()); // avg bytes per second
    fmt.extend(4u16.to_le_bytes()); // block size
    fmt.extend(16u16.to_le_bytes()); // bits per sample

    let list = [&b"INFO"[..], &info(b"INAM", title), &info(b"IART", artist), &info(b"IPRD", album)].concat();
    let data = vec![0u8; n_samples as usize * 4];
    let riff = [&b"WAVE"[..], &chunk(b"fmt ", &fmt), &chunk(b"LIST", &list), &chunk(b"data", &data)].concat();

    let mut out = Vec::new();
    out.extend(b"RIFF");
    out.extend((riff.len() as u32).to_le_bytes());
    out.extend(riff);
    out
}

/// Generates (or reuses) the corpus: half opus albums, half wav albums, one cover each.
fn corpus(with_covers: bool) -> PathBuf {
    let kind = if with_covers { "covers" } else { "tags" };
    let root = std::env::temp_dir().join(format!("phonoscule-scan-bench-v{CORPUS_VERSION}-{kind}"));
    if root.exists() {
        return root;
    }
    let tmp = root.with_extension("partial");
    let _ = std::fs::remove_dir_all(&tmp);
    for a in 0..N_ALBUMS {
        let album = format!("Album {a:02}");
        let artist = format!("Artist {:02}", a / 2);
        let dir = tmp.join(format!("{artist}/{album}"));
        std::fs::create_dir_all(&dir).unwrap();
        for t in 0..TRACKS_PER_ALBUM {
            let title = format!("Track {t:02}");
            if a % 2 == 0 {
                std::fs::write(dir.join(format!("{t:02} track.opus")), opus_bytes(&title, &artist, &album, 100)).unwrap();
            } else {
                std::fs::write(dir.join(format!("{t:02} track.wav")), wav_bytes(&title, &artist, &album, 4800)).unwrap();
            }
        }
        if with_covers {
            let cover =
                image::RgbaImage::from_fn(1000, 1000, |x, y| image::Rgba([(x / 4) as u8, (y / 4) as u8, (a * 24) as u8, 255]));
            cover.save(dir.join("cover.png")).unwrap();
        }
    }
    std::fs::rename(&tmp, &root).unwrap();
    root
}

/// Runs a scan to completion, applying the streamed events like the GUI would. Caching is
/// disabled: this measures the full scanning work, reproducibly.
fn drain(root: PathBuf) -> Vec<Album> {
    smol::block_on(async {
        let options = library::ScanOptions {
            root,
            priority: vec![],
            known_covers: Default::default(),
            cache_file: None,
            covers_dir: None,
        };
        let mut albums: Vec<Album> = Vec::new();
        let mut stream = std::pin::pin!(library::scan(options));
        while let Some(event) = stream.next().await {
            match event {
                ScanEvent::Album(album) => albums.push(*album),
                ScanEvent::Cover { albums: ids, art } => {
                    for album in albums.iter_mut().filter(|a| ids.contains(&a.id)) {
                        album.cover = Some(art.clone());
                    }
                }
                ScanEvent::Done { .. } => break,
            }
        }
        albums
    })
}

fn scan(c: &mut Criterion) {
    let mut group = c.benchmark_group("scan");
    group.sample_size(10);

    let tags = corpus(false);
    group.bench_function("tags_only", |b| {
        b.iter(|| {
            let albums = drain(tags.clone());
            assert_eq!(albums.len(), N_ALBUMS);
            albums
        })
    });

    let covers = corpus(true);
    group.bench_function("with_covers", |b| {
        b.iter(|| {
            let albums = drain(covers.clone());
            assert_eq!(albums.len(), N_ALBUMS);
            assert!(albums.iter().all(|a| a.cover.is_some()));
            albums
        })
    });

    group.finish();
}

criterion_group!(benches, scan);
criterion_main!(benches);
