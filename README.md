# Hunkra

Native Rust/GPUI Git client for Fedora. Current checkpoint shows staged/unstaged side-by-side diffs, stages files/hunks/selected +/- lines, creates index-only commits and local branches through gix. Native branch panels also switch branches and control merge/rebase through explicitly selected Git coordinators. English, Russian and programmer Russian switch at runtime. The compact dark interface follows Zed. Editable conflict results, network operations and history remain unfinished; the full MVP is in progress.

```sh
scripts/dev bootstrap
scripts/dev build-ui
scripts/dev demo
scripts/dev run /absolute/path/to/repository
```

On this workspace, prepared demo: `scripts/dev run "$PWD/demo-repositories/preview/changes"`.

Checks: `scripts/dev check`, `scripts/dev check-ui`, `scripts/dev check-editor`, `scripts/dev check-i18n`, `scripts/dev check-release`. Native scenarios: `scripts/dev smoke /absolute/path/to/report.json` and `scripts/dev smoke-operations /absolute/path/to/operations.json` after build-ui (requires display; writes only disposable demos). Bundled UI resources work offline from any working directory. Bootstrap keeps tools and missing Fedora linker files project-local, without sudo.

[What works, launch and known limits](docs/handoff.md) · [Tasks](TASKS.md) · [Full MVP](docs/requirements.md) · [Architecture](docs/architecture.md) · [Verified dependencies](docs/dependencies.md) · [gix-first evidence](docs/gix-capabilities.md) · [Development/release workflow](docs/workflow.md).

Product licensing is not selected yet. No GPL editor code was copied from Zed.
