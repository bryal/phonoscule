# Where the CPU goes

Written because both players were costing 10-20% of a core on a weak machine (an MNT Pocket Reform)
while sitting in the background playing music, which is too much for something meant to be left
running.

Everything below was measured, not reasoned about. Where a measurement contradicts what the code
looked like it would do, the measurement wins and the note says so.

## How to reproduce

```sh
RUSTFLAGS="-C force-frame-pointers=yes" cargo build --profile profiling
scripts/census-matrix-tui.sh 30 3 > /tmp/tui-baseline.txt
```

`scripts/cpu-census.sh` reports per-thread CPU and wakeups for a running player;
`scripts/census-env.sh` points it at scratch cache and state roots warmed from the real ones, so a
measurement run cannot leave the real player somewhere it did not put itself;
`scripts/tui-harness.py` runs the TUI on a pty of an exact size, because under a tiling compositor
the window size is otherwise the compositor's decision and "the same terminal, one third the height"
is not a thing a run can ask for.

**Units.** Every percentage is a percentage of **one core**, which is the unit the complaint was in.

**Caveats, stated once.** The machine these numbers come from is a 16-core x86 workstation that was
in use while measuring, so: three windows per row with the spread reported; GPU numbers read only as
relative within a run; and the absolute figures are roughly a percent or two high because the
profiling build forces frame pointers (a constant offset, so it does not touch any before/after
comparison). Kernel sampling is unavailable here (`perf_event_paranoid` is 2), so flamegraphs
decompose user time only - which is why wakeup counts, not the profiler, are the evidence for
anything syscall-shaped.

## The headline

TUI, half blocks, 200x50, warm caches, a 759-album library, on a pty with no terminal emulator in the
measurement:

| configuration | process CPU% | main thread | `phonoscule-audio` | `threaded-ml` |
| --- | --- | --- | --- | --- |
| Library, paused | **0.03** | 0.00 | 0.00 | 0.00 |
| Library, playing | **1.83** (1.66-2.16) | 0.54 | 0.70 | 0.46 |
| Player, paused | **0.03** | 0.00 | 0.00 | 0.00 |
| Player, playing | **1.84** (1.83-1.86) | 0.66 | 0.66 | 0.42 |
| Library, playing, 80x24 | **1.39** | 0.19 | 0.66 | 0.41 |

Three things fall straight out of that table.

**Paused is free.** 0.03%. Every cost in this program is playback-driven, so nothing that runs on a
timer regardless of state is worth chasing for CPU.

**The audio path is the larger half.** `phonoscule-audio` (decode and convert) plus `threaded-ml`
(the PulseAudio client thread that hands samples to the server) is ~1.1% of the 1.83%, against ~0.54%
for drawing. I had expected the reverse.

**Scaling to the machine that prompted this.** 1.83% here, against the 10-20% reported on the Pocket
Reform, is a factor of 6-11 - about what an A53/A72-class core against a modern x86 one predicts for
scalar float DSP, especially with no SIMD anywhere in the decoder. So the shape of this table is
believable as the shape of the problem on the target, with one adjustment noted under `floorf` below.

## What the audio thread is doing

45 seconds of `perf record -t` on `phonoscule-audio`, 425 samples, user-mode, self time:

| symbol | share of the thread |
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
| `opuscule::entdec::ec_dec_uint` | 1.39% |
| `floorf` | 0.86% |
| `pa_detect_fork` + `pa_frame_size` | 1.18% |
| `opuscule::util::OrPanic::or_panic::<i32>` | 0.75% |

Decoding is **84.68%** of the thread. Grouped: PVQ (`decode_pulses` + `quant_band` + `alg_unquant` +
`exp_rotation1`) is ~34.7%, the inverse MDCT and its FFT ~16.8%, the comb filter 16.7%, everything
else smaller. That ordering matches what the structure predicted, with one exception that turned out
to be the biggest single item in the decoder.

## Findings

### 1. The comb filter multiplies by zero for most music

`comb_filter` was the largest leaf in the entire decoder. RFC 6716's C runs its two loops whatever the
postfilter gains are; libopus later added an early return for `g0 == 0 && g1 == 0`, and opuscule -
being a translation of the RFC-era code - did not have it. An encoder leaves the postfilter off for a
great deal of music, and when it is off every one of those multiply-accumulates has a zero
coefficient.

