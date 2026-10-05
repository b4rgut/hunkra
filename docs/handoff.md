# Hunkra: первый работающий slice

Ветка `codex/bootstrap-mvp`. Исходный checkpoint `a2b566c`, план `61a0166`; проверяющий checkpoint будет записан после первого implementation commit. Полный MVP ещё не готов.

Работает нативное GPUI-окно: открыть реальный repo через диалог/путь при запуске, недавние проекты, branch/unborn/detached, список independent staged/unstaged и conflict, выбрать файл, выровненный side-by-side (HEAD→index / index→worktree), номера/знаки/общая вертикальная прокрутка/hunk navigation. Три Fluent-профиля переключаются без Git-запроса и смены текущего diff/выбора. Автоматическая сверка каждые 2 секунды, F5/Обновить, Up/Down выбрать файл, F7/Shift-F7 между изменениями, Ctrl+O открыть. Двоичные/не-UTF8/большие/gitlink/conflict отображают ограничение, не фиктивный diff. Приложение только просматривает пользовательские репозитории.

Запуск из корня проекта:

```sh
scripts/dev run "$PWD/demo-repositories/changes"
```

Собранный бинарник: `/home/akalsin/Documents/Projects/hunkra/target/debug/hunkra`; его можно запускать из любого каталога с абсолютным путём repo. Ресурсы Fluent встроены. На новом Fedora host `scripts/dev bootstrap` ставит pinned Rust/local tools, извлекает недостающие devel RPMs без sudo; затем `scripts/dev build-ui`. На текущем host всё выполнено, ручное завершение настройки не нужно.

Demo: `/home/akalsin/Documents/Projects/hunkra/demo-repositories/changes` — файл `Изменения с пробелом.bsl`: A уже staged; B/C в worktree. Переключите вкладки и языки; также есть delete/new/binary/BOM+CRLF/no-final-newline. `merge-conflict` — externally started merge; `rebase-conflict` — первая из двух конфликтных остановок rebase. Их операции только обнаруживаются: редактор/resolve/continue пока не реализованы. Новый независимый набор: `scripts/dev demo` либо `scripts/dev demo /NEW/PATH` (существующий путь отвергается).

Выполнены: bootstrap; fmt; strict Clippy core/app и UI all-targets; 15 headless tests; check-i18n (keys/args/overlay/format/plurals); native binary build; finite native Wayland scenario с 11 render passes, обеими comparison tabs и Commit/Фиксация/Коммит (docs/validation/first-slice.json). Это scripted native render, не ручная проверка мышью владельцем. Нативный directory picker, clipboard/editor и Fedora RPM пока не автоматизированы/не проверены. Current future-incompatibility notice proc-macro-error2 2.0.1 фиксируется в dependencies.md.

Все реализованные production Git reads/diff/clean conversion — gix 0.88.0. CLI fallback нет; Git используется независимым оракулом тестов и generator demos. Write/mutation executors ещё не выбраны, gix-first сохраняется. Точные latest версии, первичные источники и единственное transitive exception libc 0.2.189 — docs/dependencies.md. Real lower MSRV не установлен; поддерживаемый compiler floor 1.99.

Ограничения: длинные строки пока обрезаются в панели, горизонтальная прокрутка ожидает HNK-013; diff до 2 MiB / 40k total lines / 16k byte line. Timer сейчас повторяет status/diff чтение; watcher/cache и сравнимые нагрузки ещё нужны. Partial/file staging, Commit, история/network/ветки, полноценные merge/rebase и editable Result пока недоступны. Установка/запись translations не меняет repo. Пользовательскую лицензию не выбирали.

Следующие: HNK-006 incremental reconciliation/cache; HNK-007 безопасный настоящий index + частичный выбор; HNK-008 Commit с hooks/signature; HNK-009/010 merge/rebase и Result. Release token/environment/GitHub ruleset не настроены; ни push, ни release не выполнялись. RPM — HNK-014; подготовленный CI не исполнялся удалённо.
