# Hunkra

Native Rust/GPUI Git client for Fedora. Current checkpoint is a working **read-only preview**, not the full MVP: inspect real staged and unstaged side-by-side diffs in English, Russian or programmer Russian.

```sh
scripts/dev bootstrap
scripts/dev build-ui
scripts/dev demo
scripts/dev run /absolute/path/to/repository
```

On this workspace, prepared demo: `scripts/dev run "$PWD/demo-repositories/changes"`.

Checks: `scripts/dev check`, `scripts/dev check-ui`, `scripts/dev check-i18n`, `scripts/dev check-release`. Bundled UI resources work offline from any working directory. Bootstrap keeps tools and missing Fedora linker files project-local, without sudo.

[What works, launch and known limits](docs/handoff.md) · [Tasks](TASKS.md) · [Full MVP](docs/requirements.md) · [Architecture](docs/architecture.md) · [Verified dependencies](docs/dependencies.md) · [gix-first evidence](docs/gix-capabilities.md) · [Development/release workflow](docs/workflow.md).

Product licensing is not selected yet. No GPL editor code was copied from Zed.
