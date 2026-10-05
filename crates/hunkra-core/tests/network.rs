mod support;
use hunkra_core::network::{
    NetworkCapability, ObjectFormat, PushRequest, clone_repository, fetch_remote, init_repository,
    prepare_network, pull_ff_only, push_branch, set_upstream,
};
use hunkra_core::{CoreError, prepare_commit};
use std::{fs, sync::atomic::AtomicBool};
use support::Repo;

/// Reads one full immutable object id through independent Git.
fn oid(repo: &Repo, name: &str) -> String {
    String::from_utf8(repo.git(&["rev-parse", name]))
        .unwrap()
        .trim()
        .to_owned()
}

#[test]
/// Native init preserves existing project bytes and rejects reinit, symlink metadata and invalid branch input.
fn init_preserves_existing_files_and_selects_hash_branch_before_objects() {
    for (format, hash) in [
        (ObjectFormat::Sha1, "sha1"),
        (ObjectFormat::Sha256, "sha256"),
    ] {
        let owner = Repo::new();
        let root = owner.path().join("new-project");
        fs::create_dir(&root).unwrap();
        fs::write(root.join("файл"), b"\xef\xbb\xbfproject\r\nEOF").unwrap();
        assert_eq!(
            init_repository(&root, "feature/ветка", format)
                .unwrap()
                .backend,
            "gix"
        );
        assert_eq!(
            fs::read(root.join("файл")).unwrap(),
            b"\xef\xbb\xbfproject\r\nEOF"
        );
        assert_eq!(
            owner.git(&[
                "-C",
                root.to_str().unwrap(),
                "rev-parse",
                "--show-object-format"
            ]),
            format!("{hash}\n").as_bytes()
        );
        assert_eq!(
            owner.git(&["-C", root.to_str().unwrap(), "symbolic-ref", "HEAD"]),
            "refs/heads/feature/ветка\n".as_bytes()
        );
        let before = fs::read(root.join(".git/HEAD")).unwrap();
        assert!(matches!(
            init_repository(&root, "other", format),
            Err(CoreError::RepositoryDestination)
        ));
        assert_eq!(before, fs::read(root.join(".git/HEAD")).unwrap());
        let invalid = owner.path().join("invalid");
        assert!(matches!(
            init_repository(&invalid, "-wrong", format),
            Err(CoreError::BranchName)
        ));
        assert!(!invalid.exists());
        #[cfg(unix)]
        {
            let linked = owner.path().join("linked-dir");
            fs::create_dir(&linked).unwrap();
            std::os::unix::fs::symlink(root.join(".git"), linked.join(".git")).unwrap();
            assert!(matches!(
                init_repository(&linked, "other", format),
                Err(CoreError::RepositoryDestination)
            ));
            assert_eq!(before, fs::read(root.join(".git/HEAD")).unwrap());
        }
    }
}

#[test]
/// gix clones exact hashes/blobs/modes and configures origin/upstream without trusting current owner repositories.
fn clone_populated_and_unborn_preserves_remote_objects() {
    for format in ["sha1", "sha256"] {
        let source = Repo::with_format(format);
        source.write("файл", b"\xef\xbb\xbfCyrillic\r\nEOF");
        source.write("binary", b"raw\0blob");
        source.write("exec", b"#!/bin/sh\nexit 0\n");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(
                source.path().join("exec"),
                fs::Permissions::from_mode(0o755),
            )
            .unwrap();
            std::os::unix::fs::symlink("файл", source.path().join("link")).unwrap();
        }
        source.initial();
        let parent = tempfile::tempdir().unwrap();
        let destination = parent.path().join("clone");
        let cancel = AtomicBool::new(false);
        assert_eq!(
            clone_repository(source.path().to_str().unwrap(), &destination, &cancel)
                .unwrap()
                .backend,
            "gix"
        );
        let snapshot = prepare_network(&destination).unwrap();
        assert_eq!(
            snapshot.repository.repository.head.as_deref(),
            Some(oid(&source, "HEAD").as_str())
        );
        assert_eq!(snapshot.current_branch.as_deref(), Some("main"));
        assert_eq!(
            snapshot.upstream.as_ref().unwrap().branch,
            "refs/heads/main"
        );
        assert_eq!(
            snapshot.upstream.as_ref().unwrap().tracking_ref.as_deref(),
            Some("refs/remotes/origin/main")
        );
        assert!(snapshot.remotes[0].fetch_supported && snapshot.remotes[0].push_supported);
        assert_eq!(
            fs::read(destination.join("файл")).unwrap(),
            fs::read(source.path().join("файл")).unwrap()
        );
        assert_eq!(fs::read(destination.join("binary")).unwrap(), b"raw\0blob");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_ne!(
                fs::metadata(destination.join("exec"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o111,
                0
            );
            assert_eq!(
                fs::read_link(destination.join("link")).unwrap(),
                std::path::Path::new("файл")
            );
        }
        assert!(
            source
                .command(&["-C", destination.to_str().unwrap(), "fsck", "--full"])
                .status
                .success()
        );
        assert!(
            source
                .git(&["-C", destination.to_str().unwrap(), "status", "--porcelain"])
                .is_empty()
        );
        let before = prepare_commit(&destination).unwrap();
        assert!(matches!(
            clone_repository(source.path().to_str().unwrap(), &destination, &cancel),
            Err(CoreError::RepositoryDestination)
        ));
        assert_eq!(before, prepare_commit(&destination).unwrap());
    }
    let source = Repo::new();
    let parent = tempfile::tempdir().unwrap();
    let destination = parent.path().join("empty");
    clone_repository(
        source.path().to_str().unwrap(),
        &destination,
        &AtomicBool::new(false),
    )
    .unwrap();
    assert!(prepare_commit(&destination).unwrap().head.is_none());
    assert_eq!(
        prepare_network(&destination).unwrap().remotes[0].name,
        "origin"
    );
}

