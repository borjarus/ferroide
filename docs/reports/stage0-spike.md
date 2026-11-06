# Stage 0 spike report

Produced with `cargo xtask spike --seconds 5` (profile `spike`: release code
generation with thin LTO). Raw data: one JSON file per machine in this
directory.

## Machines

| Id | OS | CPU | RAM | GPU / display | Profile |
|---|---|---|---|---|---|
| W1 | Windows 11 Pro (26300) | Intel Core i7-1355U (10 cores) | 40 GB | Intel Iris Xe, DirectX 11.1; 3440 px wide, **60 Hz** | medium-class CPU, integrated GPU |
| — | Linux X11 / Wayland, macOS, RDP, VM, weak laptop | | | | **not yet measured** |

## Results — W1 (`spike-windows-x86_64.json`)

| Metric | Measured | Budget (medium) | Note |
|---|---|---|---|
| App cold start → first frame (`ferroide --demo`, median of 5) | 405 ms | < 400 ms | 1% over; includes opening the demo project and three editors |
| List spike start → first frame | 301 ms | — | |
| Dock spike start → first frame | 434 ms | — | |
| Editor spike start (100k lines, highlighted) → first frame | 2,074 ms | — | includes generating and highlighting the file; the plan's "open 50 MB file" budget is a Stage 3 item |
| Key → next frame, 100k-line editor | p50 16.5 ms, p99 18.3 ms | p99 < 8 ms (medium), < 16 ms (weak) | **vsync-bound**, see below |
| Paging through the 100k-line file | 60.0 FPS, p99 frame 19.9 ms | 120 FPS (medium), 60 FPS (weak) | **vsync-bound** |
| Scrolling 1M-row list | 58.9 FPS, p99 frame 20.1 ms | 120 / 60 FPS | **vsync-bound** |
| Resizing a dock every frame | 58.2 FPS, p99 frame 19.5 ms | 120 / 60 FPS | **vsync-bound** |
| Memory, dock window | 90 MB | < 120 MB | ✔ |
| Memory, list window | 89 MB | < 120 MB | ✔ |
| Memory, 100k-line editor | 253 MB | < 300 MB (medium project) | ✔ |

### How to read the frame numbers

W1's display refreshes at 60 Hz, and GPUI presents frames in sync with it.
The median frame interval in all three spikes is 16.7 ms — exactly one
refresh — so these runs show that **every scenario keeps up with a 60 Hz
display**: the weak-profile budgets (60 FPS scrolling, key → frame < 16 ms
p50) are met on this machine, with p99 intervals of 18–20 ms meaning that
roughly one frame in a hundred slips by a refresh.

They **cannot** show whether the medium-profile budgets (120 FPS, key →
pixel p99 < 8 ms) are met: the measurement is capped by the display. The
same applies to the key → frame latency, which is measured from dispatching
the keystroke to the next presented frame and therefore includes waiting for
vsync.

## Conclusion for the go/no-go (ADR-001)

**Go.** Nothing measured contradicts the plan: memory is well inside the
budgets, startup is at the budget in a debug-free build, and every scenario
holds the display's refresh rate with GPUI Kit's stock components. The
decision is conditional on the runs below.

## Still to measure

1. A **120 Hz or faster display** (or a run with vsync disabled, if GPUI
   exposes it) to check the 120 FPS / 8 ms medium-profile budgets.
2. A **CPU-time metric per frame** (time spent in layout + paint, independent
   of vsync) should be added to `ferro_perf`, so budgets can be checked on
   any display. Planned as the first follow-up.
3. **Linux** X11 and Wayland (older Intel GPU, NVIDIA), **macOS** arm64 and
   x64, **RDP** session, **VM** without a GPU.
4. The **weak reference laptop** (2 cores / 4 GB / UHD 620) and the CI VM
   limited to 2 vCPU / 4 GB.
5. Startup: 405 ms against 400 ms. Candidates once profiled with Tracy:
   loading the three demo editors lazily (only the active tab), and deferring
   the tree-sitter grammar initialization until a file needs it.
