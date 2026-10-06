# Backend load evidence 2026-10-06

Fedora 44 x86_64, Rust 1.99.0, locked gix 0.88.0, debug build. Three process-local rounds per fresh synthetic fixture; filesystem cache is warm after fixture creation and Git oracle. Baseline and cached collectors ran sequentially after native regressions completed. This is a local measurement, not a product speed claim or native GPU/paint measurement.

3003 tracked files; 3000 identical BOM/CRLF/EOF rewrites; 8000 untracked files; 18000-line selected diff; >2MiB explicit preview refusal. Independent Git uses --no-optional-locks; index/HEAD/status and representative worktree bytes are invariant. All six scenarios pass in both collector modes.

| Scenario | Full status bundle median ms | Uncached diff median ms | Cached diff median ms | Cache-hit prepare median ms | Cached RSS KiB | Retained cache bytes |
|---|---:|---:|---:|---:|---:|---:|
| first-open-and-repeat-refresh | 199.72 | 19.86 | 19.30 | 0.004 | 14104 | 22803 |
| one-change-and-fast-switch | 198.13 | 19.48 | 19.63 | 0.005 | 13884 | 22885 |
| 3000-unchanged-content-rewrites | 198.81 | 19.56 | 19.38 | 0.005 | 13676 | 22885 |
| 8000-untracked-files | 228.03 | 19.46 | 19.45 | 0.004 | 22312 | 22885 |
| 18000-line-diff | 208.66 | 33.50 | 22.56 | 0.107 | 29460 | 5836434 |
| oversized-file-refusal | 206.41 | 20.19 | 19.96 | 0.143 | 27992 | 4194736 |

Detailed samples: validation/perf-baseline.json and validation/perf-cached.json. Total/source/prepare times are separate in the cached collector. Small-file total remains dominated by source/index/filter validation; no significant small-file improvement is claimed. Larger repeated alignment avoids row/string reconstruction. RSS includes the whole headless process and allocator behavior; cache budget counts retained buffer/collection capacities, excluding allocator overhead and active/external shared snapshots.

Reproduce with scripts/dev measure /ABS/baseline.json --uncached and scripts/dev measure /ABS/cached.json. The finite collector writes only its own TemporaryDirectory and requested report. No owner repo/settings/remotes mutation.

Cache acceptance: five new core tests, 124 headless tests (107 core + 17 app), strict headless/UI Clippy, native build and all 69 prior native windows pass. Same-size/same-mtime external replacements, index/HEAD/mode changes, fresh attrs/filter output/failure, two hashes/comparison modes, conflict resolution, another repository and LRU/byte budgets are independently checked. Worker/UI share immutable Arc alignment, not deep row copies.

HNK-006 refresh/load acceptance now passes; HNK-013 path filter/focus/recent/long-line UX remains in progress.

## Native Wayland load evidence

Five finite debug-build windows, each with 3000 tracked BOM/CRLF/EOF files and initially 4000 untracked files. Independent Git preserves HEAD/index/stages and representative bytes; fixture external edits are asserted separately. All 69 prior native scenario regressions also pass. Report validation/load.json; reproduce with scripts/dev smoke-load /ABS/report.json after build-ui.

| Scenario | Final changed paths | Max visible files / diff rows | Status service mean ms | Main preparation mean ms | Next-frame callback mean ms | RSS KiB | Watch hints |
|---|---:|---:|---:|---:|---:|---:|---:|
| load-watch | 4003 | 19 / 1 | 113.48 | 0.307 | 35.07 | 116676 | 1 |
| load-mass | 5003 | 19 / 1 | 113.59 | 0.308 | 35.33 | 110952 | 1 |
| load-large | 4003 | 19 / 13 | 117.54 | 0.339 | 37.14 | 127624 | 0 |
| load-periodic | 4003 | 19 / 1 | 117.75 | 0.310 | 33.44 | 110304 | 0 |
| load-draft | 4003 | 16 / 0 | 124.66 | 0.372 | 41.41 | 110068 | 1 |

Main/visible-row values measure wall time spent in CPU preparation code; next-frame callback includes scheduling/vsync, not isolated GPU/paint execution. No physical input/X11/GPU performance claim. load-periodic intentionally has no native hints and still observes actual changed bytes through periodic reads. load-draft retains unsaved Result/selection while refusing stale external writes. Cache-hit alignment remains shared; changed content invalidates old row coordinates. Initial watch setup budgets do not promise a global bound on future library/OS directory maps.