#[test]
/// Fetch advances only remote tracking refs while exact staged/unstaged bytes and HEAD stay reviewed.
fn fetch_keeps_dirty_index_and_worktree_for_both_hashes() {
    for format in ["sha1", "sha256"] {
        let source = Repo::with_format(format);
        source.write("f", b"base\n");
        source.initial();
        let parent = tempfile::tempdir().unwrap();
        let destination = parent.path().join("clone");
        let cancel = AtomicBool::new(false);
        clone_repository(source.path().to_str().unwrap(), &destination, &cancel).unwrap();
        fs::write(destination.join("f"), b"index\n").unwrap();
        source.git(&["-C", destination.to_str().unwrap(), "add", "f"]);
        fs::write(destination.join("f"), b"worktree\r\nEOF").unwrap();
        let before = prepare_commit(&destination).unwrap();
        let index = fs::read(destination.join(".git/index")).unwrap();
        let snapshot = prepare_network(&destination).unwrap();
        source.write("f", b"incoming\n");
        source.git(&["add", "f"]);
        source.git(&["commit", "-m", "incoming"]);
        let result = fetch_remote(&snapshot, "origin", &cancel).unwrap();
        assert_eq!(result.backend, "gix");
        let actual = prepare_network(&destination).unwrap();
        assert_eq!(actual.repository.repository, before);
        assert_eq!(fs::read(destination.join(".git/index")).unwrap(), index);
        assert_eq!(fs::read(destination.join("f")).unwrap(), b"worktree\r\nEOF");
        assert_eq!(
            actual
                .branches
                .iter()
                .find(|branch| branch.name == "origin/main")
                .unwrap()
                .id,
            oid(&source, "HEAD")
        );
        assert_eq!(
            fetch_remote(&actual, "origin", &cancel)
                .unwrap()
                .updated_refs,
            0
        );
    }
}

#[test]
/// Config/refs are guarded; upstream writes preserve includes/comments, modes and index/worktree bytes.
fn upstream_transactions_preserve_project_and_refuse_stale_lock_symlink() {
    let repo = Repo::new();
    repo.write("f", b"base\n");
    repo.initial();
    repo.git(&["remote", "add", "origin", repo.path().to_str().unwrap()]);
    repo.write("included", "[hunkra]\n\tvalue = preserved\n");
    repo.git(&[
        "config",
        "include.path",
        repo.path().join("included").to_str().unwrap(),
    ]);
    let config_path = repo.path().join(".git/config");
    let mut config_bytes = fs::read(&config_path).unwrap();
    config_bytes.extend_from_slice(b"\n# Keep this local comment.\n");
    fs::write(&config_path, config_bytes).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&config_path, fs::Permissions::from_mode(0o640)).unwrap();
    }
    let before = prepare_commit(repo.path()).unwrap();
    let snapshot = prepare_network(repo.path()).unwrap();
    set_upstream(&snapshot, "origin", "feature/ветка").unwrap();
    assert!(
        fs::read(&config_path)
            .unwrap()
            .windows(b"# Keep this local comment.".len())
            .any(|window| window == b"# Keep this local comment.")
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(&config_path).unwrap().permissions().mode() & 0o777,
            0o640
        );
    }
    assert_eq!(
        repo.git(&["config", "--get", "branch.main.remote"]),
        b"origin\n"
    );
    assert_eq!(
        repo.git(&["config", "--get", "branch.main.merge"]),
        "refs/heads/feature/ветка\n".as_bytes()
    );
    assert_eq!(
        repo.git(&["config", "--get", "hunkra.value"]),
        b"preserved\n"
    );
    assert_eq!(before, prepare_commit(repo.path()).unwrap());
    assert!(matches!(
        set_upstream(&snapshot, "origin", "main"),
        Err(CoreError::Stale)
    ));
    let snapshot = prepare_network(repo.path()).unwrap();
    repo.write(".git/config.lock", "foreign");
    assert!(matches!(
        set_upstream(&snapshot, "origin", "main"),
        Err(CoreError::ConfigLock)
    ));
    assert_eq!(
        fs::read(repo.path().join(".git/config.lock")).unwrap(),
        b"foreign"
    );
    fs::remove_file(repo.path().join(".git/config.lock")).unwrap();
    repo.write("included", "[hunkra]\n\tvalue = changed\n");
    assert!(matches!(
        set_upstream(&snapshot, "origin", "main"),
        Err(CoreError::Stale)
    ));
    #[cfg(unix)]
    {
        let config = repo.path().join(".git/config");
        let outside = repo.path().join("outside-config");
        fs::rename(&config, &outside).unwrap();
        std::os::unix::fs::symlink(&outside, &config).unwrap();
        let bytes = fs::read(&outside).unwrap();
        assert!(matches!(
            prepare_network(repo.path()),
            Err(CoreError::NetworkConfig)
        ));
        assert_eq!(bytes, fs::read(outside).unwrap());
    }
}

