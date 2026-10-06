//! Native host/executor acceptance for directory replies; GPUI 0.2.2's test platform has no prompt_for_paths implementation.
use super::*;
use gpui::TestAppContext;

#[gpui::test]
/// Exercises registered preferences clicks and shortcuts while keeping Git generations, diff identity and editable state unchanged.
fn preferences_preserve_workspace_and_use_sidebar_layout(cx: &mut TestAppContext) {
    let fixture = tempfile::tempdir().unwrap();
    let root = repository(fixture.path(), "preferences");
    let repository = hunkra_core::scan(&root).unwrap();
    let diff = Arc::new(
        hunkra_core::read_diff(&root, std::path::Path::new("f"), DiffKind::Unstaged).unwrap(),
    );
    let index = std::fs::read(root.join(".git/index")).ok();
    let (app, cx) = cx.add_window_view(|window, cx| {
        let mut app = host(cx);
        app.root = Some(root.clone());
        app.view.repository = Some(repository);
        app.view.selected = Some("f".into());
        app.view.diff = Some(diff.clone());
        app.path_filter.rows = vec![0].into();
        app.view.line_selection.added.insert(0);
        app.message.update(cx, |editor, _| {
            editor.buffer.reset("Сообщение🦀\r\nEOF".into());
            editor.buffer.select(0, false);
            editor.buffer.select(12, true);
        });
        window.focus(&app.focus);
        app
    });
    cx.simulate_resize(size(px(1280.), px(820.)));
    cx.run_until_parked();
    let before = app.read_with(cx, |app, cx| {
        let message = &app.message.read(cx).buffer;
        (
            app.scan_generation,
            app.view.diff_generation,
            message.text().to_owned(),
            message.selection,
            message.revision,
        )
    });
    let sidebar = cx.debug_bounds("workspace-sidebar").unwrap();
    let commit = cx.debug_bounds("commit-panel").unwrap();
    let comparison = cx.debug_bounds("workspace-diff").unwrap();
    assert_eq!(sidebar.size.width, px(workspace_ui::SIDEBAR_WIDTH));
    assert!(sidebar.contains(&commit.origin));
    assert!(commit.right() <= sidebar.right() && commit.bottom() <= sidebar.bottom());
    assert!(comparison.origin.x >= sidebar.right());
    assert!(comparison.size.height > px(650.));
    cx.update(|window, cx| window.focus(&app.read(cx).filter_input.read(cx).focus_handle(cx)));
    cx.run_until_parked();
    let settings_button = cx.debug_bounds("settings-menu").unwrap();
    cx.simulate_click(settings_button.center(), Default::default());
    cx.run_until_parked();
    assert!(app.read_with(cx, |app, _| app.settings_open));
    app.read_with(cx, |app, cx| {
        assert_eq!(
            app.settings_focus.as_ref(),
            Some(&app.filter_input.read(cx).focus_handle(cx))
        )
    });
    assert!(cx.debug_bounds("preferences-panel").is_some());
    for (selector, language, locale) in [
        (
            "language-setting-1",
            settings::Language::RuDev,
            Locale::RuDev,
        ),
        ("language-setting-2", settings::Language::Ru, Locale::Ru),
        ("language-setting-3", settings::Language::En, Locale::En),
        (
            "language-setting-0",
            settings::Language::System,
            settings::system_locale(),
        ),
    ] {
        let bounds = cx.debug_bounds(selector).unwrap();
        cx.simulate_click(bounds.center(), Default::default());
        cx.run_until_parked();
        app.read_with(cx, |app, cx| {
            assert!(app.settings_open);
            assert_eq!(app.settings.locale, language);
            assert_eq!(app.view.locale, locale);
            assert_eq!(app.scan_generation, before.0);
            assert_eq!(app.view.diff_generation, before.1);
            assert!(Arc::ptr_eq(app.view.diff.as_ref().unwrap(), &diff));
            assert!(app.view.line_selection.added.contains(&0));
            let message = &app.message.read(cx).buffer;
            assert_eq!(
                (message.text(), message.selection, message.revision),
                (before.2.as_str(), before.3, before.4)
            );
        });
    }
    let bounds = cx.debug_bounds("diff-details-setting").unwrap();
    cx.simulate_click(bounds.center(), Default::default());
    cx.run_until_parked();
    assert!(app.read_with(cx, |app, _| app.settings.diff_details));
    cx.simulate_keystrokes("ctrl-enter");
    assert!(!app.read_with(cx, |app, _| app.writing));
    cx.simulate_keystrokes("escape");
    assert!(!app.read_with(cx, |app, _| app.settings_open));
    assert!(cx.update(|window, cx| {
        app.read(cx)
            .filter_input
            .read(cx)
            .focus_handle(cx)
            .is_focused(window)
    }));
    cx.simulate_keystrokes("ctrl-,");
    assert!(app.read_with(cx, |app, _| app.settings_open));
    cx.simulate_keystrokes("ctrl-,");
    assert!(!app.read_with(cx, |app, _| app.settings_open));
    cx.simulate_keystrokes("ctrl-,");
    app.update(cx, |app, _| app.writing = true);
    cx.run_until_parked();
    let close = cx.debug_bounds("window-close").unwrap();
    cx.simulate_click(close.center(), Default::default());
    assert!(app.read_with(cx, |app, _| app.close_requested && !app.settings_open));
    app.update(cx, |app, _| {
        app.writing = false;
        app.close_requested = false;
    });
    assert_eq!(std::fs::read(root.join("f")).unwrap(), b"worktree\r\nEOF");
    assert_eq!(std::fs::read(root.join(".git/index")).ok(), index);
}

