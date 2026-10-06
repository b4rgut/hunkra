# Hunkra: запуск и фактическое состояние

Ветка `codex/bootstrap-mvp`. Проверены staging, index-only Commit, локальные ветки,
switch/merge/rebase, независимый Result, порционная история и native Init/Clone/Remotes. Полный MVP ещё не готов:
Локальный Fedora RPM проверен; remote release chain и общая приёмка остаются в TASKS.md.

```sh
scripts/dev build-ui
scripts/dev demo
scripts/dev run "$PWD/demo-repositories/preview/changes"
```

Готовый `/home/akalsin/Documents/Projects/hunkra/target/debug/hunkra` принимает
абсолютный путь repo из любого cwd. На новом Fedora host сначала scripts/dev
bootstrap: pinned Rust/local tools и verified devel RPMs извлекаются локально без
sudo. Ресурсы встроены, соседний Zed не нужен. Тёмные компактные панели, вкладки
и status bar следуют запросу владельца; GPL editor/workspace не копировались.

В Changes доступны staged/unstaged/conflict flags, HEAD→index / index→worktree
с выровненными строками, +/- и EOF, общим vertical scroll и hunk navigation.
Файл/hunk/независимые +/- строки записывают настоящий index; worktree не меняется.
Shift+click выбирает диапазон на своей стороне. В demo `Изменения с пробелом.bsl`
имеет A в index, B/C только в worktree: подготовьте обе стороны B, введите любое
сообщение и Commit содержит A+B, оставляя C. Unstage B возвращает index к A.
Commit hooks/signing работают; ошибка сохраняет текст без unsigned retry.

В Branches отдельные create/switch/merge/rebase действия; выбор строки не делает
checkout. Continue/Abort/Skip (rebase) идут по фактической незавершённой операции.
В demo merge-conflict/rebase-conflict откройте конфликт: A / Result / B, Base
toggle и участки A/B/A→B/B→A. Result принимает собственный текст и undo/redo.
Save меняет только рабочий файл; затем Mark resolved отдельно пишет index;
Continue выполняется отдельно. После новой правки resolved Result требуется
новый Mark resolved. Несохранённый draft переживает refresh/locale/History,
изменившийся внешний файл запрещает stale write. Dirty navigation/close и
Abort/Skip имеют подтверждения. Binary/UTF-16 сохраняются и разрешаются внешним
редактором, с явным ограничением native text writes.

History показывает 100-row pages (cache восемь порций), сообщение и изменённые
файлы выбранного commit. Root сравнивается с empty tree; у merge выбирается
настоящий parent с полным OID. Raw Git blobs/modes не используют нынешние attrs.
Rename показан как delete/add. Просмотр read-only; staging/Commit выключены.
F5 начинает новую историю от текущего HEAD; фоновый refresh не сдвигает её tip.
Legacy/invalid metadata объясняются явно, без старого сообщения под новым commit.

Ctrl+1/2/3/4 переключают Changes/Branches/History/Remotes; F5 refresh, Up/Down selection,
F7/Shift+F7 hunk, Ctrl+O open, Ctrl+Enter Commit, Ctrl+Shift+Enter Continue.
Три Fluent-профиля сохраняют message/Result/selection/undo без Git-запросов.
Все accepted writes сериализованы; обычное закрытие ждёт writer. Ошибка отменяет
pending close и сохраняет пользовательский ввод.

Проверено 2026-10-06: fmt/i18n/plan, strict headless/UI Clippy, 129 headless tests
(107 core + 22 app), 26 gui-tests library tests including 3 isolated GPUI input/clipboard/undo/bubbling tests and a host-injected native HistoryView test, native build.
12 real Wayland history windows, 17 conflict windows, 15 branch/operation и
6 staging/Commit и 19 network/creation windows прошли с независимым Git-оракулом в disposable repos.
Reports: docs/validation/history.json, conflicts.json, operations.json,
index-selection.json, network.json, load.json, ux.json. Five native load and three UX windows additionally pass. scripts/dev smoke|smoke-operations|smoke-conflicts|
smoke-history|smoke-network|smoke-load|smoke-ux принимают абсолютный report path после build-ui. Native smoke
проверяет render/handlers/input callbacks, не физическую мышь/аппаратный IME.
Owner clipboard не менялся. Directory picker, physical input, X11, изолированные GPU
метрики ещё не сертифицированы. RPM установлен и проверен в rootless clean Fedora container; установки на систему владельца не было. Nested X11 capture был пустым,
временные процессы/failed artifacts удалены.

gix 0.88.0 читает status/diff/filters/history, пишет index/objects/refs/commit и
resolve. Git CLI заранее выбран только для audited complete switch/merge/rebase
coordinator gaps и explicit push/ff-only integration; failed gix write никогда не повторяется через CLI. Git также
независимый test oracle. Exact guards/locks сохраняют данные; универсальный lock
внешнего редактора не обещается. Split/sparse/shared/SKIP_WORKTREE и прочие
сложные index variants имеют объяснимый отказ; см. docs/gix-capabilities.md.
Diff ограничен 2MiB / 40k total lines / 16k byte line, status 100k changed files,
commit metadata 1MiB, commit changed files 50k. Длинные строки имеют общий горизонтальный сдвиг обеих сторон; gutters остаются на месте. Никакого превосходства по скорости не заявлено.