#[test]
/// Failure/cancellation never retries or deletes a partial clone, and unsupported targets cannot execute helpers.
fn failures_leave_partial_clone_and_reject_unsafe_fetch_destinations() {
    let source = Repo::new();
    source.write("f", "base\n");
    source.initial();
    let parent = tempfile::tempdir().unwrap();
    let destination = parent.path().join("failed");
    assert!(matches!(
        clone_repository(
            parent.path().join("missing").to_str().unwrap(),
            &destination,
            &AtomicBool::new(false)
        ),
        Err(CoreError::NetworkExecution)
    ));
    assert!(destination.join(".git/HEAD").is_file());
    let canceled = parent.path().join("canceled");
    assert!(matches!(
        clone_repository(
            source.path().to_str().unwrap(),
            &canceled,
            &AtomicBool::new(true)
        ),
        Err(CoreError::NetworkCanceled)
    ));
    assert!(!canceled.exists());
    assert!(matches!(
        clone_repository("ext::echo secret", &canceled, &AtomicBool::new(false)),
        Err(CoreError::NetworkTarget)
    ));
    assert!(!canceled.exists());
    source.git(&["remote", "add", "origin", source.path().to_str().unwrap()]);
    source.git(&[
        "config",
        "remote.origin.fetch",
        "+refs/heads/*:refs/heads/*",
    ]);
    let snapshot = prepare_network(source.path()).unwrap();
    let before = prepare_commit(source.path()).unwrap();
    assert!(matches!(
        fetch_remote(&snapshot, "origin", &AtomicBool::new(false)),
        Err(CoreError::NetworkConfig)
    ));
    assert!(matches!(
        fetch_remote(&snapshot, "-option", &AtomicBool::new(false)),
        Err(CoreError::NetworkTarget)
    ));
    assert_eq!(before, prepare_commit(source.path()).unwrap());
    assert_eq!(
        source
            .diff("f", hunkra_core::DiffKind::Unstaged)
            .right
            .bytes,
        b"base\n"
    );
    source.git(&[
        "remote",
        "set-url",
        "origin",
        "https://TOKEN:SECRET@example.invalid/project",
    ]);
    let snapshot = prepare_network(source.path()).unwrap();
    let printed = format!("{snapshot:?}");
    assert!(
        !printed.contains("TOKEN")
            && !printed.contains("SECRET")
            && !printed.contains("example.invalid")
    );
}

struct HttpFixture(std::process::Child, u16);
impl HttpFixture {
    /// Launches one loopback CGI oracle with a private root; the owner always kills and waits for it.
    fn new(root: &std::path::Path) -> Self {
        use std::io::BufRead;
        let certificate = root.join("certificate.pem");
        let key = root.join("key.pem");
        assert!(
            std::process::Command::new("openssl")
                .args([
                    "req",
                    "-x509",
                    "-newkey",
                    "rsa:2048",
                    "-nodes",
                    "-days",
                    "1",
                    "-subj",
                    "/CN=127.0.0.1",
                    "-addext",
                    "subjectAltName=IP:127.0.0.1",
                    "-out"
                ])
                .arg(&certificate)
                .arg("-keyout")
                .arg(&key)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .unwrap()
                .success()
        );
        let mut child = std::process::Command::new("python3")
            .arg(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/tests/support/smart-http.py"
            ))
            .arg(root)
            .arg(certificate)
            .arg(key)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let mut port = String::new();
        std::io::BufReader::new(child.stdout.take().unwrap())
            .read_line(&mut port)
            .unwrap();
        Self(child, port.trim().parse().unwrap())
    }
}
impl Drop for HttpFixture {
    /// Reaps the fixture server on success or panic, leaving no continuation process.
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
/// Real curl HTTPS verifies a fixture CA, uses helpers and stores accepted credentials without terminal input.
fn https_fetch_authenticates_with_existing_credential_helper() {
    let source = Repo::new();
    source.write("f", "incoming\n");
    source.initial();
    let root = tempfile::tempdir().unwrap();
    source.git(&[
        "clone",
        "--bare",
        source.path().to_str().unwrap(),
        root.path().join("repo.git").to_str().unwrap(),
    ]);
    let server = HttpFixture::new(root.path());
    let destination = Repo::new();
    destination.git(&[
        "remote",
        "add",
        "origin",
        &format!("https://127.0.0.1:{}/repo.git", server.1),
    ]);
    let before = prepare_commit(destination.path()).unwrap();
    let snapshot = prepare_network(destination.path()).unwrap();
    assert!(matches!(
        fetch_remote(&snapshot, "origin", &AtomicBool::new(false)),
        Err(CoreError::NetworkExecution)
    ));
    assert_eq!(before, prepare_commit(destination.path()).unwrap());
    destination.git(&[
        "config",
        "http.sslCAInfo",
        root.path().join("certificate.pem").to_str().unwrap(),
    ]);
    let helper = destination.path().join("credentials");
    let calls = destination.path().join("helper-actions");
    destination.write("credentials", format!("#!/bin/sh\necho \"$1\" >> '{}'\nif [ \"$1\" = get ]; then\nprintf 'username=fixture\\npassword=secret\\n'\nfi\n", calls.display()));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&helper, fs::Permissions::from_mode(0o755)).unwrap();
    }
    destination.git(&["config", "credential.helper", helper.to_str().unwrap()]);
    let snapshot = prepare_network(destination.path()).unwrap();
    let before = prepare_commit(destination.path()).unwrap();
    fetch_remote(&snapshot, "origin", &AtomicBool::new(false)).unwrap();
    assert_eq!(before, prepare_commit(destination.path()).unwrap());
    assert_eq!(
        oid(&destination, "refs/remotes/origin/main"),
        oid(&source, "HEAD")
    );
    let actions = fs::read_to_string(calls).unwrap();
    assert!(actions.lines().any(|line| line == "get"));
    assert!(actions.lines().any(|line| line == "store"));
    assert!(!format!("{:?}", prepare_network(destination.path()).unwrap()).contains("secret"));
}

#[test]
/// Fetched attributes are audited before UTF-16 checkout can emit incorrect bytes; partial Git data stays inspectable.
fn clone_refuses_unsupported_utf16_before_worktree_write() {
    let source = Repo::new();
    source.write(".gitattributes", "f working-tree-encoding=UTF-16LE\n");
    let bytes: Vec<_> = "текст\r\n"
        .encode_utf16()
        .flat_map(u16::to_le_bytes)
        .collect();
    source.write("f", &bytes);
    source.initial();
    let root = tempfile::tempdir().unwrap();
    let destination = root.path().join("clone");
    assert!(matches!(
        clone_repository(
            source.path().to_str().unwrap(),
            &destination,
            &AtomicBool::new(false)
        ),
        Err(CoreError::CloneCheckout)
    ));
    assert!(!destination.join("f").exists());
    assert_eq!(
        prepare_commit(&destination).unwrap().head.as_deref(),
        Some(oid(&source, "HEAD").as_str())
    );
    assert!(
        source
            .command(&["-C", destination.to_str().unwrap(), "fsck", "--full"])
            .status
            .success()
    );
}