/// Creates the real host without periodic timers or writes to even the shared isolated test settings directory.
fn host(cx: &mut Context<Hunkra>) -> Hunkra {
    let mut app = Hunkra::new(None, None, SmokeOptions::default(), cx);
    app.poll_task = None;
    app.settings_worker = LatestWorker::new("picker-settings-fixture", |_| Ok(()));
    app
}

/// Creates an actual independent unborn Git repository with a byte-sensitive working file.
fn repository(parent: &std::path::Path, name: &str) -> PathBuf {
    let root = parent.join(name);
    std::fs::create_dir(&root).unwrap();
    let result = std::process::Command::new("git")
        .args(["init", "-b", "main"])
        .current_dir(&root)
        .output()
        .unwrap();
    assert!(result.status.success());
    std::fs::write(root.join("f"), b"worktree\r\nEOF").unwrap();
    root.canonicalize().unwrap()
}
/// Delivers a real gix status bundle through the same generation/activation path used by the worker mailbox.
fn accept_open(app: &mut Hunkra, root: &std::path::Path, cx: &mut Context<Hunkra>) {
    assert!(app.receive_status(app.scan_generation, read_status(root), cx));
}

#[gpui::test]
/// A failed repository request must retain the active Git snapshot, selected diff and unfinished message.
fn failed_target_preserves_active_workspace(cx: &mut TestAppContext) {
    let fixture = tempfile::tempdir().unwrap();
    let root = repository(fixture.path(), "active");
    let repository = hunkra_core::scan(&root).unwrap();
    let diff = Arc::new(
        hunkra_core::read_diff(&root, std::path::Path::new("f"), DiffKind::Unstaged).unwrap(),
    );
    let (app, cx) = cx.add_window_view(|_, cx| host(cx));
    app.update(cx, |app, cx| {
        app.root = Some(root.clone());
        app.view.repository = Some(repository);
        app.view.selected = Some("f".into());
        app.view.diff = Some(diff.clone());
        app.message.update(cx, |editor, _| {
            editor.buffer.reset("Сохранить🦀\r\nEOF".into());
            editor.buffer.select(0, false);
            editor.buffer.select(18, true);
        });
        let before = {
            let message = &app.message.read(cx).buffer;
            (
                message.text().to_owned(),
                message.selection,
                message.revision,
            )
        };
        app.open(fixture.path().join("missing"), cx);
        assert_eq!(app.root.as_ref(), Some(&root));
        assert_eq!(
            app.view.selected.as_deref(),
            Some(std::path::Path::new("f"))
        );
        assert!(Arc::ptr_eq(app.view.diff.as_ref().unwrap(), &diff));
        let generation = app.scan_generation;
        for locale in [Locale::En, Locale::Ru, Locale::RuDev] {
            app.set_locale(locale, cx);
            app.refresh(cx);
            assert_eq!(app.scan_generation, generation);
        }
        assert!(app.receive_status(generation, read_status(&fixture.path().join("missing")), cx));
        assert!(app.pending_open.is_none() && !app.scanning && app.error_key.is_some());
        assert_eq!(app.root.as_ref(), Some(&root));
        assert!(Arc::ptr_eq(app.view.diff.as_ref().unwrap(), &diff));
        let message = &app.message.read(cx).buffer;
        assert_eq!(message.text(), before.0);
        assert_eq!(message.selection, before.1);
        assert_eq!(message.revision, before.2);
    });
}

