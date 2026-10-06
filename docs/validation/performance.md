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

HNK-006/013 remain in progress: watcher/debounce/periodic reconciliation, native prepare/render/RAM/virtualization load metrics, path filter/focus/recent/long-line UX and full performance acceptance are still required.
