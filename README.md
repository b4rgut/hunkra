# Hunkra

Native Rust/GPUI Git client for Fedora. Current checkpoint previews real staged/unstaged side-by-side diffs and stages/unstages files, hunks or selected +/- lines through gix. English, Russian and programmer Russian switch at runtime. Commit and merge editing remain unfinished; this is not the full MVP.

```sh
scripts/dev bootstrap
scripts/dev build-ui
scripts/dev demo
scripts/dev run /absolute/path/to/repository
```

On this workspace, prepared demo: `scripts/dev run "$PWD/demo-repositories/preview/changes"`.

Checks: `scripts/dev check`, `scripts/dev check-ui`, `scripts/dev check-i18n`, `scripts/dev check-release`. Native scenarios: `scripts/dev smoke /absolute/path/to/report.json` after build-ui (requires display; writes only disposable demos). Bundled UI resources work offline from any working directory. Bootstrap keeps tools and missing Fedora linker files project-local, without sudo.

[What works, launch and known limits](docs/handoff.md) · [Tasks](TASKS.md) · [Full MVP](docs/requirements.md) · [Architecture](docs/architecture.md) · [Verified dependencies](docs/dependencies.md) · [gix-first evidence](docs/gix-capabilities.md) · [Development/release workflow](docs/workflow.md).

Product licensing is not selected yet. No GPL editor code was copied from Zed.
