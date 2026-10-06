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

Проверено 2026-10-06: fmt/i18n/plan, strict headless/UI Clippy, 142 headless tests
(109 core + 33 app), 47 gui-tests library tests including registered native editor input/clipboard/undo/scroll/page/word keys and host-injected HistoryView/NetworkView; 5 host/executor opening tests, native build.
12 real Wayland history windows, 17 conflict windows, 15 branch/operation и
6 staging/Commit и 19 network/creation windows прошли с независимым Git-оракулом в disposable repos.
Reports: docs/validation/history.json, conflicts.json, operations.json,
index-selection.json, network.json, load.json, ux.json. Seven native load and three UX windows additionally pass. scripts/dev smoke|smoke-operations|smoke-conflicts|
smoke-history|smoke-network|smoke-load|smoke-ux принимают абсолютный report path после build-ui. Native smoke
проверяет render/handlers/input callbacks, не физическую мышь/аппаратный IME.
Owner clipboard не менялся. Directory picker, physical input, X11, изолированные GPU
метрики ещё не сертифицированы. RPM установлен и проверен в rootless clean Fedora container; установки на систему владельца не было. Nested X11 capture был пустым,
временные процессы/failed artifacts удалены.

Async opening callbacks дополнительно проверены: один pending dialog, delayed answer не меняет новый проект, старый error не перекрывает product prompt; cancellation quiet, текущий path сохраняет message/selection/revision без Git read, dirty Result использует Save/Discard/Cancel. Это production host/executor acceptance с injected platform result; actual системный dialog не запускается, поскольку GPUI0.2.2 TestPlatform prompt_for_paths unimplemented. Native3UX+19network регрессий после исправления прошли.

NativeEditor держит один bounded line index на revision. Для больше40 000 отображаемых строк или строки длиннее16 КиБ показывается переведённое ограничение; байты не обрезаются. Copy/replace/undo/redo сохраняют весь текст, а notice не выдаёт фиктивные caret/hit coordinates. Отдельное Wayland окно проверило32КиБ emoji line и2МиБ переводов строк, три языка и возврат обычного просмотра; every private Git/worktree byte invariant. Evidence docs/validation/editor-layout.json, запуск scripts/dev smoke-editor /ABS/report.json. В системный clipboard этот native сценарий не пишет.

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

Fedora44 x86_64 local review artifact: /home/akalsin/Documents/Projects/hunkra/target/rpm/bd8b459/hunkra-0.1.0-1.fc44.x86_64.rpm, clean source `bd8b459aa2a643e8288640c48cc000ada47a854d`, dirty=false, RPM SHA256 `ce7debdd105325110173fdf168bdc847fb70d0a879494ec856dd90824cf3014c`. Он включает host-injected HistoryView/NetworkView, revision-cached Result marker summary, native wheel/page/caret navigation с исправленным preferred column, repository-root recovery и explicit bounded runtime directory maps. Input SHA256 `ce1a0f9d0e37aded2e99cd99b25dfb042c82ac13ea15d2a70317a4011e5fa46b`; stripped packaged binary SHA256 `9d4dee9c4a8538b04ad23a30c162ed2149ce11cf67af40bb598b096e6ca4a4b6`. Повторная сборка одного payload/epoch совпала целиком; все1057 файлов/609 dependency declarations, notices, modes, automatic ELF/GLIBC/OpenSSL requirements, desktop provide и displayless version проверены. Все80 native сценариев в8groups прошли на extracted optimized binary. Fresh rootless pinned Fedora44 nodocs installation/rpm-V/mandatory source/notices/hash passed; reports docs/validation/package-multiclick-check.json/package-multiclick-install.json. Этот RPM включает neighboring grapheme seeking и Ctrl word navigation/deletion; UTF-16 coordinate cache также входит в него; multi-click/drag selections также включены; прежние reports сохраняют свою точную source/binary identity. Product license не выбрана, RPM unsigned/local review; publication и independent full Rust builds across hosts не выполнялись.
Команды упаковки: scripts/dev bootstrap --packaging; scripts/dev package-rpm /ABS/NEW/OUTPUT; scripts/dev check-package /ABS/FILE.rpm /ABS/REPORT.json --native; scripts/dev check-package-install /ABS/FILE.rpm /ABS/INSTALL.json (Podman). Сведения о сборке /usr/share/hunkra/build-info.json и обязательные лицензии /usr/share/licenses/hunkra сохраняются без optional docs. Подготовленный Fedora artifact workflow и local release-plz update/squash/tag gate проверены раздельно; реальные GitHub PR/token/environment/tag-trigger/job остаются непроверенными и запрещены в этом запуске. HNK-014 implemented, HNK-015 in_progress. HistoryView теперь принимает bounded reader хоста; проверен source без локального repo и сохранение readonly message/diff/locale. Architecture содержит текущую проверку Zed a1b71072 и честные границы оставшихся Panel/Item/GitStore/buffer/remote адаптеров (HNK-101 deferred). Продолжение: общая приёмка MVP и проверка подготовленных host boundaries; матрица доказательств docs/validation/mvp-acceptance.md.

