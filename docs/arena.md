# The arena

The arena is how bunny_ui measures itself in a real window, on the same scenes, with the same
clock, run after run. The apps live in `apps/arena`; each opens a 1280×800 window, plays a script
that a worker thread paces with a real clock, and exits on its own.

```bash
cargo build --release -p arena
BUNNY_WINDOW_FLOATING=1 \
  target/release/arena_table --script wheel --secs 10 --rows 10000
```

## Scenes

| app | scene | scripts |
|---|---|---|
| `arena_table` | ten thousand rows of six columns in a `virtual_list`, 24 pt rows | `rest`, `wheel`, `soak` |
| `arena_editor` | a `text_editor` over 400 or 30 000 lines, the keyboard in it | `type`, `append`, `wheel` |
| `arena_stream` | a chat that grows by one line every 33 ms | `stream` |
| `arena_canvas` | a `canvas` looping at 60 fps while the window rests | `rest` |

Scripts: `rest` is `--secs` of nothing; `wheel` is 240 steps a second of 6 px, half down then
half up; `type` alternates a character and a backspace ten times a second; `append` types a
character ten times a second and nothing else, so every stroke is a text the engine has never
seen; `stream` appends every 33 ms. Steps are raised through the shell's own door (`bunny_ui_macos::drive`), the same one the
window system's callbacks use — no synthesized system events, no accessibility permission.

The producer uses absolute monotonic deadlines: event `i` is due at `i / 240` seconds for
wheel, `i / 10` for typing, or `i × 33 ms` for streaming. A late producer catches up without
discarding events; handling time is never added to the next deadline. Event counts round down.
The active script starts one second after the first-frame announcement and keeps a one-second
grace after its requested duration. Rest has no input steps or extra grace.

The table, editor and chat name **Menlo** explicitly (12 pt in the table's cells, 13 pt in the
editor and chat). The font is part of the macOS fixture and must be installed; a platform's
default face changes glyph widths, wrapping and raster work. Record the installed font's digest
alongside the executable's digest. The historical tables below predate this fixed-face fixture
and must not be mixed with its measurements.

Every app prints `FIRST_FRAME <unix ms>` once, when its root has run its first pass; the launch
cost is that line against the spawn time. A scripted run also prints `SCRIPT_START <unix ms>` when
its first step is about to be sent, and the stretch a scene is measured over is anchored there —
a window that reports its first frame late, or early, would otherwise slide the stretch into idle
time or into the launch. The fixtures (`rows-10k.tsv`, `lines-400.txt`,
`lines-30k.txt`) are read from `ARENA_FIXTURES`.

At the end of the producer's duration, `SCRIPT_QUEUED` prints its Unix timestamp, `count`,
`elapsed_ns` and `max_late_ns`. After the main thread has handled every queued step in order,
`SCRIPT_DONE` prints its timestamp and handled `count`. Handler completion does not measure
input-to-photon latency. Comparisons require matching requested, queued and handled counts;
the current admission limits are 20 ms for producer lateness/duration overrun and completion
by the requested duration plus two seconds, including the one-second grace. Missing or late
completion is a failed workload and its observed CPU, memory and GPU values are excluded from
rankings. These markers are outside the steady CPU counter window.

Typing and append also require the `editor-tail-v1` state contract. The editor starts focused,
with a collapsed selection at the document's end and that caret in view. Before the first
mutation, `EDITOR_READY` reports its Unix timestamp, `length_utf16`, `caret_utf16` and
`selection_utf16` from the live editor. `EDITOR_DONE` reports the same fields plus `text_ok`
after comparing the entire final document with the requested sequence: append adds exactly
the requested number of `x` characters; alternating typing restores the original for an even
count and leaves one trailing `x` for an odd count. Both observations use UTF-16 units.

The final observation happens before `SCRIPT_DONE`, after the measured window. The original
fixture is reloaded there, so an extra full document is not held throughout measurement.
Missing state, an incorrect caret/selection or `text_ok=0` rejects the editor row even when
event counts match. Earlier editor measurements below predate these state witnesses; they
describe their recorded Bunny fixture, not a comparison with a verified common starting state.

The historical before/after tables below use the preserved script implementation from their
source revisions. Binaries with the absolute-deadline protocol need a fresh comparison session.

## Non-editor geometry and work

