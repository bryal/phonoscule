# Where the CPU goes

Written because both players were costing 10-20% of a core on a weak machine (an MNT Pocket Reform)
while sitting in the background playing music, which is too much for something meant to be left
running.

Everything below was measured. Where a measurement contradicted what the code looked like it would
do, the measurement won and the note says so - about half of what follows is that.

## Result

Percentages are of **one core**, which is the unit the complaint was in.

Terminal player, half blocks at 200x50, a 759-album library, warm caches:

| configuration | before | after |
| --- | --- | --- |
| Library, paused | 0.03% | 0.03% |
| Library, playing | **1.83%** | **0.91%** |
| Player, playing | **1.84%** | **0.90%** |

Graphical player:

| configuration | before | after |
| --- | --- | --- |
| Library, paused | 0.03% | 0.03% |
| Library, playing | 1.73% | 1.70% (main thread 0.34% to 0.03%) |
| Player, playing | **3.92%** | **2.13%** |

Roughly half, in both, from three changes. Scaled by the 6-11x this workstation's core has over an
A53/A72-class one, that should be the difference between 10-20% and 5-10% on the machine that
prompted it.

## Method

```sh
RUSTFLAGS="-C force-frame-pointers=yes" cargo build --profile profiling
scripts/census-matrix-tui.sh 30 3 > /tmp/tui.txt
scripts/census-matrix-gui.sh 30 3 > /tmp/gui.txt
```

`cpu-census.sh` reports per-thread CPU and wakeups for a running player. Per thread matters more than
anything else here: the threads are already named, so the decoder's cost and the drawing's cost
separate with no instrumentation, and process CPU alone cannot say which of the two to go and fix.
`census-env.sh` points a run at scratch cache and state roots warmed from the real ones.
`tui-harness.py` runs the TUI on a pty of an exact size, because under a tiling compositor the window
size is otherwise the compositor's decision - and comparing a tall terminal against a short one is how
you find out whether a frame is priced by what is on screen or by the size of the library.

**Caveats, once.** This is a 16-core x86 workstation that was in use throughout: three windows per row
with the spread reported, GPU figures read only as relative within a run, and absolute numbers about a
percent or two high because the profiling build forces frame pointers (a constant offset, so
before/after is unaffected). Kernel sampling is unavailable (`perf_event_paranoid` is 2), so
flamegraphs decompose user time only - which is why wakeup counts, not the profiler, are the evidence
for anything syscall-shaped. The TUI figures exclude the terminal emulator, deliberately: the pty
harness is measuring the player.

## What the audio thread was doing

45 s of `perf record -t` on `phonoscule-audio`, 425 samples, user time, self:

| symbol | share of thread |
| --- | --- |
| `opuscule::celt::comb_filter` | **16.69%** |
| `opuscule::cwrs::decode_pulses` | 15.73% |
| `opuscule::bands::quant_band` | 10.24% |
| `opuscule::kiss_fft::opus_ifft` | 8.61% |
| `opuscule::mdct::clt_mdct_backward` | 8.17% |
| `phonoscule::player::player_loop` (convert + sink write) | 5.18% |
| `opuscule::vq::alg_unquant` | 4.46% |
| `opuscule::vq::exp_rotation1` | 4.24% |
| `opuscule::celt::deemphasis` | 2.56% |
| `pa_detect_fork` + `pa_frame_size` | 1.18% |
| `floorf` | 0.86% |

Decoding was 84.68% of the thread. Grouped: PVQ ~34.7%, inverse MDCT and its FFT ~16.8%, the comb
filter 16.7%.

## The three changes that paid

### Progress reported four times a second instead of sixteen

`player.rs` emitted `Event::Progress` 16 times a second and both front ends turned every one into a
whole frame, though both render the position in whole seconds. Confirmed at the wakeup level: the
main thread woke 14.3 times a second and this was the only thing waking it.

Worth 0.51 points on the TUI (1.61% to 1.10%, main thread 0.54% to 0.04%) and 1.79 points on the GUI
(3.92% to 2.13%, main thread 1.43% to 0.58%). The GUI gains more because one message there is a whole
widget-tree rebuild, a full layout, a draw and a present - iced 0.14 has no way for an update to say
"nothing changed".

The TUI additionally skips the redraw entirely when the second on screen has not changed, which is
what takes its main thread to 0.04%: it now draws about once a second while playing, and not at all
while paused.

### Filling the buffer before writing to the sink

A decoder returns at most what is left of its current frame, so a read is cut short at a frame
boundary and not only at the end of a track: Opus decodes 960 samples at a time, which was filling a
512-frame buffer 512 and then 448, and each short read became its own write.

Writes are not cheap. `threaded-ml` - libpulse's own thread, inside our process - cost 0.40% for 94
writes a second, against 0.59% for all the decoding. Filling the buffer first and raising `CHUNK` to
2048 took the TUI from 1.10% to 0.91%: the client thread 0.40% to 0.31% and 273 to 213 wakeups a
second, and the audio thread 0.59% to 0.50%, since per-write overhead is partly paid there too.

Costs latency: commands are checked once per chunk, so a pause or seek waits up to 43 ms rather than
11 ms.

### Skipping the comb filter when neither tap has any gain

The largest single leaf in the decoder. RFC 6716's C runs its two multiply-accumulate loops whatever
the gains are; libopus later added an early return for `g0 == 0 && g1 == 0`, and opuscule, being a
translation of the RFC-era code, did not have it. Bit-exact, and all 12 reference vectors still match
the baseline in all four build configurations.