Смена проекта сначала проверяет выбранный путь на существующем status worker. Ошибка сохраняет текущие status/diff/filter/message/branch/Result; stale replies не переключают проект. Locale/refresh не заменяют pending target, canonical alias текущего repo сохраняет текст. Dirty Result prompt появляется только для реально прочитанного другого repo; Cancel сохраняет draft, explicit Discard активирует уже проверенный snapshot. Writes/close/creation retire old target reads. Native три UX окна проверяют настоящий неуспешный gix read и все private Git/worktree/settings bytes;5 host/executor tests проверяют generation/cancellation/alias/approval paths. После этого прошли все78 debug native сценариев, strict headless/gui/gui-tests checks и131 headless tests. Evidence validation/opening-ux.json; clean efec339 RPM включает bounded diff и этот opening slice, все78 native сценариев прошли на его extracted binary.

Remotes/creation NetworkView также принимает источник хоста. Standalone factory выбирает gix в main; view не создаёт свой Git service.29lib/5host GPUI tests и131headless tests/strictheadless+gui+gui-tests/build проходят; injected source проверяет отсутствие чтения nonexistent host root, input/selection/revision/locale retention, busy/error/retry и точный Arc в emitted Fetch. Native19network+3UX+12history регрессии прошли. Реальный snapshot guard подготовлен один раз в private Git fixture; это проверка границы чтения/команд, не remote/Zed implementation. Optimized efec339 RPM предшествует этому ownership refactor; debug binary содержит его.

Result сохраняет число marker fragments/ошибку разбора до изменения text revision. Подтверждённая нативная нагрузка1,599,833 bytes/39,999 CRLF rows:17 запросов без изменения текста теперь требуют1 parse вместо17; debug parser wall total~7.114ms вместо~120.212ms в этих двух наблюдениях. Locale/selection/Base/busy не сбрасывают summary, edit/reset/undo/redo сбрасывают. Select/A/B по-прежнему проверяют текущий текст и captured revision, cached ranges/копии source отсутствуют.31lib+5host GPUI/131headless/strict checks/build и native editor+17conflicts+19network+5load+3UX проходят; все private Git/worktree bytes сохранены. Report validation/marker-summary.json содержит обе binary identities; optimized efec339 RPM ещё не включает этот slice или b742b1a reader ownership.

NativeEditor теперь прокручивает Result/исходные стороны/readonly history message, сохраняя текст и курсор: обычное колёсико по вертикали, Shift+колёсико/горизонтальный трекпад по горизонтали. Малые pixel deltas суммируются до строки; смещение ограничено документом и shaped visible widths. Locale/repaint больше не возвращают просмотр к курсору; input/Home/End/стрелки/выбор участка снова показывают курсор.33lib+5host GUI tests/131headless/strict checks/build проходят. Native editor-layout проверяет handler/source-coordinate visibility и неизменность всех private Git/worktree bytes; зарегистрированные wheel/pointer/IME события отдельно проверены на isolated GPUI.17conflicts+12history+6staging/Commit+3UX регрессии прошли. Evidence validation/editor-scroll.json; это functional acceptance без physical/GPU/pixel/speed claims. Проверенный optimized5a1c718 RPM пока предшествует этой прокрутке; обновление артефакта — следующий checkpoint.

При замене/перемещении самого worktree/gitdir/common root старое наблюдение теперь считается потерянным: фактическая периодическая сверка продолжается каждые2s, Refresh подключает watcher заново на существующем worker. Обычная правка вложенного файла остаётся healthy hint. Ошибка сначала воспроизвелась в synthetic/native directory-swap tests; исправление проходит133headless/35lib+5host GUI tests/strict checks/build. Шестой native load case реально перемещает/возвращает private root, сохраняет message/selection/Arc, вызывает actual Refresh и проверяет новый native hint/содержимое после rearm; остальные5load+3UX+editor проходят. Evidence validation/root-watch-load.json; native scenario total теперь79 в8groups. Проверенный699715a RPM с wheel/input включает прежние78cases/clean nodocs install и предшествует root-recovery fix; новый артефакт — следующий checkpoint.

