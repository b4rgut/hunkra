# Проверенные зависимости — 2026-10-05

Версии проверены ответами официального crates.io API `/api/v1/crates/NAME` (max_stable_version), GitHub release API и stable channel manifest Rust. Прямые зависимости закреплены `=version`, транзитивный согласованный граф — Cargo.lock (793 packages, включая targets других платформ). Обязательных `../zed` или плавающих Git-dependencies нет.

| Зависимость / инструмент | Назначение | Последняя проверенная / выбрана | Первичный источник |
|---|---|---|---|
| Rust | compiler, fmt, Clippy | 1.99.0 / 1.99.0 | https://static.rust-lang.org/dist/channel-rust-stable.toml ; https://doc.rust-lang.org/stable/releases.html |
| gpui | официальный native UI | 0.2.2 / 0.2.2 | https://crates.io/api/v1/crates/gpui ; https://docs.rs/gpui/0.2.2/gpui/ |
| gix | repository/status/blobs/filters/diff | 0.88.0 / 0.88.0 | https://crates.io/api/v1/crates/gix ; https://docs.rs/gix/0.88.0/gix/ |
| thiserror | structured errors | 2.0.21 / 2.0.21 | https://crates.io/api/v1/crates/thiserror |
| tempfile | isolated tests | 3.27.0 / 3.27.0 | https://crates.io/api/v1/crates/tempfile |
| fluent-bundle | runtime grammar | 0.16.0 / 0.16.0 | https://crates.io/api/v1/crates/fluent-bundle |
| fluent-syntax | keys/argument validation | 0.12.0 / 0.12.0 | https://crates.io/api/v1/crates/fluent-syntax |
| unic-langid | ru and en identifiers | 0.9.6 / 0.9.6 | https://crates.io/api/v1/crates/unic-langid |
| serde | application settings | 1.0.229 / 1.0.229 | https://crates.io/api/v1/crates/serde |
| serde_json | settings, smoke evidence | 1.0.151 / 1.0.151 | https://crates.io/api/v1/crates/serde_json |
| Lefthook | local hooks | 2.1.17 / 2.1.17 | https://github.com/evilmartians/lefthook/releases/tag/v2.1.17 |
| Cocogitto | conventional messages (no bump/changelog commands) | 7.0.0 / 7.0.0 | https://github.com/cocogitto/cocogitto/releases/tag/7.0.0 |
| release-plz | sole version/changelog manager | 0.3.169 / 0.3.169 | https://github.com/release-plz/release-plz/releases/tag/release-plz-v0.3.169 ; https://release-plz.dev/docs/config |
| actions/checkout | CI checkout | v7.0.1 / 3d3c42e5aac5ba805825da76410c181273ba90b1 | https://github.com/actions/checkout/releases/tag/v7.0.1 |
| actions/upload-artifact | CI binary artifact | v7.0.1 / 043fb46d1a93c77aae656e7c1c64a875d1fc6a0a | https://github.com/actions/upload-artifact/releases/tag/v7.0.1 |

Compiler: rustc 1.99.0 (b940084d7 2026-09-28), pinned in rust-toolchain.toml. Supported compiler floor is deliberately 1.99; **a lower real MSRV has not been established or tested**. gix itself declares 1.88; that does not prove Hunkra or the full UI graph supports 1.88.

## Единственное исключение согласованного графа

First `cargo check -p hunkra --features gui` with latest transitive libc 0.2.190 failed:
`xattr-0.2.3/src/sys/mod.rs:12`, E0425 `libc::ENOATTR` unavailable on Linux. Chain verified with cargo tree: gpui → gpui_http_client → zed-async-tar 0.5.0-zed → xattr 0.2.3. libc 0.2.189 still supplies deprecated ENOATTR=ENODATA; 0.2.190 removes it. `cargo update -p libc --precise 0.2.189` repaired compilation; only this transitive package was constrained. GPUI, all direct crates and Rust remain latest stable. This exception must not be silently removed by cargo update. Next repair condition: a tested official GPUI release/upstream revision eliminates old xattr; then refresh lock and run full checks. No vendored GPL code or impersonated crate versions.

The selected official GPUI release was sufficient after this single transitive compatibility constraint, so no development Git revision was introduced. Native runtime/development libs: installed Fedora 44; missing devel RPMs (fontconfig, freetype, libxcb, libxkbcommon-x11) can be verified and extracted project-locally by bootstrap without sudo. Build uses LIBRARY_PATH through scripts/dev. On this host extracted fontconfig-devel 2.17.0-4.fc44, freetype-devel 2.14.3-1.fc44, libxcb-devel 1.17.0-7.fc44, libxkbcommon-x11-devel 1.13.1-2.fc44.

GPUI 0.2.2 pulls proc-macro-error2 2.0.1 with a future-incompatibility notice under Rust 1.99; current checks succeed. It is recorded, not suppressed. Latest gpui-component 0.7.1 was examined but not added: it requires gpui-pre 0.3.8, a different package; Hunkra uses official GPUI.

Developer assets are Linux x86_64, pinned URLs + release SHA-256 in scripts/bootstrap.py; signatures on local Fedora RPMs are checked. CI action tag SHA resolved from official GitHub API. External workflow execution has not occurred: no push was made.
