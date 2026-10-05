mod support;
use hunkra_core::network::{
    ObjectFormat, clone_repository, fetch_remote, init_repository, prepare_network, set_upstream,
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