Текущий runtime watcher использует явный nonrecursive набор не более8192папок/16MiB path capacities на поколение, включая внешние parents. Directory create/remove/rename/overflow объединяются в один флаг и bounded recensus на существующем worker; повторная сверка после регистрации покрывает ранние изменения дерева. При превышении лимита native observation освобождается, actual periodic reads остаются2s; Refresh повторяет настройку. Exact root replacement по-прежнему требует явного Refresh. Три новых tests сначала показали рост11→779kernel registrations, затем проверили map limits, automatic new/renamed-child hints и budget recovery.136headless/38lib+5host GPUI/strict checks/build и7load+3UX+editor прошли. Evidence validation/bounded-watch-load.json; native total80. Per-generation map bound не является whole-process RAM limit и не включает asynchronous retirement/library temporary buffers. Проверенный b9ad499 RPM содержит79cases и предшествует runtime bounds; следующий checkpoint обновит артефакт.

Page Up/Down перемещают курсор и viewport нативного редактора на страницу с перекрытием одной строки; Shift расширяет выделение. Движение сохраняет grapheme column через короткие строки, новая text revision сбрасывает предыдущий столбец. Readonly history/source остаются readonly; single-line fields передают page keys родителю, unsupported preview не создаёт фиктивных координат.44GPUI tests/strict GUI checks/build и native editor+12history+17conflicts+6stagingCommit прошли; source/revision/whole fixture Git bytes сохранены. Evidence validation/editor-page.json. Подтверждённый69c8176 RPM содержит runtime watch bounds/all80cases/clean install и предшествует этой навигации; следующий checkpoint обновит артефакт.

Соседние символы TextBuffer теперь ищутся от текущего курсора через GraphemeCursor с полным Unicode context. Повторная нативная проверка32Left/Right handlers в конце1.6MiB Result:4499.240ms →0.096442ms в этих двух debug runs. Это CPU-code wall обработчиков, не полный кадр/GPU/optimized speed. Полные границы и данные проверены на всех byte offsets/UTF8 interiors/EOF, включая сложные Unicode clusters.137headless/45GPUI/strict checks/build и native editor+17conflicts+6stagingCommit+12history+3UX прошли; text/revision/undo/fixture Git bytes сохранены. Evidence validation/grapheme-navigation.json. Проверенный05c252c RPM содержитPage/watcher/all80cases/clean install и предшествует новой cursor seeking оптимизации; следующий checkpoint обновит артефакт.

Ctrl+Left/Right и Shift выделяют слова, Ctrl+Backspace/Delete удаляют выбранный диапазон или соседнее слово. Собственная политика группирует letters/numbers/underscore и punctuation/emoji, пропуская начальные Unicode spaces; точная Zed token parity не заявлена. Grapheme destinations сохраняют Cyrillic/combining/ZWJ/CRLF. Undo удаления восстанавливает исходный cursor, а явного выделения — его направление; readonly поля не редактируются.139headless/48GPUI/strict checks/build и native editor+17conflicts+6stagingCommit+12history+19network+3UX проходят. Evidence validation/editor-word.json; clean720e24d RPM включает прежние grapheme seeking/all80cases/clean install и предшествует word commands. Следующий checkpoint обновит артефакт.

Native input UTF-16 coordinates кешируются scalar-safe pairs по512bytes текущего текста. Edit/reset/composition/undo/redo очищают их; selection/locale/unmark/refused input сохраняют. Whole source копии отсутствуют, максимум2MiB проверен с <=65568pair bytes без allocator overhead. Native debug EOF query: first10.311→4.510ms;32unchanged selected/text callback pairs1596.468→0.155582ms в этих двух последовательных runs. Это CPU-code wall callbacks, не frame/GPU/physical IME.141headless/50GPUI/strict checks/build и editor+17conflicts+6stagingCommit+12history+19network+3UX проходят; данные сохранены. Evidence validation/utf16-coordinates.json. Проверенный3ce8153 RPM содержитWord/grapheme/watcher/all80cases/clean install и предшествует этой оптимизации; следующий checkpoint обновит артефакт.

Double-click выделяет собственный Unicode token в пределах текущей строки, triple-click — полную строку с исходным newline. Перетаскивание продолжает начатое token/line выделение в обе стороны, Shift сохраняет anchor. Captured text revision запрещает перенос старых координат на новый source; mouse release/no-held-left/accepted key прекращает drag.142headless/52GPUI/strict checks/build и native editor+17conflicts+12history+6stagingCommit+3UX проходят. Evidence validation/editor-multiclick.json; source/revision/private Git bytes сохранены. Физические gestures/pixels/exact Zed token policy не сертифицированы. Проверенныйdaf89ec RPM включаетUTF16/Word/watcher/all80cases/clean install и предшествует этому pointer slice; следующий checkpoint обновит артефакт.