Rust 1.99.0, GPUI 0.2.2, gix 0.88.0 и Fluent 0.16.0 проверены по exact locked
sources (docs/dependencies.md). libc 0.2.189 сохраняется из-за воспроизведённой
GPUI→old xattr регрессии на 0.2.190. proc-macro-error2 future notice не скрыт;
lower MSRV не проверен. release-plz core-only/locales-only fixtures проходят,
remote release chain не выполнялась. Push/PR/product tags/releases отсутствуют,
Zed read-only. Лицензию продукта выбирает владелец.

Init/Clone открываются верхними кнопками, destination вводится полным путём. Init сохраняет existing project files, Clone требует новой папки и сохраняет partial data после ошибки. В Remotes выбираются remote/целевая ветка, отдельно Fetch, Set upstream, Push и ff-only Pull. SSH agent/helpers существующие, OAuth не требуется. Push одной explicit ветки без force/mirror/tags; отслеживание записывается после подтверждённого успеха. Pull сначала gix fetch, затем только fast-forward; dirty/diverged/untracked/ignored/locks дают явный отказ без stash/rebase. Ctrl+Shift+F/P/L — Fetch/Push/Pull, Ctrl+Enter в creation — создать и открыть. Все четыре inputs/selection/undo сохраняются при смене языка и отказе. Dirty Result не теряется; Fetch его сохраняет, смена проекта требует Save/Discard/Cancel. Unsupported remote helpers/ref-transaction fetch hooks/UTF-16 clone checkout/included upstream values — explicit limits; production trace/transport output/URL в reports не сохраняются.

Native external refresh: coalesced watcher hints request actual reads, periodic reconciliation survives lost/unavailable events. Linked worktree/common metadata included, symlink targets excluded. Same timestamp/size does not justify content reuse; four-entry/24MiB alignment cache validates actual bytes/attrs/filters/index/HEAD each hit. Five native load fixtures and six backend control/cached scenarios distinguish service/source/alignment, visible preparation, next-frame callback latency and RSS. Evidence docs/validation/performance.md; isolated GPU time is unmeasured. Manual Refresh retries failed watch setup.

Path filter: Ctrl+F, case-insensitive AND terms, arrows navigate visible paths; Enter retains the highlighted document and returns focus to Changes. Query/selection/revision/draft survive locales/refresh; matching uses its own bounded worker with status-prepared paths, no render/Git filtering. Clear restores the list without losing the selected document. Tab/Shift+Tab or F6 cycle native fields/Result/message; Escape returns focus. Ctrl+R opens Recent from an active workspace; arrows/Enter select, Escape closes. Choosing the current project preserves the draft/message and makes no Git request; another project uses dirty Result Save/Discard/Cancel. Changes/History share synchronized horizontal text offsets via Shift+wheel, trackpad horizontal, Alt+Left/Right/Home or visible buttons. Only visible lines are shaped; actual glyph widths bound scroll, resized viewports clamp it, gutters/vertical alignment stay fixed. History Ctrl+Up/Down navigates changed files. Native smoke launches from an arbitrary cwd and preserves every fixture worktree/settings byte; physical input/pixel/X11 certification remains unclaimed.

Fedora44 x86_64 local review artifact: /home/akalsin/Documents/Projects/hunkra/target/rpm/cfd73e9/hunkra-0.1.0-1.fc44.x86_64.rpm, source checkpoint cfd73e9, dirty=false, SHA256395b8fb5... . Optimized /home/akalsin/Documents/Projects/hunkra/target/release/hunkra также запускается абсолютным путём и имеет --version без display. RPM assembler проверяет версии, automatic ELF/GLIBC/OpenSSL deps и desktop provide, не перезаписывает существующие файлы. Две сборки одного payload/epoch совпали целиком; input/stripped binary hashes различаются и записаны отдельно. Stripped binary SHA2565e59f971... совпадает с предыдущим review artifact, на котором прошли все77 native сценариев. Every1057payloadfile/609dependency declarations и notices проверены. Rootless clean pinned Fedora44 image установил чистый пакет с tsflags=nodocs; rpm-V/mandatory licenses/MPL source/build info/icon/launcher/version/hash прошли. Reports docs/validation/package-clean-check.json/package-clean-install.json; прежний package.json сохраняет77native evidence на идентичном бинарнике. Пять тихих optimized load окон прошли отдельно (load-release.json/performance.md), без параллельной работы сборки агента. Этот RPM предшествует новым HNK-015 изменениям источника HistoryView; debug build содержит их. Product license не выбрана, RPM unsigned/local review; publication не было. Independent full Rust builds across hosts не сравнивались.

Команды упаковки: scripts/dev bootstrap --packaging; scripts/dev package-rpm /ABS/NEW/OUTPUT; scripts/dev check-package /ABS/FILE.rpm /ABS/REPORT.json --native; scripts/dev check-package-install /ABS/FILE.rpm /ABS/INSTALL.json (Podman). Сведения о сборке /usr/share/hunkra/build-info.json и обязательные лицензии /usr/share/licenses/hunkra сохраняются без optional docs. Подготовленный Fedora artifact workflow и local release-plz update/squash/tag gate проверены раздельно; реальные GitHub PR/token/environment/tag-trigger/job остаются непроверенными и запрещены в этом запуске. HNK-014 implemented, HNK-015 in_progress. HistoryView теперь принимает bounded reader хоста; проверен source без локального repo и сохранение readonly message/diff/locale. Architecture содержит текущую проверку Zed a1b71072 и честные границы оставшихся Panel/Item/GitStore/buffer/remote адаптеров (HNK-101 deferred). Продолжение: async picker/editor limits и общая приёмка MVP, затем обновлённый optimized artifact.
