# Границы реализации

Два crate: hunkra-core (обычные модели Git/status/document/diff, gix adapter), hunkra (GPUI standalone + Fluent + app-local settings). UI получает snapshots и структурированные ошибки. Переводы на границе UI. Блокирующие операции в ограниченном worker; snapshot list и выбранный diff имеют независимые поколения.

В будущей Zed интеграции standalone host заменяется адаптером Panel/Item, GitStore, host buffers/unsaved content и remote projects. Shared view не открывает окна и не читает standalone settings. Второй GitStore не создаётся.

Изучен read-only Zed @ 0c70487f830b10abde93ef75a0772a4552744f15: crates/project/src/git_store.rs (GitStore), crates/workspace/src/dock.rs (Panel), crates/workspace/src/item.rs (Item), crates/git_ui/src/{git_panel,staged_diff,unstaged_diff,text_diff_view}.rs, crates/buffer_diff/src/buffer_diff.rs (BufferDiff/DiffOperations), crates/editor/Cargo.toml. Editor связан с language/text/multi_buffer/project/workspace и не самостоятельный пакет. Код не копировался; GPUI Apache-2.0 — самостоятельная библиотека. API extension panels не считается подтверждённым.
