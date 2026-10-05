# HNK-010 / HNK-022 / HNK-023

Цель полного MVP: нативные A слева / независимый Result в центре / B справа,
read-only Base по переключателю. Собственный компактный GPUI интерфейс по образцу
Zed, без переноса GPL editor/workspace. Работать до просьбы владельца остановиться.

HNK-022: gix stage 1/2/3 и точные OID/modes; отсутствующая сторона не пустой файл.
Result — raw worktree bytes, источники переводятся официальным gix filter pipeline
в worktree representation, без lossy UTF-8. Save только атомарно сохраняет файл
или явно выбранное удаление. Mark resolved отдельно меняет один index path на
stage 0 / deletion, сохраняя REUC своих и других путей. Continue остаётся отдельным
координатором HNK-021. Все действия привязаны к точным HEAD/index/worktree/state
guards; foreign locks и неподдержанные форматы до записи. Binary/encoding/modes
получают объяснение, внешнее решение доступно через refresh.

Участки: bounded parser существующих Git merge/diff3/zdiff3 markers (в том числе
custom marker size), точные ranges, A/B и оба явных порядка. Общие строки не
дублируются; EOF/newline не добавляется скрыто. При неполных/неоднозначных markers
автовыбор участка недоступен, ручной Result остаётся возможен. Отсутствие markers
не снимает nonzero index stages: требуется явный Mark resolved.

HNK-023: native selection/clipboard/Unicode/undo, file/chunk navigation, A/B
provenance особенно rebase. Locale/status refresh сохраняют draft/selection/undo;
save, resolve, continue — три действия. Dirty navigation/close и destructive
abort/skip требуют собственного product confirmation с save/discard/cancel;
не спрашивать владельца разрешение в чате на обычную разработку.

Приёмка: SHA-1/SHA-256, linked worktree, modify/modify, add/add, modify/delete
keep/delete, BOM/CRLF/no-final-newline, attrs/filters, stale/locks, external
resolution/restart, обе остановки rebase. Headless/core, isolated input и реальные
Wayland сценарии проверяются раздельно; physical keyboard/IME и X11 не заявляются
без фактической проверки. После каждого checkpoint обновить TASKS/agent-state.

Проверяющий HNK-021 checkpoint: 5ad92be (`feat(operations): control merge and rebase
from native branch panels`). HNK-022 verified 2026-10-06: 15 new conflict tests, 84 total headless, strict headless/UI Clippy; HNK-023 native UI verified below.

Source references: [Git index / REUC](https://git-scm.com/docs/gitformat-index), [Git merge stages / markers](https://git-scm.com/docs/git-merge), exact Git 2.55 source read-cache.c mode preference 2/1/3. Current locked gix-filter 0.35.0 worktree/encode_to_worktree.rs calls encoding_rs Encoder, which uses UTF-8 output for UTF-16 labels; the independent Git cat-file --filters oracle exposed this gap. Those checkout sides are explicitly unsupported before editor writes; external binary/UTF-16 resolution is certified. No change to libc pin or dependencies.

HNK-023 acceptance 2026-10-06: 88 headless tests, 2 isolated input tests, strict Clippy, build-ui; 17 real Wayland Result runs plus 21 prior native regressions pass. See TASKS.md and docs/validation/conflicts.json for actual matrices. Result entities retain text/selection/undo across refresh/locale/Base toggle; a stale draft stays visible and cannot overwrite external changes. Product dirty/delete/abort/skip prompts are separate from developer permission. Save-before-navigation waits for confirmed filesystem persistence. A later Save of an already resolved Result sets result_staged=false and Continue remains blocked until explicit Mark resolved updates stage zero; REUC sources stay recoverable. No clipboard owner mutation or physical input/X11 claim.