#[gpui::test]
/// Latest target wins; locale/refresh cannot replace it, while writes/close/creation retire it and aliases retain text.
fn target_validation_obeys_newer_intents(cx: &mut TestAppContext) {
    let fixture = tempfile::tempdir().unwrap();
    let old = repository(fixture.path(), "old");
    let first = repository(fixture.path(), "first");
    let latest = repository(fixture.path(), "latest");
    let (app, cx) = cx.add_window_view(|_, cx| host(cx));
    app.update(cx, |app, cx| {
        app.open(old.clone(), cx);
        accept_open(app, &old, cx);
        app.message.update(cx, |editor, _| {
            editor.buffer.reset("pending text🦀".into());
        });
        app.open(first.clone(), cx);
        let obsolete = app.scan_generation;
        app.open(latest.clone(), cx);
        let current = app.scan_generation;
        assert!(!app.receive_status(obsolete, read_status(&first), cx));
        assert_eq!(app.root.as_ref(), Some(&old));
        assert_eq!(app.pending_open.as_ref(), Some(&latest));
        app.set_locale(Locale::RuDev, cx);
        app.refresh(cx);
        assert_eq!(app.scan_generation, current);
        app.begin_write(cx);
        assert!(app.pending_open.is_none());
        assert!(!app.receive_status(current, read_status(&latest), cx));
        app.writing = false;
        assert_eq!(app.message.read(cx).buffer.text(), "pending text🦀");
        app.open(first.clone(), cx);
        let obsolete = app.scan_generation;
        assert!(app.request_close(cx));
        assert!(!app.receive_status(obsolete, Err(CoreError::Stale), cx));
        assert!(app.error_key.is_none());
        app.open(first.clone(), cx);
        let obsolete = app.scan_generation;
        app.show_creation(NetworkPage::Init, cx);
        assert!(!app.receive_status(obsolete, read_status(&first), cx));
        assert!(app.network_tab);
        assert_eq!(app.root.as_ref(), Some(&old));
        let alias = old.join("nested");
        std::fs::create_dir(&alias).unwrap();
        app.open(alias.clone(), cx);
        accept_open(app, &alias, cx);
        assert_eq!(app.root.as_ref(), Some(&old));
        assert_eq!(app.message.read(cx).buffer.text(), "pending text🦀");
        app.open(latest.clone(), cx);
        assert_eq!(app.root.as_ref(), Some(&old));
        accept_open(app, &latest, cx);
        assert_eq!(app.root.as_ref(), Some(&latest));
        assert_eq!(app.view.repository.as_ref().unwrap().root, latest);
        assert!(app.message.read(cx).buffer.text().is_empty());
        assert!(app.pending_open.is_none() && app.reviewed_open.is_none());
    });
    for root in [&old, &first, &latest] {
        assert_eq!(std::fs::read(root.join("f")).unwrap(), b"worktree\r\nEOF");
    }
}

