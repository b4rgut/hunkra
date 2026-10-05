# Продолжение

Ветка codex/bootstrap-mvp, исходный a2b566c; план 61a0166. Сверять git log/status и TASKS.md. Origin https://github.com/b4rgut/hunkra.git; push/PR/tag/release запрещены. Исходное дерево чистое, Git identity настроена и не менялась. Zed read-only /home/akalsin/Documents/Projects/zed @ 0c70487f830b10abde93ef75a0772a4552744f15; upstream SHA и API-различия в architecture.md.

Первый slice HNK-002..005 проверен: Rust 1.99.0 + официальный GPUI 0.2.2/gix 0.88.0. scripts/dev bootstrap, check (15 tests), check-ui, build-ui прошли. Native Wayland smoke прошёл: docs/validation/first-slice.json (11 render passes, 3 locales, staged/unstaged). UI не выполняет Git writes. demo-repositories/{changes,merge-conflict,rebase-conflict} существуют локально и gitignored. Binary target/debug/hunkra собран. Полный MVP не готов.

Незавершённое: HNK-006 refresh/queues/generations реализованы, cache/watcher/performance measurements остаются; horizontal long-line UX ограничен. Следующий ID HNK-006, затем API audit HNK-007 (index writes и stale snapshot), HNK-008 (hooks/signature-aware Commit), HNK-009/010 (merge/rebase + настоящий editor). Не менять продуктовую лицензию.

Общие команды: scripts/dev check, check-ui, build-ui, run /ABS/REPO, check-i18n, demo (new path only), check-release. jobs=2. Локальные devel libs извлечены bootstrap без sudo; scripts/dev устанавливает LIBRARY_PATH. Cargo.lock фиксирует libc 0.2.189: GPUI→old xattr не компилируется с 0.2.190; не обновлять молча. Будущий MSRV ниже 1.99 не проверен.

Release config/workflows подготовлены и локально static-validated; реальная цепочка не запускалась. Token/environment/ruleset/RPM — внешние/поздние задачи. Не выдавать draft workflows за проверенный release. Code copied from Zed: none.
