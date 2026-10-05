# Hunkra — источник статусов

Статусы: todo, ready, in_progress, blocked, implemented (нет обязательной проверки), done (критерии доказаны), deferred. Доказательства и проверяющие коммиты добавляются после checkpoint; SHA собственного коммита внутрь него не записывается.

| ID | Приоритет / этап | Статус | Зависимости | Наблюдаемый результат и границы | Приёмка / проверка | План / файлы / доказательства |
|---|---|---|---|---|---|---|
| HNK-001 | P0 / 0 | done | — | Обследование, полный MVP и очередь | Чистое исходное дерево, пути/SHA, DAG без циклов | docs/execplans/first-slice.md; 61a0166 |
| HNK-002 | P0 / 0 | done | HNK-001 | Workspace, стабильные зависимости, общие команды, hooks и CI | fmt, Clippy, тесты, lockfile, hooks; точные версии | Cargo.toml, scripts/dev, docs/dependencies.md |
| HNK-003 | P0 / 0 | done | HNK-001 | Полные Fluent en/ru и ru-dev overlay | Ключи, аргументы, грамматика, числа, офлайн | crates/hunkra/src/i18n.rs, locales/ |
| HNK-004 | P0 / 1 | done | HNK-002 | gix status, HEAD/index/worktree и diff одного файла | done | crates/hunkra-core/, docs/gix-capabilities.md |
| HNK-005 | P0 / 1 | done | HNK-003, HNK-004 | GPUI открыть → выбрать → side-by-side → сменить язык | Сборка, запуск, реальный demo; виртуальные строки, независимые обновления | crates/hunkra/, docs/execplans/first-slice.md |
| HNK-006 | P0 / 1 | in_progress | HNK-005 | Повторный refresh, внешние изменения, быстрый выбор | done | workers, UI, tests |
| HNK-007 | P0 / 2 | todo | HNK-006 | Stage/unstage файла, hunk и выбранных строк | A staged, B/C unstaged → A+B index, C worktree; часть замены, stale diff, блокировки, attributes | Отдельный execplan перед реализацией |
| HNK-008 | P0 / 2 | todo | HNK-007 | Сообщение и Commit без stage-all | Git-оракул commit/index/worktree; hooks/signing/errors | core, UI, tests |
| HNK-009 | P0 / 3 | todo | HNK-008 | Ветки, merge/rebase, состояние continue/abort/skip | Внешние операции, несколько остановок rebase, восстановление после запуска | core, tests, demos |
| HNK-010 | P0 / 3 | todo | HNK-009 | Base/A/B + редактируемый Result | Кириллица, clipboard, selection, undo/redo; save/resolve/continue отдельно; add/add, modify/delete | UI editor, core, tests |
| HNK-011 | P1 / 4 | todo | HNK-008 | Init/clone/fetch/push/upstream/ff-only pull | Локальные remotes, credentials, capability routing, сохранность | core, UI, tests |
| HNK-012 | P1 / 4 | todo | HNK-005 | Порционная история и diff коммита | done | core, UI, tests |
| HNK-013 | P1 / 5 | todo | HNK-010, HNK-011, HNK-012 | Нагрузки и UX, недавние проекты, фильтр, клавиатура | Тысячи файлов, большой файл, массовая перезапись, метрики backend/UI/RAM | benchmarks, UI |
| HNK-014 | P1 / 0,5 | todo | HNK-002 | Release-plz git-only, Fedora-артефакт | done | release-plz.toml, workflows, packaging |
| HNK-015 | P1 / 5 | todo | HNK-013, HNK-014 | Проверка всего MVP и адаптеров Zed | Все MVP-A..F; handoff различает выполненное/частичное | docs/handoff.md, architecture |
| HNK-101 | P2 / post-MVP | deferred | HNK-015 | Встраивание Panel/Item и host GitStore/buffers/remote | Отдельный план; исходный Zed не менять | docs/architecture.md |
| HNK-102 | P2 / post-MVP | deferred | HNK-015 | OAuth/PR/CI, граф, интерактивная история, stash | Отдельные сценарии после MVP | backlog |
| HNK-103 | P2 / post-MVP | deferred | HNK-015 | XML/BSL semantics, AI, plugins, IDE | Вне MVP | backlog |

Критический путь первого GUI: 001 → 002/003 → 004 → 005 → 006. Полный MVP: 007 → 008 → 009 → 010, параллельно независимые 011/012; далее 013/014 → 015. Циклов и отсутствующих ID нет. Ближайшая очередь: 002, 003, 004, 005. Дальние задачи уточняются перед реализацией, исходный объём MVP сохраняется.

Первый slice проверен 2026-10-05: scripts/dev bootstrap, check (15 tests), check-ui, build-ui; native smoke в реальной Wayland-сессии — 11 render passes, файл с staged+unstaged, 3 локали, HEAD→index и index→worktree. Доказательство: docs/validation/first-slice.json. Проверяющий коммит будет указан следующим checkpoint. HNK-006 частична: очереди/поколения/refresh есть; watcher/cache/load metrics не закрыты.
