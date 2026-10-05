# Первый проверяемый сквозной сценарий

Цель: запустить GPUI на Fedora, открыть настоящий repo, отличить HEAD→index и index→worktree, выбрать файл и увидеть выровненный side-by-side; сменить три локали без смены данных.

Места: Cargo workspace (core/app), bundled locales, scripts/dev, integration tests. Общие manifests/lockfile и план принадлежат текущему агенту; работы последовательны.

1. Проверить crates.io/официальные API; зафиксировать stable Rust и GPUI без ../zed.
2. Добавить Fluent каталог/overlay и проверки keys/args/plurals.
3. gix status/head/index/blob/worktree, явные limits и структурированные ошибки; тесты Git-оракулом.
4. Вычислять выровненный diff только выбранного файла. Worker вне UI, ограниченные pending задания и поколения запросов.
5. GPUI окно: open, list, tabs, row numbers/signs, shared scroll, locales, refresh.
6. Настоящие demo staged+unstaged, binary/Unicode/CRLF. Сборка/launch/scenario отдельно.
7. Саморевью, check, docs, checkpoint; затем решить следующую ready-задачу.

Приёмка: реальные версии проверены независимым Git; viewing не меняет index/worktree; en/ru/ru-dev показывают одинаковые данные; GUI запуск подтверждён отдельно. Остатки MVP не маркируются done.

Риски: GPUI системные build dependencies (использовать разрешённые локальные извлечённые пакеты при необходимости), runtime GPU/display, большая compilation/RAM (jobs=2). Не переносить Zed editor: его GPL/workspace dependencies не дают независимого permissive widget. Реальное редактирование нужно исследовать до merge stage.