The non-editor fixtures use `scene-geometry-v1`. The table has a 40 pt header above a
1280×760 viewport, 24 pt rows with no gaps, and six left-aligned columns of 70/300/90/140/90/110 pt.
Column gaps are 8 pt and the leading inset is 12 pt. Streaming starts empty at the top of a
1280×800 viewport, with 28 pt rows, an 8 pt leading inset and one exact message per append.
Both lists paint alternating row backgrounds across the viewport width. Their floor colour
is `17171c`, the other stripe is `1c1c21`, and text is regular Menlo in `e6e6ea`.

The canvas has the same 40 pt header, with a 240×240 drawing box at `(0,40)`. A 12 pt square
in `e69933` follows a 72 pt radius orbit over two seconds. Headers use 13 pt regular Menlo
with a 12 pt leading inset. Text stays vertically centred; native font rasterization remains
part of the implementation being measured.

`SCENE_READY` and `SCENE_DONE` report the actual layout, held scroll offset and content state
at input boundaries. The observer uses the window's root and runtime, with an extra layout
before the first input or after completion, outside the CPU counter window. Rest has no input
and reports only completion. Streaming completion verifies every stored message and its count;
an unchanged offset at the top is part of the scene, even after content exceeds the viewport.

A separate `ARENA_SCENE_DIAGNOSTIC=1` run establishes actual motion. For `wheel`, it sends
120 downward events at 240 Hz, allows one second to settle, records `SCENE_DOWN`, then repeats
upward and records `SCENE_UP`. The expected settled offsets are 0 → 720 → 0 pt. For the canvas,
`CANVAS_FRAME` samples the actual paint rectangle, marker and phase across wall time. These
are paint observations, not presentation counts or input-to-photon measurements. The diagnostic
changes the workload and must be unset for CPU, footprint and GPU runs.

Admission combines observed geometry and content, own-window visual inspection, and separate
motion diagnostics. Preserve their source and executable hashes with the frozen measurement
session; completion counters alone cannot prove that content was drawn. Earlier non-editor
tables below describe their recorded fixtures and predate this geometry contract.

## What is read

CPU and footprint runs leave `BUNNY_PRESENT_TRACE` and `BUNNY_FRAME_STATS` unset. Detailed
tracing changes the work being measured: four interleaved runs of the same fixed-face binary
measured a wheel median of 14.968% CPU with both enabled and 7.203% with both disabled. That
difference is diagnostic overhead, not an engine improvement. Collect the present tape and
frame histograms in a separate pass:

```bash
BUNNY_WINDOW_FLOATING=1 BUNNY_PRESENT_TRACE=/tmp/table-{pid}.trace BUNNY_FRAME_STATS=1 \
  target/release/arena_table --script wheel --secs 10 --rows 10000
```

The present tape contains:

- `F` — a frame's CPU half: `settle=` (bodies and effects), `layout=` (measure and place), `place=`.
- `P` — a present, with the drawable's size and the command count; `P` lines per second at rest
  must be zero.
- `E` — the present's own duration on the CPU side.
- `G` — the frame's time on the GPU, from the command buffer's own `GPUStartTime`/`GPUEndTime`,
  read when its slot is taken again.
- `W` — a wake, and whether it needed a frame.
- `Q` — a change presented through the patch layer (its box, and `moved` when the box changed),
  or the patch stepping aside.

Around the process: its CPU time from the kernel at the start and the end of the script's active stretch —
the CPU column is that difference over the wall time between them (`top`'s once-a-second samples
miss short bursts and are kept only as a second view); `footprint` for the physical footprint and
the graphics memory it holds (IOAccelerator, IOSurface), read when the window has rested long
enough for the Metal driver's launch-time allocations to drain (seconds after a launch every Metal
app holds 100–300 MB that are gone ten seconds later); `ioreg`'s accelerator statistics for the
device's utilization against an idle baseline; and, in a separate pass, Instruments' Metal
Application recording for the GPU's execution intervals, keeping the app, global WindowServer
and other processes separate. WindowServer activity is an observation of the whole desktop;
it cannot all be attributed exclusively to the measured app. Read footprint after the CPU
counter window closes, and retain the raw counter values and actual sample times.

## Rules

