# HNK-006/HNK-013

Полный scope: actual external reconciliation, bounded cache/queues, stale rejection,
recent/filter/keyboard UX, native virtual/horizontal diff and measured load. No
backend superiority or GPU timing inferred from a headless benchmark.

First collect the current read-only debug baseline in fresh temporary repositories:
3003 tracked files, first open/three repeat scans, one byte change, fast switching,
3000 rewrites of identical BOM/CRLF/EOF contents, 8000 untracked files, 18000-line
aligned diff and >2MiB refusal. Independent Git oracle uses no optional locks, so
its status query cannot itself refresh the fixture index. Record scan/bundle/diff
latency and process RSS/peak RSS separately; assert index/HEAD/worktree bytes.

Then implement a bounded selected-diff cache that always rereads exact guarded
sources and applies current attributes/filters before reuse. Byte timestamps are
never the source of truth; writing guards stay exact. Reuse alignment/row allocations
only on equal path/kind/documents/index/HEAD/raw worktree/mode. Fixed entry and
allocated-byte limits; errors cannot return old content as new. Worker owns the
cache; no render Git calls or unbounded pending jobs.

Watcher events are hints to a debounced actual status/selected-diff refresh, with
periodic reconciliation for dropped events/unsupported filesystems. Linked gitdir
and common metadata must be watched too. Setup/walk runs off the UI thread; event
storage is a bounded flag, not paths accumulated forever. Read access events must
not create a self-refresh loop. No owner sysctl/global settings changes.

Native acceptance must record service/source/alignment timing, UI prepare/render
CPU scope and memory distinctly. Thousands of files must render visible rows only,
large-file presentation remains explicit, fast switches discard obsolete results,
unchanged refresh retains selections, external same-size/mtime replacement changes
content and stale coordinates are refused. Dirty Result/locales/messages survive.

UX follows the owner's Zed direction: compact dark tabs/panels/status bar, path
filter and focused keyboard navigation, recent repos, readable horizontally long
diff with synchronized panes. No GPL Zed editor/workspace copying.

Current primary watcher API checked 2026-10-06:
[notify 8.2.0](https://docs.rs/notify/8.2.0/notify/). Its documented large-tree/event
loss limits require periodic reconciliation, not trusting event lists.

First cache/load acceptance complete: five core tests, 124 headless and strict headless/UI Clippy/build; all 69 prior native windows pass. DiffReader keeps <=4 entries/24MiB retained capacities, freshly converts sources/guards each hit and shares Arc rows with UI. Collector modes reproduce uncached/cached paths. Six backend scenarios pass sequentially after native tests; docs/validation/performance.md has separate samples/source/prepare/RSS and scope limits. Watcher/native load/UX remain required.

Native refresh acceptance complete: four watcher tests (128 total headless), strict headless/UI Clippy, two isolated GPUI input tests and build; five real Wayland load windows plus all 69 prior native regressions pass. Exact same-size/mtime replacement, 3000 unchanged rewrites +1000 new paths, 18000-line virtual diff/oversized refusal/fast selection, degraded periodic reconciliation and externally stale dirty Result are independently checked. Status/service/source/alignment, visible preparation/callback latency and RSS are separately reported in validation/load.json and performance.md. HNK-006 done; HNK-013 path filter/focus/recent and synchronized long-line presentation remain required.
