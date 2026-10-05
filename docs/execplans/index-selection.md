# HNK-007 / HNK-016 — выбранные строки в настоящий index

Checkpoint первого GUI: b0c3699. Дальнейшая задача сохраняет полную семантику MVP-B; Commit остаётся HNK-008.

1. Guard diff: root, exact index digest/presence, HEAD, raw worktree bytes+mode; чтение сверяется в конце.
2. До записи проверяется поддержка index/selected mode и post-index-change hook. Непроверенные variants безопасно отвергаются; blanket CLI fallback отсутствует.
3. gix-lock index.lock; новый gix read под lock; compare guards/documents; выбранные + и − строки независимо; unstage не пишет worktree. Write blob + stage-zero entry, remove stale tree cache, checksum, atomic index commit. Повторная сверка источников непосредственно перед commit lock.
4. Tests Git oracle: A staged/B+C worktree, части replacements, additions/deletions/new/unborn, CRLF/no newline, lock/stale worktree/index/HEAD, tree-cache, no fallback. Изменения только disposable repos.
5. UI: selected added/deleted lines, file/hunk actions and explicit stage/unstage labels; writer queue serial, request snapshot immutable. Во время write refresh deferred. Ошибки translated; safe refusal без ложного success.
6. Checks → docs/checkpoint; HNK-007 не done, пока полноценные variants/required tests отсутствуют.

Предел гарантии: index.lock координирует совместимые Git writers. Внешние приложения не предоставляют общий lock рабочих файлов; гарантируется применение к точному подтверждённому снимку и сохранность worktree, с последней проверкой перед atomic index replacement. Не обещать абсолютное исключение некооперативной внешней записи между final check и rename. Неподдержанные hooks/advanced indexes отражаются отдельно, не оправдывают CLI на поддержанном simple path.

Результат HNK-016: scripts/dev check (32 tests), check-ui, build-ui и два настоящих native Wayland сценария прошли. Первый — диапазоны +/- → три локали → stage B → staged view → unstage hunk B; независимый Git проверяет обе версии index и неизменный worktree. Второй — закрытие окна сразу после принятия write; выход после завершения, A+B в index, worktree цел и index.lock отсутствует. Воспроизведение: scripts/dev smoke /ABS/REPORT.json; отчет docs/validation/index-selection.json.

Поддержанные варианты и отказы — gix-capabilities.md. HNK-007 остаётся частичной: HNK-017 уточнит семантику независимой половины EOF replacement и расширение hook/advanced-index вариантов. Текущая половина без newline, которая склеила бы две строки, отвергается до lock; полная пара сохраняет EOF. GUI не создаёт Commit — HNK-008.

Проверяющий implementation checkpoint: 78e0081 — feat(staging): update selected index lines through gix.