#[test]
/// An isolated SSH command shim proves gix honors configured command/protocol/context without any owner host connection.
fn ssh_command_fetch_and_linked_tracking_are_isolated() {
    let source = Repo::with_format("sha256");
    source.write("f", "incoming\n");
    source.initial();
    let local = Repo::with_format("sha256");
    local.write("local", "local\n");
    local.initial();
    local.write("ssh-command", "#!/bin/sh\nshift\nexec /bin/sh -c \"$*\"\n");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(
            local.path().join("ssh-command"),
            fs::Permissions::from_mode(0o755),
        )
        .unwrap();
    }
    local.git(&[
        "config",
        "core.sshCommand",
        local.path().join("ssh-command").to_str().unwrap(),
    ]);
    local.git(&["config", "ssh.variant", "simple"]);
    local.git(&[
        "remote",
        "add",
        "origin",
        &format!("ssh://fixture@fixture.invalid{}", source.path().display()),
    ]);
    let linked = local.path().join("linked");
    local.git(&["worktree", "add", "--detach", linked.to_str().unwrap()]);
    let snapshot = prepare_network(&linked).unwrap();
    assert!(snapshot.current_branch.is_none());
    let main_index = fs::read(local.path().join(".git/index")).unwrap();
    let before = prepare_commit(&linked).unwrap();
    fetch_remote(&snapshot, "origin", &AtomicBool::new(false)).unwrap();
    assert_eq!(before, prepare_commit(&linked).unwrap());
    assert_eq!(
        main_index,
        fs::read(local.path().join(".git/index")).unwrap()
    );
    assert_eq!(
        oid(&local, "refs/remotes/origin/main"),
        oid(&source, "HEAD")
    );
    local.git(&["pack-refs", "--all"]);
    assert_eq!(
        prepare_network(&linked).unwrap().branches[0].id,
        oid(&source, "HEAD")
    );
    assert!(matches!(
        set_upstream(&prepare_network(&linked).unwrap(), "origin", "main"),
        Err(CoreError::Upstream)
    ));
}

#[test]
/// Fresh clone invokes configured post-checkout once; a subprocess isolates the global fixture config from other tests.
fn clone_post_checkout_hook_uses_normal_configured_context() {
    if let Some(root) = std::env::var_os("HUNKRA_CLONE_HOOK_FIXTURE") {
        let root = std::path::PathBuf::from(root);
        clone_repository(
            root.join("source").to_str().unwrap(),
            &root.join("clone"),
            &AtomicBool::new(false),
        )
        .unwrap();
        return;
    }
    let source = Repo::new();
    source.write("f", "base\n");
    source.initial();
    let root = tempfile::tempdir().unwrap();
    source.git(&[
        "clone",
        "--bare",
        source.path().to_str().unwrap(),
        root.path().join("source").to_str().unwrap(),
    ]);
    let hooks = root.path().join("hooks");
    fs::create_dir(&hooks).unwrap();
    let marker = root.path().join("hook-called");
    fs::write(
        hooks.join("post-checkout"),
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\n",
            marker.display()
        ),
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(
            hooks.join("post-checkout"),
            fs::Permissions::from_mode(0o755),
        )
        .unwrap();
    }
    let config = root.path().join("global-fixture");
    fs::write(
        &config,
        format!("[core]\n\thooksPath = {}\n", hooks.display()),
    )
    .unwrap();
    let result = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "clone_post_checkout_hook_uses_normal_configured_context",
        ])
        .env("HUNKRA_CLONE_HOOK_FIXTURE", root.path())
        .env("GIT_CONFIG_GLOBAL", &config)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(
        fs::read_to_string(marker).unwrap(),
        format!("{}\n{}\n1\n", "0".repeat(40), oid(&source, "HEAD"))
    );
}

/// Creates one isolated bare destination with an initial branch and a matching configured source remote.
fn bare_remote(source: &Repo) -> tempfile::TempDir {
    let parent = tempfile::tempdir().unwrap();
    let bare = parent.path().join("remote.git");
    source.git(&[
        "clone",
        "--bare",
        source.path().to_str().unwrap(),
        bare.to_str().unwrap(),
    ]);
    source.git(&["remote", "add", "origin", bare.to_str().unwrap()]);
    parent
}

/// Builds one explicit single-branch request; callers choose upstream persistence rather than inheriting defaults.
fn push_request(branch: &str, set_upstream: bool) -> PushRequest {
    PushRequest {
        remote: "origin".into(),
        branch: branch.into(),
        set_upstream,
    }
}

