# Idle presentation lifetime

A quiet window must not retain work solely to keep its pixels visible. The
macOS shell parks its animation and housekeeping clocks when no scene needs
them. Two additional lifetime rules apply to the native presenter:

- A presented `CAMetalDrawable` is retained only until presentation lands.
  After that, `ShownSurface` retains the visible IOSurface and releases the
  drawable wrapper. The visible surface remains protected from purging.
- An empty task queue does not create an alarm. The first actual deadline
  creates one; later empty queues park it and later deadlines reuse it.

The drawable transition also happens when the system previously refused a
purge. Replacing a frame, resizing and dropping the presenter release the
resource appropriate to its current state. The existing presentation-complete
check and first-frame grace interval are preserved.

## Shared task deadlines

Native scenes that call `Runtime::drive_tasks_by_wall` share one monotonic
anchor with the thread-local task executor. Registering a second scene joins
that anchor; each elapsed interval is counted once even when every window
observes it. Equal or backwards observations cannot move the anchor backwards.
A newly created sleeper synchronizes first, so synchronous window work before
`task::sleep` cannot consume part of its requested delay. Each runtime retains
a driver; dropping the last releases automatic wall synchronization. Headless
manual ticks and per-scene animation/touch steps retain their existing contract.

The macOS shell uses this wall-driver path. Other shells' frame-driven task
clocks are unchanged by this repair and need separate native qualification.
Run the injected-clock regressions with `cargo test -p bunny-ui-core --lib
runtime::clock_tests`. The native `shared_task_clock` example requests 600 ms
with one, two, three, then two windows, prints unrounded elapsed observations
and rejects a deadline more than 20 ms early. It is a correctness probe, not an
idle CPU or comparative performance measurement.

## Evidence and scope

On the tested macOS 27 system, the retained drawable kept the system's
FramePacing aggregation timer firing every second after the window stopped
rendering. A native weak-reference test proves that the wrapper disappears
while its IOSurface remains valid; withholding the transition makes the test
fail. A separate native-window check verifies exact pixels across seven
whole-frame resting transitions and subsequent patch/base changes. The
multi-window exercise still completes after the first window closes.

The previously unused task alarm was scheduled to fire hourly even when no
task had ever slept. Its regression fails on the original implementation and
passes when alarm creation is deferred until a real deadline.

This is not an absolute-zero CPU guarantee. The tested Apple GPU driver also
keeps a separate three-second timer; the targeted process observations still
contain nonzero CPU deltas. Other operating-system events can run inside an
idle process. Do not round those deltas into a zero-work claim or infer new
competitive rankings from this lifetime repair.

## Focused verification

```sh
cargo test -p bunny-ui-apple --lib a_landed_frame_keeps_pixels_without_retaining_its_drawable
cargo test -p bunny-ui-macos --lib an_empty_task_queue_never_arms_an_unused_alarm
cargo run --release -p bunny-ui-macos --example shared_task_clock
cargo run --release -p bunny-ui-macos --example two_windows -- --drive
cargo run --release -p bunny-ui-macos --example counter_window
```

Read unrounded process user/system counters only after startup settles.
Timer tracing and CPU observations are separate runs: instrumentation changes
process work. Keep unusual intervals rather than discarding them to obtain a
zero. A CPU-rendering control is a diagnostic, not an adopted rendering policy.
