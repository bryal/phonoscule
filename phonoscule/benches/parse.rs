//! Benchmarks for metadata/tag parsing — the per-file cost a library scanner pays.
//!
//! The inputs are synthesized in-process (the parsers never read past the headers, so the "audio"
//! can be silence): no files, assets, or network required, and the numbers are reproducible.

use criterion::{Criterion, criterion_group, criterion_main};
use embedded_io_adapters::futures_03::FromFutures;
use phonoscule::{
    io::Skippable,
    metadata::{Metadata, StaticMetadata},
    opus::OggOpus,
    wav::Wav,
};
use std::hint::black_box;

/// Builds a valid Ogg Opus stream: OpusHead, OpusTags with the given comments, and `n_packets`
/// dummy audio packets (never decoded by the benchmarks).
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
        // A 20 ms CELT-mode TOC byte followed by nothing much; framing-valid, never decoded here.
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

fn tag_parsing(c: &mut Criterion) {
    // ~100 audio packets / samples: parsing stops after the headers, so the body length mostly
    // just keeps the input realistic-looking.
    let opus = opus_bytes("Some Title", "Some Artist", "Some Album", 100);
    let wav = wav_bytes("Some Title", "Some Artist", "Some Album", 4800);

    c.bench_function("opus_tags", |b| {
        b.iter(|| {
            smol::block_on(async {
                let parsed = OggOpus::<StaticMetadata, _>::parse(&opus[..]).await.unwrap();
                assert_eq!(parsed.metadata.title(), "Some Title");
                black_box(parsed.metadata)
            })
        })
    });

    // Headers-only parsing: what a library scanner should use.
    c.bench_function("opus_tags_headers_only", |b| {
        b.iter(|| {
            smol::block_on(async {
                let mut inp = &opus[..];
                let parsed = phonoscule::opus::Headers::<StaticMetadata>::parse(&mut inp).await.unwrap();
                assert_eq!(parsed.metadata.title(), "Some Title");
                black_box(parsed.metadata)
            })
        })
    });

    c.bench_function("wav_tags", |b| {
        b.iter(|| {
            smol::block_on(async {
                let f = Skippable(FromFutures::new(smol::io::Cursor::new(&wav[..])));
                let parsed = Wav::<StaticMetadata, _>::parse(f).await.unwrap();
                assert_eq!(parsed.metadata.title(), "Some Title");
                black_box(parsed.metadata)
            })
        })
    });
}

criterion_group!(benches, tag_parsing);
criterion_main!(benches);
