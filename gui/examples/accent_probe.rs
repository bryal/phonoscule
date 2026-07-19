//! Diagnostic for the accent heuristic: runs the exact thumbnail + accent pipeline on an image,
//! prints the winning accent and the top-scoring histogram buckets, so a wrong glow can be traced
//! to the bucket contest that produced it.
//!
//! `cargo run -p phonoscule-gui --example accent_probe -- <image>`

use phonoscule_gui::library::{THUMB, accent_color};

fn main() {
    let path = std::env::args().nth(1).expect("usage: accent_probe <image>");
    let img = image::open(&path).expect("cannot decode");
    let rgb = img.resize_to_fill(THUMB, THUMB, image::imageops::FilterType::Triangle).into_rgb8().into_raw();

    let accent = accent_color(&rgb);
    println!("accent: r={:.3} g={:.3} b={:.3}", accent.r, accent.g, accent.b);

    // Mirror of accent_color's internals (kept in sync by hand; this is a debugging aid).
    let mut buckets = vec![[0u64; 4]; 16 * 16 * 16];
    for px in rgb.chunks_exact(3).step_by(7) {
        let (r, g, b) = (px[0] as u64, px[1] as u64, px[2] as u64);
        let bucket = &mut buckets[((r >> 4 << 8) | (g >> 4 << 4) | (b >> 4)) as usize];
        *bucket = [bucket[0] + 1, bucket[1] + r, bucket[2] + g, bucket[3] + b];
    }
    let samples = (rgb.len() / 3).div_ceil(7) as u64;
    let score = |&[n, r, g, b]: &[u64; 4]| {
        if n == 0 {
            return 0.0;
        }
        let (r, g, b) = ((r / n) as f32 / 255.0, (g / n) as f32 / 255.0, (b / n) as f32 / 255.0);
        let chroma = r.max(g).max(b) - r.min(g).min(b);
        let vivid = if n * 1000 >= samples { chroma.powi(3) } else { 0.0 };
        n as f32 * (1e-4 + vivid)
    };
    let mut ranked: Vec<&[u64; 4]> = buckets.iter().filter(|b| b[0] > 0).collect();
    ranked.sort_by(|a, b| score(b).total_cmp(&score(a)));
    println!("top buckets (score, population, mean rgb):");
    for bucket in ranked.iter().take(8) {
        let &[n, r, g, b] = *bucket;
        println!("  {:>12.1}  n={:<6} rgb=({:>3}, {:>3}, {:>3})", score(bucket), n, r / n, g / n, b / n);
    }
}
