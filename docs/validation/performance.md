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

HNK-006 refresh/load acceptance now passes; HNK-013 path filter/focus/recent/long-line UX native acceptance also passes (validation/ux.json).

## Native Wayland load evidence

Five finite debug-build windows, each with 3000 tracked BOM/CRLF/EOF files and initially 4000 untracked files. Independent Git preserves HEAD/index/stages and representative bytes; fixture external edits are asserted separately. All 69 prior native scenario regressions also pass. Report validation/load.json; reproduce with scripts/dev smoke-load /ABS/report.json after build-ui.

| Scenario | Final changed paths | Max visible files / diff rows | Status service mean ms | Main preparation mean ms | Next-frame callback mean ms | RSS KiB | Watch hints |
|---|---:|---:|---:|---:|---:|---:|---:|
| load-watch | 4003 | 16 / 1 | 120.01 | 0.503 | 44.15 | 121980 | 1 |
| load-mass | 5003 | 16 / 1 | 114.46 | 0.445 | 41.40 | 111912 | 1 |
| load-large | 4003 | 16 / 11 | 131.14 | 0.444 | 44.86 | 127104 | 0 |
| load-periodic | 4003 | 16 / 1 | 133.77 | 0.511 | 41.79 | 115340 | 0 |
| load-draft | 4003 | 14 / 0 | 142.54 | 0.512 | 53.82 | 114756 | 1 |

Main/visible-row values measure wall time spent in CPU preparation code; next-frame callback includes scheduling/vsync, not isolated GPU/paint execution. No physical input/X11/GPU performance claim. load-periodic intentionally has no native hints and still observes actual changed bytes through periodic reads. load-draft retains unsaved Result/selection while refusing stale external writes. Cache-hit alignment remains shared; changed content invalidates old row coordinates. Initial watch setup budgets do not promise a global bound on future library/OS directory maps.

Current native report regenerated with the final UX graph; a separate two-job release compilation was active during the load runs. Latencies include that host load and are not a before/after speed comparison. Quiet native remeasurement remains part of final packaging/acceptance.

## Quiet optimized RPM measurement

All five scenarios also passed on 2026-10-06 from the stripped optimized cfd73e9 RPM binary, SHA256 `5e59f971f1ea1b4e2a65794ab3de2f93ecea798c87914a8989d7d7f14a1740dd`. No agent compilation, packaging/container operation or other native acceptance run competed with this run. Fixture creation and Git oracle warm the filesystem cache; desktop/system load is not controlled. These are observations from one finite run, not a controlled comparison with the earlier debug measurements. Full samples and source identity: validation/load-release.json.

| Scenario | Status service mean ms | Selected diff mean ms | Main preparation mean ms | Next-frame callback mean ms | Peak RSS KiB | Visible files / diff rows |
|---|---:|---:|---:|---:|---:|---:|
| load-watch | 22.60 | 1.24 | 0.100 | 18.18 | 94764 | 16 / 1 |
| load-mass | 24.62 | 1.07 | 0.089 | 15.51 | 87712 | 16 / 1 |
| load-large | 23.03 | 2.64 | 0.069 | 15.58 | 99748 | 16 / 11 |
| load-periodic | 23.51 | 1.02 | 0.089 | 17.27 | 87584 | 16 / 1 |
| load-draft | 24.48 | 3.93 | 0.087 | 16.51 | 87388 | 14 / 0 |

Independent HEAD/index/stages/worktree oracle passes in every fixture. No-event fallback still reconciles actual bytes; dirty Result stays intact. GPU execution, physical input, cold-cache startup and cross-host reproducibility remain unmeasured.
