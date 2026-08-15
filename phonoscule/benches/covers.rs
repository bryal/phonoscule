//! What a cached cover thumbnail costs to take back off disk: the work a rescan repeats for every
//! cover it is not told to skip. Per-cover, so it multiplies out by however many a library holds.
//!
//! The corpus is generated once and cached under the temp dir, following `benches/scan.rs`.

use criterion::{Criterion, Throughput, criterion_group, criterion_main};
use phonoscule::library;
use std::path::PathBuf;

/// Raw cached thumbnails are [`library::THUMB`]² RGB.
const RGB_LEN: usize = (library::THUMB * library::THUMB * 3) as usize;

/// Bump when changing the generator to invalidate cached corpora.
const CORPUS_VERSION: u32 = 1;

/// Covers in the library this was written for, for the multiplied-out figure.
const LIBRARY_COVERS: usize = 731;

/// One thumbnail's worth of bytes, shaped like album art rather than like noise: `accent_color`
/// histograms into 4096 buckets, and noise spreads over all of them where real artwork piles into a
/// few. So, a dominant hue with a small vivid patch.
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

    // A file read plus `rgb_to_rgba`'s widening. The page cache is warm after the first iteration, so
    // this is the syscall and the widening, not the disk.
    group.throughput(Throughput::Bytes(RGB_LEN as u64));
    group.bench_function("read_thumbnail", |b| {
        b.iter(|| {
            let rgba = smol::block_on(library::read_thumbnail(&dir, 0));
            assert!(rgba.is_some());
            rgba
        })
    });

    // The accent histogram on its own, given the pixels.
    group.bench_function("accent_color", |b| b.iter(|| library::accent_color(&rgb)));

    // Both together: what one cover costs a rescan that was not told to skip it.
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
