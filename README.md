# Hunkra

Native Rust/GPUI Git client for Fedora. Staged/unstaged side-by-side diffs, file/hunk/line staging, index-only commits and local branches use gix. Native panels control switch/merge/rebase, edit independent conflict Results and keep Save, Mark resolved and Continue separate. History loads bounded pages and compares a commit with an explicitly selected parent. English, Russian and programmer Russian switch at runtime. The compact dark interface follows Zed. Native Init/Clone/Remotes control gix fetch and explicit push/upstream/ff-only pull. Native watcher/cache/load and path/focus/recent/long-line UX checks pass; an optimized Fedora RPM has been assembled and native payload checks pass. Local RPM payload/reproducibility, all78 packaged native scenarios and clean-container installation are checked; full MVP acceptance and the external release chain remain incomplete.

```sh
scripts/dev bootstrap
scripts/dev build-ui
scripts/dev demo
scripts/dev run /absolute/path/to/repository
```

On this workspace, prepared demo: `scripts/dev run "$PWD/demo-repositories/preview/changes"`.

Checks: `scripts/dev check`, `scripts/dev check-ui`, `scripts/dev check-editor`, `scripts/dev check-i18n`, `scripts/dev check-release`. Backend load measurements: `scripts/dev measure /absolute/report.json` (add `--uncached` for the control path). Native scenarios: `scripts/dev smoke`, `smoke-operations`, `smoke-conflicts`, `smoke-history`, `smoke-network`, `smoke-load`, `smoke-ux` or `smoke-editor`, each followed by an absolute report path, after build-ui (requires display; writes only disposable demos). Bundled UI resources work offline from any working directory. Bootstrap keeps tools and missing Fedora linker files project-local, without sudo.

Fedora packaging: `scripts/dev bootstrap --packaging`, then `scripts/dev package-rpm /absolute/new/output`. Inspect the actual RPM with `scripts/dev check-package /absolute/file.rpm /absolute/report.json --native`; `scripts/dev check-package-install /absolute/file.rpm /absolute/install.json` verifies installation in a rootless private Fedora container (requires Podman). Product licensing remains unselected; no publication is performed.

[What works, launch and known limits](docs/handoff.md) · [Tasks](TASKS.md) · [Full MVP](docs/requirements.md) · [Architecture](docs/architecture.md) · [Verified dependencies](docs/dependencies.md) · [gix-first evidence](docs/gix-capabilities.md) · [Development/release workflow](docs/workflow.md).

Product licensing is not selected yet. No GPL editor code was copied from Zed.