#[gpui::test]
/// An async answer survives locales/periodic refresh; choosing the current root preserves text, selection and scan state.
fn delayed_choice_preserves_current_project(cx: &mut TestAppContext) {
    let fixture = tempfile::tempdir().unwrap();
    let root = repository(fixture.path(), "current");
    let (app, cx) = cx.add_window_view(|_, cx| host(cx));
    app.update(cx, |app, cx| {
        app.open(root.clone(), cx);
        accept_open(app, &root, cx);
        app.message.update(cx, |editor, _| {
            assert!(editor.buffer.reset("Сообщение🦀\r\nEOF".into()));
            editor.buffer.select(0, false);
            editor.buffer.select(12, true);
        });
        let ticket = app.begin_repository_choice().unwrap();
        assert!(app.begin_repository_choice().is_none());
        let timer = cx.background_executor().timer(Duration::from_secs(1));
        let path = root.clone();
        app.await_repository_choice(
            ticket,
            async move {
                timer.await;
                Ok(Some(path))
            },
            cx,
        );
        for locale in [Locale::En, Locale::Ru, Locale::RuDev] {
            app.set_locale(locale, cx);
            app.refresh(cx);
        }
    });
    let message = app.read_with(cx, |app, _| app.message.clone());
    let before = message.read_with(cx, |editor, _| {
        (
            editor.buffer.text().to_owned(),
            editor.buffer.selection,
            editor.buffer.revision,
        )
    });
    let generation = app.read_with(cx, |app, _| app.scan_generation);
    cx.run_until_parked();
    cx.executor().advance_clock(Duration::from_secs(1));
    cx.run_until_parked();
    assert!(app.read_with(cx, |app, _| app.root == Some(root)
        && app.pending_picker.is_none()));
    assert_eq!(app.read_with(cx, |app, _| app.scan_generation), generation);
    assert_eq!(
        before,
        message.read_with(cx, |editor, _| {
            (
                editor.buffer.text().to_owned(),
                editor.buffer.selection,
                editor.buffer.revision,
            )
        })
    );
}

#[gpui::test]
/// New navigation retires a delayed response/error; cancellation is quiet and accepted failures remain localized.
fn superseded_cancelled_and_failed_choices(cx: &mut TestAppContext) {
    let fixture = tempfile::tempdir().unwrap();
    let old = fixture.path().join("old");
    let recent = repository(fixture.path(), "recent");
    let (app, cx) = cx.add_window_view(|_, cx| host(cx));
    app.update(cx, |app, cx| {
        let ticket = app.begin_repository_choice().unwrap();
        let timer = cx.background_executor().timer(Duration::from_secs(1));
        app.await_repository_choice(
            ticket,
            async move {
                timer.await;
                Ok(Some(old))
            },
            cx,
        );
        app.open_recent(recent.clone(), cx);
        accept_open(app, &recent, cx);
        assert!(
            app.begin_repository_choice().is_none(),
            "platform dialog is still outstanding"
        );
    });
    cx.run_until_parked();
    cx.executor().advance_clock(Duration::from_secs(1));
    cx.run_until_parked();
    assert!(app.read_with(cx, |app, _| app.root == Some(recent.clone())
        && app.pending_picker.is_none()));
    app.update(cx, |app, cx| {
        app.message.update(cx, |editor, _| {
            editor.buffer.reset("keep".into());
        });
        let generation = app.scan_generation;
        let cancelled = app.begin_repository_choice().unwrap();
        app.finish_repository_choice(cancelled, Ok(None), cx);
        assert_eq!(app.scan_generation, generation);
        assert_eq!(app.message.read(cx).buffer.text(), "keep");
        assert!(app.error_key.is_none());
        let superseded = app.begin_repository_choice().unwrap();
        app.open_recent(recent.clone(), cx);
        app.finish_repository_choice(superseded, Err(()), cx);
        assert!(app.error_key.is_none());
        let accepted = app.begin_repository_choice().unwrap();
        app.finish_repository_choice(superseded, Err(()), cx);
        assert_eq!(app.pending_picker, Some(accepted));
        app.finish_repository_choice(accepted, Err(()), cx);
        assert_eq!(app.error_key, Some("error-io"));
        assert_eq!(app.message.read(cx).buffer.text(), "keep");
        assert_eq!(app.scan_generation, generation);
    });
}

