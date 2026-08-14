//! What a cached cover thumbnail costs to take back off disk.
//!
//! This is the work a rescan repeats for every cover it is not told to skip: read the raw thumbnail,
//! widen it to RGBA, and pick the accent colour out of it. A player that already knows an album's
//! artwork path and already has its accent in the persisted index gets nothing back for any of it,
//! so the per-cover figures here multiply straight out by however many covers a library holds.
//!
//! The corpus is generated once and cached under the temp dir, following `benches/scan.rs`.

use criterion::{Criterion, Throughput, criterion_group, criterion_main};
use phonoscule::library;
use std::path::PathBuf;

/// Raw cached thumbnails are [`library::THUMB`]² RGB.
const RGB_LEN: usize = (library::THUMB * library::THUMB * 3) as usize;

/// Bump when changing the generator to invalidate cached corpora.
const CORPUS_VERSION: u32 = 1;

/// How many covers a library the size of the one this was written for holds, for the multiplied-out
/// figure in the report.
const LIBRARY_COVERS: usize = 731;

/// One thumbnail's worth of bytes, shaped like album art rather than like noise.
///
/// The shape matters for `accent_color`, which histograms into 4096 buckets of a 4-bit-per-channel
/// space and then scores them: real artwork piles into a few buckets, while random pixels spread
/// across all of them and would take a path through the scoring loop that no real cover takes. So
/// this is a dominant hue over most of the image with a small vivid patch, which is what a sleeve
/// tends to look like to that histogram.
fn thumbnail_rgb(seed: u32) -> Vec<u8> {
    let edge = library::THUMB;
    let mut rgb = Vec::with_capacity(RGB_LEN);
    let (br, bg, bb) = ((seed * 37 % 90 + 20) as u8, (seed * 61 % 80 + 30) as u8, (seed * 17 % 70 + 40) as u8);
    for y in 0..edge {
        for x in 0..edge {
            // A patch in one corner, vivid enough to win the accent scoring against the ground.
            let vivid = x > edge * 3 / 5 && y > edge * 3 / 5 && (x + y) % 3 != 0;
            match vivid {
                true => rgb.extend_from_slice(&[220, (seed * 43 % 120) as u8, 40]),
                // A gentle gradient, so the ground is a handful of neighbouring buckets rather than
                // exactly one -- a single flat colour would make the histogram trivial.
                false => rgb.extend_from_slice(&[br.saturating_add((x / 40) as u8), bg.saturating_add((y / 40) as u8), bb]),
            }
        }
    }
    rgb
}

/// A directory of cached thumbnails, named the way [`library::read_thumbnail`] looks them up.
fn covers(n: usize) -> PathBuf {
    let root = std::env::temp_dir().join(format!("phonoscule-covers-bench-v{CORPUS_VERSION}-{n}"));
    if root.exists() {
        return root;
    }
    let tmp = root.with_extension("partial");
    let _ = std::fs::remove_dir_all(&tmp);
    let dir = library::covers_dir(&tmp);
    std::fs::create_dir_all(&dir).unwrap();
    for id in 0..n as u64 {
        std::fs::write(dir.join(format!("{id:016x}")), thumbnail_rgb(id as u32)).unwrap();
    }
    std::fs::rename(&tmp, &root).unwrap();
    root
}

fn thumbnails(c: &mut Criterion) {
    let mut group = c.benchmark_group("covers");

    let root = covers(1);
    let dir = library::covers_dir(&root);
    let rgb = thumbnail_rgb(0);
    assert_eq!(rgb.len(), RGB_LEN, "the generator and THUMB have drifted apart");

    // A file read plus `rgb_to_rgba`'s widening: 102,400 pixels copied into a fresh 409,600-byte
    // allocation. The page cache is warm after the first iteration, so this is the read syscall and
    // the widening, not the disk.
    group.throughput(Throughput::Bytes(RGB_LEN as u64));
    group.bench_function("read_thumbnail", |b| {
        b.iter(|| {
            let rgba = smol::block_on(library::read_thumbnail(&dir, 0));
            assert!(rgba.is_some());
            rgba
        })
    });

    // The accent histogram on its own, given the pixels: a 4096-bucket table allocated per call plus
    // a pass over every seventh pixel and a scoring pass over the buckets.
    group.bench_function("accent_color", |b| b.iter(|| library::accent_color(&rgb)));

    // Both together, which is what one cover costs a rescan that was not told to skip it, and the
    // figure to multiply by the size of a library.
    group.bench_function("read_and_accent", |b| {
        b.iter(|| {
            let rgba = smol::block_on(library::read_thumbnail(&dir, 0));
            (rgba, library::accent_color(&rgb))
        })
    });

    println!("multiply the read_and_accent figure by {LIBRARY_COVERS} for a library-sized rescan");
    group.finish();
}

criterion_group!(benches, thumbnails);
criterion_main!(benches);
