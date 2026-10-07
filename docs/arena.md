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
