# Требования и границы

Источник: задание владельца от 2026-10-05. Hunkra — Rust/GPUI Git-клиент для актуальной Fedora. Один активный репозиторий. Ядро независимо от GPUI/Zed и публичных gix-типов. gix-first для всех полноценно поддержанных сценариев. Лицензию продукта выбирает владелец; GPL-код Zed не переносить.

Полный MVP не сокращается до первого slice:

- MVP-A: open/recent, branch/detached/unborn, статус staged+unstaged/untracked/deleted/conflict, фильтр и клавиатура. Side-by-side с номерами, выравниванием, общим scroll, hunk navigation и нецветовыми знаками. Unstaged=index→worktree, staged=HEAD→index. Binary/large безопасно ограничены.
- MVP-B: файл/hunk/выбранные +/- строки, часть замены, настоящий index. A staged + B/C unstaged → commit A+B; C остаётся. Unstage не меняет worktree. Снимок/предусловия/locks против внешних изменений. Commit без stage-all, hooks/signing/errors.
- MVP-C: merge/rebase и внешние операции, continue/abort/skip, несколько конфликтных остановок. Base/A/B и независимо редактируемый Result; ручной ввод, Cyrillic, clipboard, selection, undo/redo, взять A/B/оба. Save/resolve/continue отдельно. Modify/modify, add/add, modify/delete, binary limitations. Несохранённый Result переживает locale/refresh, незавершённая операция обнаруживается после перезапуска.
- MVP-D: init/clone, local/remote branches, create/switch, fetch/push/upstream, ff-only pull по умолчанию, порционная история/commit diff с явной базой merge. Credentials helpers/SSH agent; без скрытых stash/reset/discard/force. Подтверждение разрушительных действий.
- MVP-E: Fluent English/Русский/Русский (программистский), полные базовые каталоги, небольшой грамматический overlay. Commit/Фиксация/Коммит; естественные ветки/отмена/сохранение. Русские числа. Все owned UI/error/help тексты переведены. Runtime switch сохраняет файл/строки/message/Result/undo/operation и не сканирует Git. Ресурсы offline, ключи/аргументы проверяются CI, безопасный fallback.
- MVP-F: UI не блокируется Git/diff, ограниченные workers/queues/cache, stale results отвергаются; watcher только инициирует сверку. Виртуальные списки/длинные diff, diff только выбранного файла. Проверки opening/refresh/one change/thousands/mass unchanged rewrite/large/fast switch и метрики backend/prepare/render/RAM. Байты, newline, modes, filters/attributes и данные сохраняются.

Первый slice — реально работающий read-only просмотр; он не является полным MVP. Обязательные writes и merge остаются задачами. Tests пишут только новые временные Git-repos с изолированными identity/hooks/remotes. Разработка: локальные conventional commits, без push/tags/releases. Build artifacts не считаются проверенным GUI. Продуктовый RPM/release и Zed integration имеют отдельные критерии.