#[test]
/// Explicit push overrides mirror/followTags/matching defaults, preserves dirty data and writes only approved upstream.
fn push_one_branch_and_upstream_preserve_dirty_project_in_both_formats() {
    for format in ["sha1", "sha256"] {
        let source = Repo::with_format(format);
        source.write("f", "base\n");
        source.initial();
        let parent = bare_remote(&source);
        let bare = parent.path().join("remote.git");
        source.git(&["branch", "other"]);
        source.git(&["tag", "-a", "local-tag", "-m", "local"]);
        source.write("f", "committed\n");
        source.git(&["commit", "-am", "next"]);
        source.write("f", "staged\n");
        source.git(&["add", "f"]);
        source.write("f", "unstaged\n");
        for (key, value) in [
            ("remote.origin.mirror", "true"),
            ("push.followTags", "true"),
            ("push.default", "matching"),
            ("push.autoSetupRemote", "true"),
        ] {
            source.git(&["config", key, value]);
        }
        let before = prepare_commit(source.path()).unwrap();
        let index = fs::read(source.path().join(".git/index")).unwrap();
        let result = push_branch(
            &prepare_network(source.path()).unwrap(),
            &push_request("review/ветка", true),
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(result.route.capability, NetworkCapability::PushCoordinator);
        assert_eq!(result.route.backend, "git-cli");
        assert!(result.upstream_written);
        assert_eq!(result.head, before.head);
        assert_eq!(
            source.git(&[
                "-C",
                bare.to_str().unwrap(),
                "rev-parse",
                "refs/heads/review/ветка"
            ]),
            source.git(&["rev-parse", "HEAD"])
        );
        assert_ne!(
            source.git(&["-C", bare.to_str().unwrap(), "rev-parse", "refs/heads/main"]),
            source.git(&["rev-parse", "HEAD"])
        );
        assert!(
            !source
                .command(&[
                    "-C",
                    bare.to_str().unwrap(),
                    "show-ref",
                    "--verify",
                    "refs/heads/other"
                ])
                .status
                .success()
        );
        assert!(
            !source
                .command(&[
                    "-C",
                    bare.to_str().unwrap(),
                    "show-ref",
                    "--verify",
                    "refs/tags/local-tag"
                ])
                .status
                .success()
        );
        assert_eq!(
            source.git(&["config", "--get", "branch.main.merge"]),
            "refs/heads/review/ветка\n".as_bytes()
        );
        assert_eq!(index, fs::read(source.path().join(".git/index")).unwrap());
        assert_eq!(fs::read(source.path().join("f")).unwrap(), b"unstaged\n");
        assert_eq!(before, prepare_commit(source.path()).unwrap());
        let again = push_branch(
            &prepare_network(source.path()).unwrap(),
            &push_request("review/ветка", false),
            &AtomicBool::new(false),
        )
        .unwrap();
        assert!(!again.upstream_written);
    }
}

#[test]
/// Remote non-fast-forward/server-hook and local pre-push rejection do not retry or write upstream.
fn push_rejections_and_hook_output_are_observed_without_diagnostic_retention() {
    let source = Repo::new();
    source.write("f", "base\n");
    source.initial();
    let parent = bare_remote(&source);
    let bare = parent.path().join("remote.git");
    source.write("f", "next\n");
    source.git(&["commit", "-am", "next"]);
    let hook = source.path().join("hooks/pre-push");
    fs::write(&hook, b"#!/bin/sh\ncat > hook-input\nprintf '%s' \"$1\" > hook-remote\nhead -c 4194304 /dev/zero\nhead -c 4194304 /dev/zero >&2\nexit 1\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();
    }
    let snapshot = prepare_network(source.path()).unwrap();
    let before = source.git(&["-C", bare.to_str().unwrap(), "rev-parse", "main"]);
    assert!(matches!(
        push_branch(
            &snapshot,
            &push_request("main", true),
            &AtomicBool::new(false)
        ),
        Err(CoreError::NetworkCoordinator {
            capability: NetworkCapability::PushCoordinator,
            ..
        })
    ));
    assert_eq!(
        before,
        source.git(&["-C", bare.to_str().unwrap(), "rev-parse", "main"])
    );
    assert_eq!(
        fs::read(source.path().join("hook-remote")).unwrap(),
        b"origin"
    );
    let hook_input = fs::read_to_string(source.path().join("hook-input")).unwrap();
    assert!(hook_input.starts_with(&format!(
        "refs/heads/main {} refs/heads/main ",
        oid(&source, "HEAD")
    )));
    assert!(
        !source
            .command(&["config", "--get", "branch.main.remote"])
            .status
            .success()
    );
    fs::remove_file(hook).unwrap();
    push_branch(
        &prepare_network(source.path()).unwrap(),
        &push_request("main", false),
        &AtomicBool::new(false),
    )
    .unwrap();
    source.git(&["reset", "--hard", "HEAD^"]);
    source.write("f", "diverged\n");
    source.git(&["commit", "-am", "diverged"]);
    let snapshot = prepare_network(source.path()).unwrap();
    let before = prepare_commit(source.path()).unwrap();
    assert!(matches!(
        push_branch(
            &snapshot,
            &push_request("main", true),
            &AtomicBool::new(false)
        ),
        Err(CoreError::NetworkCoordinator { .. })
    ));
    assert_eq!(before, prepare_commit(source.path()).unwrap());
    let receive_hook = bare.join("hooks/pre-receive");
    fs::write(&receive_hook, b"#!/bin/sh\nexit 1\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&receive_hook, fs::Permissions::from_mode(0o755)).unwrap();
    }
    assert!(matches!(
        push_branch(
            &snapshot,
            &push_request("new", true),
            &AtomicBool::new(false)
        ),
        Err(CoreError::NetworkCoordinator { .. })
    ));
    assert!(
        !source
            .command(&[
                "-C",
                bare.to_str().unwrap(),
                "show-ref",
                "--verify",
                "refs/heads/new"
            ])
            .status
            .success()
    );
    assert!(
        !source
            .command(&["config", "--get", "branch.main.remote"])
            .status
            .success()
    );
}

#[test]
/// Stale/canceled/detached/unborn, remote groups and multiple push URLs are refusals before a push is launched.
fn push_preflight_never_expands_unreviewed_targets() {
    let source = Repo::new();
    source.write("f", "base\n");
    source.initial();
    let _parent = bare_remote(&source);
    let snapshot = prepare_network(source.path()).unwrap();
    assert!(matches!(
        push_branch(
            &snapshot,
            &push_request("main", false),
            &AtomicBool::new(true)
        ),
        Err(CoreError::NetworkCanceled)
    ));
    assert!(matches!(
        push_branch(
            &snapshot,
            &push_request("-option", false),
            &AtomicBool::new(false)
        ),
        Err(CoreError::BranchName)
    ));
    source.git(&["config", "remotes.origin", "origin another"]);
    assert!(matches!(
        push_branch(
            &snapshot,
            &push_request("main", false),
            &AtomicBool::new(false)
        ),
        Err(CoreError::Stale)
    ));
    assert!(matches!(
        push_branch(
            &prepare_network(source.path()).unwrap(),
            &push_request("main", false),
            &AtomicBool::new(false)
        ),
        Err(CoreError::NetworkConfig)
    ));
    source.git(&["config", "--unset", "remotes.origin"]);
    source.git(&["config", "--add", "remote.origin.pushurl", "one"]);
    source.git(&["config", "--add", "remote.origin.pushurl", "two"]);
    assert!(matches!(
        push_branch(
            &prepare_network(source.path()).unwrap(),
            &push_request("main", false),
            &AtomicBool::new(false)
        ),
        Err(CoreError::NetworkConfig)
    ));
    source.git(&["switch", "--detach"]);
    assert!(matches!(
        push_branch(
            &prepare_network(source.path()).unwrap(),
            &push_request("main", false),
            &AtomicBool::new(false)
        ),
        Err(CoreError::Upstream)
    ));
    let unborn = Repo::new();
    assert!(matches!(
        push_branch(
            &prepare_network(unborn.path()).unwrap(),
            &push_request("main", false),
            &AtomicBool::new(false)
        ),
        Err(CoreError::Unborn)
    ));
}

#[test]
/// gix fetch + immutable ff-only integration advances both hashes and preserves unrelated dirty staged/worktree data.
fn pull_fast_forward_and_local_ahead_never_create_merge_or_stash() {
    for format in ["sha1", "sha256"] {
        let source = Repo::with_format(format);
        source.write("f", "base\n");
        source.write("local", "base\n");
        source.initial();
        let parent = bare_remote(&source);
        let bare = parent.path().join("remote.git");
        let clone = parent.path().join("clone");
        clone_repository(bare.to_str().unwrap(), &clone, &AtomicBool::new(false)).unwrap();
        fs::write(clone.join("local"), b"staged\n").unwrap();
        source.git(&["-C", clone.to_str().unwrap(), "add", "local"]);
        fs::write(clone.join("local"), b"unstaged\n").unwrap();
        source.write("f", "incoming\n");
        source.git(&["commit", "-am", "incoming"]);
        source.git(&["push", "origin", "main"]);
        source.git(&["-C", clone.to_str().unwrap(), "config", "merge.ff", "false"]);
        source.git(&[
            "-C",
            clone.to_str().unwrap(),
            "config",
            "merge.autostash",
            "true",
        ]);
        let result =
            pull_ff_only(&prepare_network(&clone).unwrap(), &AtomicBool::new(false)).unwrap();
        assert_eq!(
            result.route.capability,
            NetworkCapability::FastForwardWorktreeCoordinator
        );
        assert_eq!(result.fetch_backend, Some("gix"));
        assert_eq!(result.head, Some(oid(&source, "HEAD")));
        assert_eq!(fs::read(clone.join("f")).unwrap(), b"incoming\n");
        assert_eq!(fs::read(clone.join("local")).unwrap(), b"unstaged\n");
        assert_eq!(
            source.git(&["-C", clone.to_str().unwrap(), "show", ":local"]),
            b"staged\n"
        );
        assert!(!clone.join(".git/MERGE_HEAD").exists());
        assert!(
            !source
                .command(&[
                    "-C",
                    clone.to_str().unwrap(),
                    "show-ref",
                    "--verify",
                    "refs/stash"
                ])
                .status
                .success()
        );
        source.git(&["-C", clone.to_str().unwrap(), "commit", "-m", "local-ahead"]);
        let head = source.git(&["-C", clone.to_str().unwrap(), "rev-parse", "HEAD"]);
        pull_ff_only(&prepare_network(&clone).unwrap(), &AtomicBool::new(false)).unwrap();
        assert_eq!(
            head,
            source.git(&["-C", clone.to_str().unwrap(), "rev-parse", "HEAD"])
        );
    }
}

#[test]
/// Divergence, overlapping edits and untracked/ignored collisions refuse integration while retaining the successful fetch.
fn pull_refusals_preserve_worktree_without_merge_rebase_or_autostash() {
    for refusal in ["diverged", "dirty", "untracked", "ignored", "lock"] {
        let source = Repo::new();
        source.write("f", "base\n");
        source.write(".gitignore", "collision\n");
        source.initial();
        let parent = bare_remote(&source);
        let bare = parent.path().join("remote.git");
        let clone = parent.path().join("clone");
        clone_repository(bare.to_str().unwrap(), &clone, &AtomicBool::new(false)).unwrap();
        match refusal {
            "diverged" => {
                fs::write(clone.join("f"), b"local\n").unwrap();
                source.git(&["-C", clone.to_str().unwrap(), "commit", "-am", "local"]);
            }
            "dirty" => fs::write(clone.join("f"), b"local\n").unwrap(),
            "untracked" | "ignored" => fs::write(
                clone.join(if refusal == "ignored" {
                    "collision"
                } else {
                    "new"
                }),
                b"local\n",
            )
            .unwrap(),
            "lock" => fs::write(clone.join(".git/index.lock"), b"foreign").unwrap(),
            _ => unreachable!(),
        }
        source.write("f", "incoming\n");
        if refusal == "untracked" {
            source.write("new", "incoming\n");
        }
        if refusal == "ignored" {
            source.write("collision", "incoming\n");
            source.git(&["add", "-f", "collision"]);
        }
        source.git(&["add", "f"]);
        if refusal == "untracked" {
            source.git(&["add", "new"]);
        }
        source.git(&["commit", "-m", "incoming"]);
        source.git(&["push", "origin", "main"]);
        let before = prepare_commit(&clone).unwrap();
        let worktree = fs::read(clone.join("f")).unwrap();
        let index = fs::read(clone.join(".git/index")).unwrap();
        assert!(
            matches!(
                pull_ff_only(&prepare_network(&clone).unwrap(), &AtomicBool::new(false)),
                Err(CoreError::NetworkCoordinator {
                    capability: NetworkCapability::FastForwardWorktreeCoordinator,
                    ..
                })
            ),
            "{refusal}"
        );
        assert_eq!(prepare_commit(&clone).unwrap(), before);
        assert_eq!(fs::read(clone.join(".git/index")).unwrap(), index);
        assert_eq!(fs::read(clone.join("f")).unwrap(), worktree);
        assert_eq!(
            source.git(&[
                "-C",
                clone.to_str().unwrap(),
                "rev-parse",
                "refs/remotes/origin/main"
            ]),
            source.git(&["rev-parse", "HEAD"])
        );
        assert!(!clone.join(".git/MERGE_HEAD").exists());
        assert!(!clone.join(".git/rebase-merge").exists());
        if refusal == "untracked" || refusal == "ignored" {
            assert_eq!(
                fs::read(clone.join(if refusal == "ignored" {
                    "collision"
                } else {
                    "new"
                }))
                .unwrap(),
                b"local\n"
            );
        }
        if refusal == "lock" {
            assert_eq!(fs::read(clone.join(".git/index.lock")).unwrap(), b"foreign");
        }
    }
}

#[test]
/// Included upstream values are refused before any config/remote write; unrelated includes remain supported.
fn included_upstream_is_not_silently_duplicated_or_overridden() {
    for key in ["remote", "merge"] {
        let source = Repo::new();
        source.write("f", "base\n");
        source.initial();
        let parent = bare_remote(&source);
        source.write(
            "included",
            format!(
                "[branch \"main\"]\n\t{key} = {}\n",
                if key == "remote" {
                    "origin"
                } else {
                    "refs/heads/main"
                }
            ),
        );
        source.git(&[
            "config",
            "include.path",
            source.path().join("included").to_str().unwrap(),
        ]);
        let snapshot = prepare_network(source.path()).unwrap();
        let config = fs::read(source.path().join(".git/config")).unwrap();
        assert!(matches!(
            set_upstream(&snapshot, "origin", "other"),
            Err(CoreError::NetworkConfig)
        ));
        assert!(matches!(
            push_branch(
                &snapshot,
                &push_request("other", true),
                &AtomicBool::new(false)
            ),
            Err(CoreError::NetworkConfig)
        ));
        assert_eq!(config, fs::read(source.path().join(".git/config")).unwrap());
        assert!(
            !source
                .command(&[
                    "-C",
                    parent.path().join("remote.git").to_str().unwrap(),
                    "show-ref",
                    "--verify",
                    "refs/heads/other"
                ])
                .status
                .success()
        );
    }
}

#[test]
/// Pull/push use the linked worktree's index/HEAD and common config without moving or staging the primary worktree.
fn linked_worktree_pull_and_push_use_explicit_context() {
    let source = Repo::with_format("sha256");
    source.write("f", "base\n");
    source.initial();
    let parent = bare_remote(&source);
    let bare = parent.path().join("remote.git");
    let clone = parent.path().join("clone");
    clone_repository(bare.to_str().unwrap(), &clone, &AtomicBool::new(false)).unwrap();
    let linked = parent.path().join("linked");
    source.git(&[
        "-C",
        clone.to_str().unwrap(),
        "worktree",
        "add",
        "-b",
        "linked",
        linked.to_str().unwrap(),
    ]);
    set_upstream(&prepare_network(&linked).unwrap(), "origin", "main").unwrap();
    let primary = prepare_commit(&clone).unwrap();
    let primary_index = fs::read(clone.join(".git/index")).unwrap();
    source.write("f", "incoming\n");
    source.git(&["commit", "-am", "incoming"]);
    source.git(&["push", "origin", "main"]);
    pull_ff_only(&prepare_network(&linked).unwrap(), &AtomicBool::new(false)).unwrap();
    assert_eq!(fs::read(linked.join("f")).unwrap(), b"incoming\n");
    assert_eq!(fs::read(clone.join("f")).unwrap(), b"base\n");
    assert_eq!(primary, prepare_commit(&clone).unwrap());
    assert_eq!(primary_index, fs::read(clone.join(".git/index")).unwrap());
    let result = push_branch(
        &prepare_network(&linked).unwrap(),
        &push_request("linked-copy", true),
        &AtomicBool::new(false),
    )
    .unwrap();
    assert!(result.upstream_written);
    assert_eq!(
        source.git(&["-C", bare.to_str().unwrap(), "rev-parse", "linked-copy"]),
        source.git(&["rev-parse", "HEAD"])
    );
    assert_eq!(
        source.git(&[
            "-C",
            linked.to_str().unwrap(),
            "config",
            "--get",
            "branch.linked.merge"
        ]),
        b"refs/heads/linked-copy\n"
    );
    assert_eq!(primary, prepare_commit(&clone).unwrap());
}

#[test]
/// Missing upstream, detached/stale state and pre-cancellation are rejected before fetching remote refs.
fn pull_preflight_keeps_remote_refs_before_transport() {
    let source = Repo::new();
    source.write("f", "base\n");
    source.initial();
    let parent = bare_remote(&source);
    assert!(matches!(
        pull_ff_only(
            &prepare_network(source.path()).unwrap(),
            &AtomicBool::new(false)
        ),
        Err(CoreError::Upstream)
    ));
    let clone = parent.path().join("clone");
    clone_repository(
        parent.path().join("remote.git").to_str().unwrap(),
        &clone,
        &AtomicBool::new(false),
    )
    .unwrap();
    let tracking = source.git(&["-C", clone.to_str().unwrap(), "rev-parse", "origin/main"]);
    source.write("f", "incoming\n");
    source.git(&["commit", "-am", "incoming"]);
    source.git(&["push", "origin", "main"]);
    let snapshot = prepare_network(&clone).unwrap();
    assert!(matches!(
        pull_ff_only(&snapshot, &AtomicBool::new(true)),
        Err(CoreError::NetworkCanceled)
    ));
    source.git(&[
        "-C",
        clone.to_str().unwrap(),
        "config",
        "hunkra.changed",
        "true",
    ]);
    assert!(matches!(
        pull_ff_only(&snapshot, &AtomicBool::new(false)),
        Err(CoreError::Stale)
    ));
    source.git(&["-C", clone.to_str().unwrap(), "switch", "--detach"]);
    assert!(matches!(
        pull_ff_only(&prepare_network(&clone).unwrap(), &AtomicBool::new(false)),
        Err(CoreError::Upstream)
    ));
    assert_eq!(
        tracking,
        source.git(&["-C", clone.to_str().unwrap(), "rev-parse", "origin/main"])
    );
}

#[test]
/// Clone/fetch honor a configured SSH command while discarding unknown stderr before gix's supervisor can forward it.
fn ssh_diagnostics_never_escape_the_application_process() {
    if let Some(root) = std::env::var_os("HUNKRA_SSH_STDERR_FIXTURE") {
        let root = std::path::PathBuf::from(root);
        let clone = root.join("clone");
        let url = format!("ssh://fixture.invalid{}", root.join("remote.git").display());
        clone_repository(&url, &clone, &AtomicBool::new(false)).unwrap();
        fetch_remote(
            &prepare_network(&clone).unwrap(),
            "origin",
            &AtomicBool::new(false),
        )
        .unwrap();
        return;
    }
    let source = Repo::new();
    source.write("f", "base\n");
    source.initial();
    let parent = bare_remote(&source);
    let script = parent.path().join("ssh-shim");
    fs::write(&script, b"#!/bin/sh\nprintf 'SSH_SECRET_SENTINEL' >&2\nhead -c 4194304 /dev/zero >&2\nshift\nexec /bin/sh -c \"$*\"\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
    }
    let config = parent.path().join("config");
    fs::write(
        &config,
        format!(
            "[core]\nsshCommand = {}\n[ssh]\nvariant = simple\n",
            script.display()
        ),
    )
    .unwrap();
    let result = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "ssh_diagnostics_never_escape_the_application_process",
            "--nocapture",
        ])
        .env("HUNKRA_SSH_STDERR_FIXTURE", parent.path())
        .env("GIT_CONFIG_GLOBAL", config)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(
        !result
            .stdout
            .windows(b"SSH_SECRET_SENTINEL".len())
            .any(|bytes| bytes == b"SSH_SECRET_SENTINEL")
    );
    assert!(
        !result
            .stderr
            .windows(b"SSH_SECRET_SENTINEL".len())
            .any(|bytes| bytes == b"SSH_SECRET_SENTINEL")
    );
    assert!(result.stderr.len() < 4096);
}

