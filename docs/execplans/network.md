# HNK-011

Полный объём: init/clone, local/remote branches, fetch/push с явным upstream,
ff-only pull. Один serialized writer и close/draft guards. Native inputs и
runtime locale сохраняют выбор/текст. Ошибки structured, credentials/URLs/raw
transport output не сохраняются в UI/log. Existing credential helpers/SSH agent,
без собственного OAuth. Tests только temporary local remotes/servers/helpers.

Executor audit 2026-10-06, exact gix 0.88.0:

- init: ThreadSafeRepository::init_opts + explicit branch/hash, gix.
- clone/fetch: blocking client, curl/OpenSSL HTTPS, configured credentials/SSH;
  PrepareFetch::fetch_then_checkout/main_worktree и Remote::connect/prepare_fetch/
  receive. Worktree/pack threads ограничены двумя. UI не делает сеть при render.
- push: gix::push содержит только enum Default, нет send-pack/receive-pack
  coordinator. CLI выбирается до записи; full explicit branch refspec без force,
  pre-push hooks/credentials. Нет fallback после failed gix operation.
- ff-only integration: existing audited merge/worktree coordinator gap; fetch
  через gix, затем отдельный guarded merge --ff-only immutable upstream target.
  Divergence не превращается в merge/rebase/stash/reset.

Clone принимает новый destination, не перезаписывает существующий. Failed fetch/
checkout оставляет partial repo для inspection вместо recursive cleanup чужих
возможных файлов. Init сохраняет existing worktree files и отвергает existing
.git/symlink. Config/upstream writes только local physical config с guard/lock;
include files/global config не переписываются. Fetch spec не может писать в
текущую local branch; tags/prune/remote helpers отдельно аудируются по API.

Приёмка: SHA-1/SHA-256 local bare remotes, clone/unborn/packed, dirty index/worktree
fetch invariance, upstream+push обычный/rejection/pre-push hook, pull ff/diverged/
dirty/untracked/stale/linked/locks. Credentials callback/local HTTPS auth и SSH
context проверяются isolated fixtures. UI реальными Wayland handlers; это не
physical hardware certification. Partial failure не означает отсутствие writes;
refresh actual state обязателен и backend retry не выполняется.

Verified primary sources: [gix clone](https://docs.rs/gix/0.88.0/gix/fn.prepare_clone.html),
[PrepareCheckout](https://docs.rs/gix/0.88.0/gix/clone/struct.PrepareCheckout.html),
[Remote](https://docs.rs/gix/0.88.0/gix/struct.Remote.html),
[push module](https://docs.rs/gix/0.88.0/gix/push/index.html),
[Git pull](https://git-scm.com/docs/git-pull). Core acceptance: nine new isolated integration tests; 109 total headless tests
(92 core + 17 app), strict headless/UI Clippy, two GPUI input tests and build-ui pass.
The new feature graph passes all 50 prior native regression windows
(12 history, 17 conflicts, 15 operations, six staging/Commit). HTTPS fixture
verifies CA refusal/trust and helper get/store; SSH command shim verifies transport
configuration and linked/detached SHA-256 fetch, not real-agent authentication.
Config comments/mode/includes/foreign lock are independently asserted. Fresh clone
post-checkout hook and UTF-16 refusal before worktree writes are tested. Fetch
counts actual new/fast-forward/forced updates, including a zero-update second fetch.

Implemented: init/clone/remote models/fetch/set-upstream through gix. Fetch disables
tags and refuses non-remote-tracking destinations; no prune/FETCH_HEAD behavior is
claimed. Configured reference-transaction hooks are refused before connection;
post-checkout is honored. Effective insteadOf transport rewrites are rechecked
before connection. At the first checkpoint push/pull and native controls remained required; HNK-011 stays
in_progress. Partial data survives cancellation/failure without automatic retry.

Push/pull core acceptance (2026-10-06): 119 headless tests, strict headless/UI
Clippy and build-ui. Push routes once to `PushCoordinator`, explicit full local and
remote branches, no force/mirror/follow-tags/prune/submodule expansion. Actual Git
2.55 requires `-c remote.NAME.mirror=false`; `--no-mirror` alone does not override
that configured default. Override is command-local. Groups/multiple endpoints and
ambiguous override keys are refused. Helpers, configured signatures and pre-push
remain Git-owned; real hook/server/non-fast-forward refusals do not write upstream.
Both pipes drain with fixed memory (8MiB hook fixture). Optional upstream uses the
separate guarded gix config transaction only after confirmed success; an already
pushed remote is not rolled back after a later config failure. Include/global
branch upstream values are explicitly refused rather than duplicated/ineffectively
overridden by local edits; unrelated includes/comments/mode are preserved.

Pull routes gix fetch first, then `FastForwardWorktreeCoordinator` for immutable
actual upstream commit. Fixed ff-only/no-autostash/no-squash/no-overwrite-ignore
protects dirty/untracked/ignored collisions; divergence leaves fetched tracking refs
for inspection but never starts merge/rebase/stash. Linked SHA-256 index/HEAD/common
config tested without changing primary worktree. Pre-cancel/stale/detached/missing
upstream refuse before transport. An accepted CLI child is waited, not killed;
force termination is not a cancellation promise. Cooperative ref/index/config
locks do not constitute a universal lock against external editing.

Real OpenSSH 10.2p1 + a private ssh-agent/loopback sshd verify clone/fetch/push/pull
in both object formats and strict known-host refusal. Fixture generates keys and
uses its own socket/trust/config; all server/agent children are reaped. Production
uses the existing configured agent. gix-transport 0.60.0 forwards unknown SSH
stderr directly; before handshake Hunkra replaces the not-yet-started SSH transport
using the same configured trusted command/variant with stderr suppression and
separate positional URL arguments. A 4MiB/sentinel subprocess assertion and the
real-agent fixture validate this gix route. No process-wide fd/environment mutation.
Primary manuals: [Git push](https://git-scm.com/docs/git-push),
[Git merge](https://git-scm.com/docs/git-merge),
[ssh-agent](https://man.openbsd.org/ssh-agent), [sshd](https://man.openbsd.org/sshd).
Native network UI remains required. Removal condition for CLI coordinators is
HNK-104: upstream gix must expose complete coordinators that pass the same hook,
locking, worktree, credential/signing and native acceptance matrices.