Interleaved rounds; before each round the machine must be quiet (no build for 60 s, a CPU canary
within 1.3× the session's first) — a busy machine is waited out, not measured; `caffeinate` for
the session; the display awake (a sleeping display stops every display link, and a frame-bound
app never reports a frame); every window floated above the others (`BUNNY_WINDOW_FLOATING`), since
a window opened from a background process lands behind the front one, and a covered window is a
different workload; medians over runs; absolutes compared only inside one session's table.
The graphical session must also remain unlocked: an awake display alone is insufficient.
Read console/login/lock/display state before and after every sample, and sample it every
500 ms in the parent collector. Preserve those observations and reject an observed lock or
a sampling gap longer than 1.5 seconds; transitions between polls are not claimed observable. A window can still report layout and yield an own-window image
while its native surface is occluded; neither observation alone admits a performance run.

## bunny_ui's own numbers

From 2026-10-06 on an Apple M5 Max, macOS 27, the main display at 1x (1680×1050, 60 Hz), release
builds, one clean round (medians over five launches). CPU is percent of one core over the script's
active stretch, from `top`; the footprint is `footprint`'s at the end of the steady window.

| scene | launch → first frame | CPU | footprint | frames |
|---|---|---|---|---|
| table at rest, 15 s | 87 ms | 0.00 % | 44 MB (52 before the atlas change) | 0 presents |
| table under the wheel, 240 steps/s | — | 7.4 % | — | frame p50 0.08 ms, p95 0.21 ms; present 0.40 ms; GPU 0.11 ms; 60 presents/s |
| editor, 400 lines, typing 10/s | — | 0.8 % | — | frame p50 0.04 ms |
| editor, 30 000 lines, typing 10/s | — | 2.9 % (6.0 before the UTF-16 change) | — | frame p50 0.79 ms |
| chat, one line every 33 ms | — | 3.8 % | — | frame p50 0.16 ms; 24 presents/s |
| looping canvas at rest, 60 fps | — | 1.0 % (5.4 before the IOSurface change) | — | the window presents nothing; the island ticks on its layer |

What those three changes were: the text atlas opens at a megapixel and doubles when asked
(`78d3731`); a live box's layer shows an IOSurface instead of a CGImage Core Animation had to copy
and colour-convert every tick (`f099360`); a UTF-16 offset for the IME is counted from the bytes
instead of decoded char by char (`093b4f7`).

## The round after (2026-10-06, afternoon)

Each change measured against the build before it, the builds interleaved round by round on the
same machine; CPU is process CPU time from the kernel's counters over the same window; GPU is
Instruments' GPU track — the GPU's own execution intervals for the app and for the window server
compositing it, in milliseconds per second.

| change | scene | before | after |
|---|---|---|---|
| patch presents (`5f5ab82`), with the diff (`dc22134`) and painted-span equality (`b874138`) | typing, 400 lines: GPU app / app + compositor | 2.20 / 6.13 ms/s | 0.28 / 3.48 ms/s |
| | chat streaming: GPU app / app + compositor | 2.43 / 7.50 ms/s | 1.25 / 4.85 ms/s |
| | wheel: GPU app | 6.50 ms/s | 6.50 ms/s |
| | typing, 400 lines: CPU | 0.77 % | 0.58 % |
| UTF-16 count (`6dd0989`), then the lent text (`bc24835`) | typing, 30 000 lines: CPU | 2.77 % | 2.17 %, then 1.87 % |
| | appending, 30 000 lines: CPU | 1.55 % | 1.21 %, then 1.14 % |
| | note-sized copies per keystroke | 8 | 2 |
| resting drawables offered back (`373095d`) | table at rest: footprint | 37 MB | 33 MB |

Absolute CPU percentages move between sessions on this machine (a busy machine schedules the same
work differently), which is why only the interleaved pairs above are compared.

## The slow clock at rest (fixed-face fixture)

The macOS shell parks its half-second housekeeping timer when no window needs it. A caret,
tooltip delay, key sequence, wheel gesture, deferred garbage or display-frame recovery keeps
the clock armed. Requests from all open windows are combined; replacing the timer's window
invalidates the former timer, and events preserve an already armed timer's cadence.

Four interleaved release pairs on the same M5 Max and display, with tracing disabled, compare
the preceding binary with this change. Rest uses seconds 10–18 after the first frame; wheel
uses seconds 1–9 after the script starts. Values are median process CPU, as percent of one core,
from two kernel-counter reads per run.

| scene | before | after |
|---|---|---|
| table at rest | 0.02364% | 0.01011% |
| table under the wheel | 7.234% | 7.188% |

Rest CPU fell by 57.2%, with a lower value in every pair; the absolute saving is small.
Wheel remained within run-to-run variation. Normal and paranoid runs each passed 896 engine
tests and 27 native-shell tests; the full scratch consumer suite passed 2,889 tests with
44 ignored. These measurements make no allocation or GPU claim.

## Small opaque patches (fixed-face fixture)

On macOS, a small opaque patch can use the software raster and a `CALayer` backed by an
IOSurface. The strategy accepts at most 128 Ki physical pixels, 64 commands, 4 KiB of text
and 128 Ki pixels of text raster work. Unsupported paint or a busy surface pool takes the
existing Metal path. Each patch keeps at most three surfaces; the current surface and any
surface the compositor still uses are protected. A DeviceRGB tag preserves the Metal layer's
colour appearance.

Interleaved release pairs on the same M5 Max and display compare this strategy with the
preceding Metal-only build, including the texel-centre correctness fix (`83dcb3f`). CPU uses
kernel counters from seconds 1–9 after the script starts, with tracing off. Footprint is read
after that counter window, during the active scene; it is physical memory, not allocation count.

| scene | pairs | CPU before | CPU after | footprint before / after |
|---|---|---|---|---|
| append, 30,000 lines | 6 | 1.850% | 1.569% | 195.5 / 45.5 MB (two separate pairs) |
| type, 400 lines | 4 | 0.790% | 0.590% | 182 / 32 MB |
| table under the wheel | 4 | 6.920% | 7.091% | 195 / 195 MB |
| chat streaming | 4 | 1.495% | 1.533% | 33.5 / 35 MB |

Append CPU fell by 15.2%, lower in all six pairs; typing fell by 25.4%, lower in all four.
Wheel and streaming each increased by 2.5%, below the 5% regression guard, with variation across pairs.
Typing's footprint reduction is mostly driver-owned graphics memory: the unmapped graphics
category was 145 MB before and about 1.2 MB after in the first pair. These small edits no
longer keep submitting Metal work while the initial frame's driver allocations can drain.

Three separate interleaved GPU pairs of append at 30,000 lines measure a median observed app
plus global WindowServer cost of 1.809 → 1.234 ms/s, 31.8% lower and lower in every pair.
The app's median is 0.602 → 0.000 ms/s; WindowServer's is 1.207 → 1.234 ms/s. The software
path still needs composition, and the global WindowServer figure is not exclusive attribution
to this window. These GPU traces are separate from every CPU measurement above.

Normal and paranoid runs each pass 896 engine, 66 Apple and 27 native-shell tests; the complete
scratch consumer suite passes 2,889 tests with 44 ignored. Actual-window captures preserve
typing pixels exactly at both document sizes; append differs by at most two channel levels
from corrected Metal. Crop-versus-whole raster and Metal parity are checked at scales 1 and 2.
Apple strict clippy has the same 43 existing diagnostics as the baseline, with none introduced.

## The absolute-deadline round (2026-10-07)

Three interleaved release rounds on the same M5 Max, macOS 27.0 and 1680×1050 display at
60 Hz use renderer `18eca8e` and arena `9ebf5bb`. The table, editor and chat use the fixed
Menlo fixture. All Bunny workloads completed within the protocol limits above. The first
CPU canary was 54.014 ms; the 70.2182 ms admission limit stayed fixed, and refused starts
waited for a quiet window.

CPU is percent of one core from two kernel-counter reads, with frame and GPU tracing off.
Active scripts last ten seconds; their CPU window is seconds 1–9 after `SCRIPT_START`.
The looping canvas uses seconds 2–8 after its first frame. Rest lasts twenty seconds and
uses seconds 10–18. Physical footprint is read after each CPU window and includes the
process's graphics allocations; it is neither allocation count nor total desktop memory.

| scene | median CPU | median physical footprint |
|---|---:|---:|
| table at rest | 0.0121% | 33 MB |
| table under the wheel | 7.1518% | 196 MB |
| type, 400 lines | 0.6018% | 32 MB |
| type, 30,000 lines | 0.8840% | 49 MB |
| append, 30,000 lines | 1.4055% | 46 MB |
| chat streaming | 1.5477% | 35 MB |
| looping canvas | 2.0015% | 32 MB |

The median spawn-to-first-frame announcement is 78.61 ms over fifteen separate launches.
This is the fixture's first root pass, not a measured first pixel on the display. These
absolute values belong to this session; changes from historical tables are not an A/B
engine speedup, since the input protocol and collection method also changed.


## Editors with state witnesses (2026-10-07)

The `editor-tail-v1` fixture at `e3274a6` keeps renderer `18eca8e` and the same machine,
font and CPU collection windows. Three interleaved release rounds validate the actual
initial and final UTF-16 selection and complete text, alongside the existing input deadlines.
All nine Bunny samples pass both protocols. The first CPU canary was 53.496708 ms; its
69.5457204 ms admission limit stayed fixed.

| scene | median CPU | median physical footprint | valid rounds |
|---|---:|---:|---:|
| type, 400 lines | 0.6060% | 32 MB | 3 |
| type, 30,000 lines | 0.8983% | 49 MB | 3 |
| append, 30,000 lines | 1.3553% | 47 MB | 3 |

These are freshly calibrated absolute observations, not a renderer speedup over the preceding
round. Physical footprint remains distinct from allocated bytes or allocation counts.


A separate GPU pass ran each editor script for thirty seconds and recorded seven seconds
starting three seconds after `SCRIPT_START`. All nine Bunny samples passed actual-state and
input-deadline validation. No CPU ranking was collected during GPU tracing.

| scene | median app GPU | median app + WindowServer | observed total range |
|---|---:|---:|---:|
| type, 400 lines | 0 ms/s | 1.3070 ms/s | 0.7149–1.4563 ms/s |
| type, 30,000 lines | 0 ms/s | 0.7165 ms/s | 0.6971–1.0908 ms/s |
| append, 30,000 lines | 0 ms/s | 1.4104 ms/s | 0.8084–1.5297 ms/s |

The WindowServer column observes the system compositor globally; it is not an exclusive
attribution to this window. A zero app-GPU value does not mean that presenting the window
costs no GPU work. The ranges retain the compositor variation instead of hiding it behind
the median. Frozen executable/resource hashes and collector sources still match after all
three phases.

## The explicit-geometry round (2026-10-07)

Fixture `ad47db9` implements `scene-geometry-v1` with renderer `18eca8e` unchanged.
The same M5 Max, macOS 27.0 and 1680×1050 display at 1×/60 Hz run three interleaved
release rounds. Separate unlocked-window diagnostics establish actual list geometry,
settled wheel travel, exact streaming content and sampled visible canvas motion before
the executables are frozen. They do not establish presentation counts.

All twelve Bunny steady-state samples pass workload, geometry and host admission. CPU
uses two kernel-counter reads; physical footprint is read after that window. Active input
uses seconds 1–9 after script start, rest uses seconds 10–18 after first frame, and the
canvas uses seconds 2–8. Tracing and diagnostic capture are disabled.

| scene | CPU median | physical footprint median | valid samples |
|---|---|---|---|
| table at rest | 0.012239% | 33 MB | 3/3 |
| table under the wheel | 6.993010% | 195 MB | 3/3 |
| chat streaming | 4.141815% | 185 MB | 3/3 |
| looping canvas | 2.070548% | 41 MB | 3/3 |

These are the explicit geometry fixtures, including full-width alternating row backgrounds.
Older non-editor figures above retain their different scene definitions; comparing them
with this table is not a renderer before/after experiment. Footprint is not allocation count.

Fifteen table launches have a median first-frame marker latency of **78.617 ms**
(range **73.338–82.799 ms**). This is an application startup marker, not input-to-photon
or confirmed first-pixel latency. All fifteen launches pass admission.


A separate GPU pass runs thirty-second scripts and records seven seconds beginning
three seconds after script start, or after the first frame for rest and canvas.
All twelve Bunny GPU samples pass workload, geometry and host admission. CPU is not
ranked during this tracing pass. Values below are GPU milliseconds per wall second.

| scene | app median | WindowServer median | app + WindowServer median | observed total range |
|---|---:|---:|---:|---:|
| table at rest | 0.0000 | 0.1025 | 0.1025 | 0.0671–0.9623 |
| table under the wheel | 9.8921 | 20.4738 | 30.3572 | 30.2074–30.3659 |
| chat streaming | 1.6771 | 9.6015 | 11.2786 | 11.2540–11.5524 |
| looping canvas | 0.0000 | 3.1895 | 3.1895 | 2.1253–6.0642 |

WindowServer is the global compositor, not an exclusive charge to this window. Its
variation remains visible in the ranges; the idle totals do not establish a GPU ranking.
The zero app-GPU value for the software canvas does not imply zero presentation cost.
Executable, resource, calibration-evidence and collector hashes match after the complete
CPU and GPU passes. This round calibrates fixtures; it changes no renderer strategy.


## A patch keeps the window's pixel origin

Software patches snap text origins and primitive/clip endpoints in the original window
coordinates before moving them into their small backing. Rounding after translation can
move a half-pixel by one pixel when the local coordinate crosses zero: at scale two,
12.25 points rounds to physical pixel 25; subtracting a 120-pixel patch origin must keep
pixel -95, while rounding the translated -95.5 would produce -96.

The patch uses the same endpoint addition order as the full raster, with unchanged fonts,
radii, stroke widths, admission limits and surface ownership. Exact crop tests cover
fractional text, fills, strokes and nested clips at scales one and two. This is a rendering
correction and makes no performance claim.