#[test]
/// A real private SSH agent authenticates clone/fetch/push/pull in both hashes with strict host trust and no owner keys/config.
fn real_ssh_agent_clone_fetch_push_pull_are_isolated() {
    if let Some(root) = std::env::var_os("HUNKRA_REAL_SSH_FIXTURE") {
        let root = std::path::PathBuf::from(root);
        let source = std::path::PathBuf::from(std::env::var_os("HUNKRA_REAL_SSH_SOURCE").unwrap());
        let bare = root.join("remote.git");
        let url = format!(
            "ssh://{}@127.0.0.1:{}{}",
            std::env::var("HUNKRA_REAL_SSH_USER").unwrap(),
            std::env::var("HUNKRA_REAL_SSH_PORT").unwrap(),
            bare.display()
        );
        let clone = root.join("clone");
        let known = root.join("known_hosts");
        let trusted = fs::read(&known).unwrap();
        fs::write(
            &known,
            format!(
                "[127.0.0.1]:{} {}",
                std::env::var("HUNKRA_REAL_SSH_PORT").unwrap(),
                fs::read_to_string(root.join("client.pub")).unwrap()
            ),
        )
        .unwrap();
        assert!(matches!(
            clone_repository(&url, &root.join("untrusted"), &AtomicBool::new(false)),
            Err(CoreError::NetworkExecution)
        ));
        fs::write(&known, trusted).unwrap();
        clone_repository(&url, &clone, &AtomicBool::new(false)).unwrap();
        fetch_remote(
            &prepare_network(&clone).unwrap(),
            "origin",
            &AtomicBool::new(false),
        )
        .unwrap();
        /// Runs the fixture's independent Git oracle with explicit identity while retaining only the private SSH environment.
        fn git(root: &std::path::Path, args: &[&str]) -> Vec<u8> {
            let result = std::process::Command::new("/usr/bin/git")
                .args([
                    "-c",
                    "user.name=SSH Fixture",
                    "-c",
                    "user.email=fixture@example.invalid",
                ])
                .args(args)
                .current_dir(root)
                .output()
                .unwrap();
            assert!(
                result.status.success(),
                "{}",
                String::from_utf8_lossy(&result.stderr)
            );
            result.stdout
        }
        fs::write(clone.join("f"), b"pushed through private agent\n").unwrap();
        git(&clone, &["add", "f"]);
        git(&clone, &["commit", "-m", "pushed"]);
        push_branch(
            &prepare_network(&clone).unwrap(),
            &push_request("main", true),
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(
            git(&bare, &["rev-parse", "main"]),
            git(&clone, &["rev-parse", "HEAD"])
        );
        git(&source, &["fetch", "origin"]);
        git(&source, &["merge", "--ff-only", "origin/main"]);
        fs::write(source.join("f"), b"pulled through private agent\n").unwrap();
        git(&source, &["commit", "-am", "incoming"]);
        git(&source, &["push", "origin", "main"]);
        let result =
            pull_ff_only(&prepare_network(&clone).unwrap(), &AtomicBool::new(false)).unwrap();
        assert_eq!(result.fetch_backend, Some("gix"));
        assert_eq!(
            git(&clone, &["rev-parse", "HEAD"]),
            git(&source, &["rev-parse", "HEAD"])
        );
        assert_eq!(
            fs::read(clone.join("f")).unwrap(),
            b"pulled through private agent\n"
        );
        return;
    }
    for format in ["sha1", "sha256"] {
        let source = Repo::with_format(format);
        source.write("f", "base\n");
        source.initial();
        let parent = bare_remote(&source);
        let result = std::process::Command::new("python3")
            .arg(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/tests/support/ssh-agent.py"
            ))
            .arg(std::env::current_exe().unwrap())
            .arg(parent.path())
            .arg(source.path())
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{} {}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
    }
}
