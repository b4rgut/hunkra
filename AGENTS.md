# Hunkra

Read TASKS.md and docs/agent-state.md before continuing. Verify cwd, remote, HEAD and git status. Preserve unrelated changes; Zed at ../zed is read-only. No push, tags, releases, PRs, global Git changes or sudo.

## Boundaries

Rust workspace: hunkra-core owns Git and diff models, no GPUI or gix in public contracts. hunkra owns the standalone GPUI shell and bundled Fluent resources. Production Git is gix-first. Document tested APIs and explicit pre-operation fallbacks in docs/gix-capabilities.md. Never retry a failed write with a different backend. Tests write only isolated temporary repositories.

## Checks and commits

Common commands: scripts/dev bootstrap|check-fast|check|check-i18n|check-ui|build-ui|run|demo|check-release; smoke /ABS/REPORT.json requires display and build-ui. Rust tests run through scripts/test.py with isolated Git/home configuration. check-release runs update only inside disposable fixtures, never in this checkout. Every Rust function/method has a doc/comment explaining its purpose. Use cargo fmt, strict Clippy and relevant tests; never silently weaken checks. Cargo.lock is tracked. Two build jobs maximum on this host.

Conventional Commits: feat/fix/perf/refactor/docs/test/build/ci/chore/style/revert, optional scope and breaking changes. Stage exact task paths, inspect staged diff before each local commit. Do not amend other people's history. Update TASKS and agent-state at checkpoints; done requires evidence. GUI compile, headless tests, GUI launch and scenario validation are different outcomes.

## Localization

Embed Fluent resources; en and ru are complete, ru-dev is a small grammatical overlay. All owned UI text/errors have translations. Language changes preserve state and never initiate Git work. Core errors are structured, untranslated. Product licensing is not selected; do not borrow Zed GPL code.

Plans: docs/execplans/first-slice.md, docs/execplans/index-selection.md; full scope: docs/requirements.md.