**How much it is worth depends on the content, and the two instruments disagreed.** Against the
reference vectors: -21.9% on hybrid (vector 06), -5.1% on mixed CELT (09), and **nothing measurable**
on CELT fullband stereo 20 ms (vector 11, p = 0.27) - which is the one shaped like an opusenc'd music
library. But the census against the real library showed the audio thread dropping from ~0.67% to
~0.59%, about 12%. So this library's encoder does leave the postfilter off a good deal of the time,
and vector 11's does not. The census is the instrument that answers the question that was asked.

## What was tested and did not pay

Recorded because each was a plausible plan that measurement killed, and the next person should not
have to re-derive them.

**Per-frame work is priced by screen area, not by library size.** The prediction was that building a
`Vec<Line>` for all 759 albums to show 40 of them would dominate a frame, in which case terminal size
would not matter. Shrinking 200x50 to 80x24 - 5.2x fewer cells - took the main thread from 0.54% to
0.19%. Solving for a fixed and a per-cell part gives roughly **0.11% scaling with the library against
0.43% scaling with the screen**, so the row build was about 6% of the process and windowing it would
buy almost nothing. Not done.

**`sample_to_i16` was already inlined.** The plan asserted that it and `float2int16`, being
non-`#[inline]` `pub fn`s in another crate with no LTO, would be real calls 96,000 times a second.
Both appear in the profile as `(inlined)` - rustc's MIR inliner crosses crate boundaries for small
functions without the attribute. Nothing to fix.

**`#[inline]` on the range decoder made it slower.** `ec_dec_icdf`, `ec_dec_bit_logp` and friends are
small, called hundreds of times per packet, cross-module, at 16 codegen units with no LTO - and
annotating them cost **+2.1%** on vector 11 (p = 0.00), plus 1.3% on hybrid and 1.0% on SILK.
Inlining them into callers as large as `quant_band` and `decode_pulses` evidently costs more in code
size and register pressure than the calls did. Reverted.

**Thin LTO did not help the decoder either.** No significant change on vector 11 (p = 0.41),
regressions of 1.7% and 2.0% on hybrid and SILK. The hot kernels are self-contained and the arch
helpers are already `#[inline(always)]`, so there was little left to win and code-layout churn to
lose. Reverted.

Between those two, the conclusion is that **the decoder's remaining cost is arithmetic, not overhead** -
PVQ and the inverse MDCT doing real work. Getting it down further means SIMD or algorithmic change,
both of which run into `forbid(unsafe_code)` and the bit-exactness rule.

**The 40 Hz volume poll is a wakeup problem, not a CPU problem.** `volume.rs` wakes a thread 40 times
a second forever, playing or idle - measured at exactly 40.0, and at **0.02%**. Blocking on libpulse's
own poll with a wake pipe registered as an io event is worth doing for power, since 40 wakeups a
second is what keeps a core out of deep idle, but it is not part of the 10-20% and should not be sold
as if it were. Not done.

**`or_panic` was chasing noise.** A symbol at 0.75% of the audio thread is about three samples out of
425, quite possibly sampling skid, and the call count argues against it too - it guards slice ranges
once per band, not per sample. Was going to inline it and outline the panic behind `#[cold]`; measured
it instead, found nothing, dropped it.

## Still open

- The GUI's Player view is 2.13% against the TUI's 0.90%, with 0.58% on its main thread at four
  messages a second and 8.6 wakeups a second where four were sent. The extra wakeups are the cover
  flow's own 62.5 Hz animation timer, which is legitimate while animating; whether it settles as
  promptly as it should is not yet measured.
- One row (a queue holding the whole library) turned out not to isolate what it was meant to: it also
  varies whether the playing album's cover has loaded, and a loaded cover is the expensive thing. No
  number is reported for it. The library-size question was answered by the 200x50-against-80x24
  comparison instead.
- The TUI passes `known_covers: Default::default()` to every scan, so each 5-minute rescan re-reads
  all 731 thumbnails and redoes the RGBA widening and the accent histogram, only to keep the accent
  the index already holds. That is a periodic spike rather than steady-state cost, so it does not
  appear in any table here. `benches/covers.rs` prices one cover; the fix is to pass the covers whose
  path is already known, as the GUI does on rescans.
- `phonoscule-audio` still wakes ~94 times a second after the chunk change, where 23 writes a second
  would predict fewer. Whatever the remaining wakeups are, they are not writes, and they have not
  been chased.

## A note on trusting these numbers

Two of the rows in a confirming run came back wrong, and both times it was the harness rather than the
player. One row measured 0.00% because the restored queue had run out and playback had stopped - a row
that stops playing looks like a triumph, since what is being measured is what playback costs. The
census now sets the loop mode first and warns if a window ended with the player not playing.

The other was the row meant to test whether a queue holding the whole library costs anything per
frame. It does switch to the player view as intended, but it also changes whether the playing album's
cover has loaded - and a loaded cover is the expensive thing on screen - so it was never isolating
queue length. No number is reported for it.

Both are the same lesson: the census answers precisely the question its configuration encodes, which
is not always the question it was named after. Where a figure here is quoted with a spread, that is
the spread of three windows on a machine that was in use, and where a figure moved less than its
spread it is reported as no change rather than as a small one.
