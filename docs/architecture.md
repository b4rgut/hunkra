# Границы реализации

Два crate: hunkra-core (обычные модели Git/status/document/diff, gix adapter), hunkra (GPUI standalone + Fluent + app-local settings). UI получает snapshots и структурированные ошибки. Переводы на границе UI. Блокирующие операции в ограниченном worker; snapshot list и выбранный diff имеют независимые поколения.

В будущей Zed интеграции standalone host заменяется адаптером Panel/Item, GitStore, host buffers/unsaved content и remote projects. Shared view не открывает окна и не читает standalone settings. Второй GitStore не создаётся.

Изучен read-only Zed @ 0c70487f830b10abde93ef75a0772a4552744f15: crates/project/src/git_store.rs (GitStore), crates/workspace/src/dock.rs (Panel), crates/workspace/src/item.rs (Item), crates/git_ui/src/{git_panel,staged_diff,unstaged_diff,text_diff_view}.rs, crates/buffer_diff/src/buffer_diff.rs (BufferDiff/DiffOperations), crates/editor/Cargo.toml. Editor связан с language/text/multi_buffer/project/workspace и не самостоятельный пакет. Код не копировался; GPUI Apache-2.0 — самостоятельная библиотека. API extension panels не считается подтверждённым.

Upstream main checked through official GitHub API on 2026-10-05: 8fda6e46539bdf056e3e326cfcab7deea831ff40. It differs from local 0c70487f; Hunkra does not assume the local clone equals latest. Both expose split GPUI/platform crates on main, whereas the selected official registry 0.2.2 uses Application directly and includes native backends. Source paths and their licenses were read in the local snapshot; no claim that Zed extension APIs provide arbitrary panels. GPL editor/text/workspace have not been copied.

First host has two independent bounded readers (status and selected diff) and a bounded settings writer. Each has at most one active/one replaceable pending/one reply. ViewState rejects older diff generations. Lazy uniform lists render visible files/paired rows. Currently a 2-second reconciliation timer refreshes status and selected diff; this is functional but incremental byte-cache/watcher/load metrics remain HNK-006/013. Refresh with unchanged bytes retains the prior diff identity and selected rows. If actual bytes change, row selection is invalidated.