#[gpui::test]
/// Busy writes/close/confirmations cannot launch or be overwritten by a picker; a dirty Result uses Save/Discard/Cancel.
fn choice_obeys_write_and_dirty_result_guards(cx: &mut TestAppContext) {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    let git = |args: &[&str]| {
        let output = std::process::Command::new("git")
            .args([
                "-c",
                "user.name=Picker Fixture",
                "-c",
                "user.email=fixture@example.invalid",
            ])
            .args(args)
            .current_dir(root)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    };
    git(&["init", "-b", "main"]);
    std::fs::write(root.join("f"), b"base\n").unwrap();
    git(&["add", "f"]);
    git(&["commit", "-m", "base"]);
    git(&["switch", "-c", "incoming"]);
    std::fs::write(root.join("f"), b"incoming\n").unwrap();
    git(&["commit", "-am", "incoming"]);
    git(&["switch", "main"]);
    std::fs::write(root.join("f"), b"current\n").unwrap();
    git(&["commit", "-am", "current"]);
    let merge = std::process::Command::new("git")
        .args([
            "-c",
            "user.name=Picker Fixture",
            "-c",
            "user.email=fixture@example.invalid",
        ])
        .args(["merge", "--no-edit", "incoming"])
        .current_dir(root)
        .output()
        .unwrap();
    assert!(!merge.status.success());
    let snapshot = Arc::new(hunkra_core::read_conflict(root, std::path::Path::new("f")).unwrap());
    let index = std::fs::read(root.join(".git/index")).unwrap();
    let bytes = std::fs::read(root.join("f")).unwrap();
    let (app, cx) = cx.add_window_view(|_, cx| host(cx));
    app.update(cx, |app, cx| {
        let ticket = app.begin_repository_choice().unwrap();
        app.begin_write(cx);
        app.finish_repository_choice(ticket, Ok(Some(root.join("ignored"))), cx);
        assert!(app.root.is_none());
        assert!(app.begin_repository_choice().is_none());
        app.writing = false;
        app.close_requested = true;
        assert!(app.begin_repository_choice().is_none());
        app.close_requested = false;
        app.root = Some(root.to_path_buf());
        app.show_conflict(snapshot, cx);
        let result = app.conflict.as_ref().unwrap().read(cx).result.clone();
        result.update(cx, |editor, _| {
            editor.buffer.reset("Unsaved🦀\r\nEOF".into());
        });
        let draft = result.read(cx).buffer.text().to_owned();
        let missing = root.parent().unwrap().join("missing-repository");
        app.open(missing.clone(), cx);
        assert!(app.confirmation.is_none());
        assert!(app.receive_status(app.scan_generation, read_status(&missing), cx));
        assert!(app.confirmation.is_none());
        assert_eq!(result.read(cx).buffer.text(), draft);
        let next_fixture = tempfile::tempdir().unwrap();
        let target = repository(next_fixture.path(), "next");
        let ticket = app.begin_repository_choice().unwrap();
        app.finish_repository_choice(ticket, Ok(Some(target.clone())), cx);
        assert!(
            app.confirmation.is_none(),
            "target validation precedes the dirty prompt"
        );
        accept_open(app, &target, cx);
        assert_eq!(app.root.as_deref(), Some(root));
        let confirmation = app.confirmation.as_ref().unwrap();
        assert!(confirmation.kind == ConfirmationKind::Dirty);
        assert!(matches!(&confirmation.intent, Intent::Open(path) if path == &target));
        assert!(app.begin_repository_choice().is_none());
        app.cancel_confirmation(cx);
        assert_eq!(result.read(cx).buffer.text(), draft);
        assert_eq!(app.root.as_deref(), Some(root));
        assert!(app.reviewed_open.is_none());
        app.open(target.clone(), cx);
        accept_open(app, &target, cx);
        app.confirm_intent(cx);
        assert_eq!(app.root.as_ref(), Some(&target));
        assert!(
            app.conflict.is_none() && app.reviewed_open.is_none() && app.pending_open.is_none()
        );
    });
    assert_eq!(std::fs::read(root.join(".git/index")).unwrap(), index);
    assert_eq!(std::fs::read(root.join("f")).unwrap(), bytes);
}