Fixed in opuscule by taking libopus's shortcut. Both normal-path call sites pass `y: None`, so it is a
plain return; the separate-output case used by packet-loss concealment still copies, as `OPUS_MOVE`
does there.

### 2. The 16 Hz progress tick costs about what drawing costs

`player.rs` emits `Event::Progress` 16 times a second, and both front ends turn every one into a
whole frame - though the TUI shows time in whole seconds. Confirmed at the wakeup level: the main
thread wakes 14.3-14.9 times a second, and it is the only thing waking it.

Worth ~0.54% in the Library view and ~0.66% in the Player view, i.e. 30-36% of the process. Not "the
whole problem" as I had assumed, but the largest single thing on the drawing side and the cheapest to
fix.

### 3. Per-frame work is priced by screen area, not by library size

This one refuted the hypothesis it was meant to test. The plan predicted the all-albums `Vec<Line>`
build (759 rows, ~40 visible) would dominate a frame, in which case shrinking the terminal would
change nothing. Shrinking it from 200x50 to 80x24 - 5.2x fewer cells - cut the main thread from 0.54%
to 0.19%.

Solving for a fixed part and a per-cell part gives roughly **0.11% that scales with the library and
0.43% that scales with the screen**. So the expensive things in a frame are `terminal.draw`'s
whole-buffer diff and reset and the half-blocks cover repaint, and building rows for 759 albums to
show 40 of them is about 6% of the process. Windowing that row build - which the plan listed as a
possible optimisation - would buy almost nothing, and the plan was right to rank it last.

### 4. `float2int16` was already inlined; its `floor` was not free

The plan asserted that `sample_to_i16` and `float2int16`, being non-`#[inline]` `pub fn`s in another
crate with no LTO, would be real calls 96,000 times a second. They are not: both appear in the profile
as `(inlined)`, because rustc's MIR inliner crosses crate boundaries for small functions without
needing the attribute. That fix was unnecessary and has been dropped.

What is real is `floorf` at 0.86% of the thread: `(x + 0.5).floor()` compiles to a libm call on
baseline x86-64, which has no `roundss` before SSE4.1. **This is target-specific and does not apply to
the machine that prompted the investigation** - aarch64 has `frintm`, so on the Pocket Reform this
cost is already absent. On x86 it is a build-flag question (`-C target-cpu=native`), not a code one.

### 5. The 40 Hz volume poll is a wakeup problem, not a CPU problem

`volume.rs` wakes a thread 40 times a second forever, playing or idle. Measured exactly: **40.0
wakeups per second**, paused and playing alike. It costs **0.02%**.

So the fix the plan proposed (blocking on libpulse's own poll with a wake pipe registered as an io
event, which I confirmed `libpulse_binding` 2.30 supports) is worth having for power - 40 wakeups a
second is what keeps a core out of deep idle - but it is not part of the 10-20% and should not be sold
as if it were.

### 6. Handing samples to PulseAudio costs nearly as much as decoding them

`threaded-ml` - libpulse's own thread, created by `pa_simple` inside the sink - costs 0.42-0.46% and
wakes **268-273 times a second**. The audio thread writes 95-97 times a second, exactly the 93.75 that
`CHUNK = 512` at 48 kHz predicts, so each write costs about 2.85 wakeups of the client thread, plus
`pa_detect_fork` and `pa_frame_size` on our own thread.

This was not in the plan at all. The lever is the chunk size: 512 frames is 10.7 ms of audio per
write, and a larger chunk divides this whole line item. The cost is command latency - the player
checks for a pause or seek once per chunk - so 2048 frames would mean up to 43 ms, still well under
what anyone notices.

### 7. A safety helper that did not inline

`OrPanic::or_panic` appears as a real symbol at 0.75% of the audio thread. It is generic, so it should
monomorphise into the caller and vanish; it carries no `#[inline]`, and at 16 codegen units without
LTO that is evidently not enough. Cheap to fix, and it is pure overhead - the panic branch never
taken.

## Status

Measured and confirmed: the table above, the wakeup counts, the audio-thread profile.

Fixed so far: finding 1 (the comb filter), pending the RFC vector bit-exactness gate.

Not yet measured: the GUI matrix, and a run in a real terminal to price the emulator's own share
(these numbers deliberately exclude it). One planned row, a queue holding the whole library, did not
start playing and needs rerunning before it says anything.
