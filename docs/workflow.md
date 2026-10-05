# Проверки, история и будущий выпуск

`main` — собираемые checkpoints. Рабочие изменения — codex/*, PR в будущем с conventional title, squash merge. Required check рекомендуемый `headless-and-ui`; для одного владельца второй reviewer не требуется. GitHub ruleset и release environment этим запуском не включались.

Локальные hooks Lefthook: pre-commit=check-fast; commit-msg=Cocogitto; pre-push=headless check. Они не форматируют и не stage-ят. История Hunkra проверяется от первого task-owned conventional checkpoint 61a0166 включительно; унаследованный a2b566c с сообщением `first commit` не переписывается. Cocogitto bump/changelog не используется. release-plz — единственный version/changelog manager.

CI использует те же scripts/dev команды, pinned checkout/upload actions, contents:read, без secrets на pull_request. Headless checks и компиляция/линковка UI есть в подготовленной конфигурации; remote runs не были выполнены.

release-plz git_only=true/publish=false, общий product version_group; internal core не создаёт tags/releases, app создаёт единственный vX.Y.Z. App changelog включает core; locales лежат внутри app package. Safe check-release читает конфигурацию и проверяет политику, не вызывает update/release. Парсер/поведение update тестируется только в одноразовой копии; static check не выдаётся за реальный выпуск.

Цепочка подготовлена: вручную разрешённый workflow_dispatch release-pr → принятие PR владельцем → dispatch release → v* tag → artifact workflow. `release_always=false` запрещает обычному main commit становиться выпуском. Нужны owner-configured release environment и HUNKRA_RELEASE_TOKEN с contents/PR permissions (app installation token или fine-grained token). Обычный GITHUB_TOKEN не считается решением: созданные им PR/tag не запускают ожидаемые workflows. В этом запуске secrets не создавались, push/PR/tag/publication не выполнялись. Артефакт workflow пока бинарный Linux файл; Fedora RPM остаётся HNK-014.
