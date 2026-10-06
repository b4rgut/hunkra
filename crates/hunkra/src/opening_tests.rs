//! Native host/executor acceptance for directory replies; GPUI 0.2.2's test platform has no prompt_for_paths implementation.
use super::*;
use gpui::TestAppContext;

/// Creates the real host without periodic timers or writes to even the shared isolated test settings directory.
fn host(cx: &mut Context<Hunkra>) -> Hunkra {
    let mut app = Hunkra::new(None, None, SmokeOptions::default(), cx);
    app.poll_task = None;
    app.settings_worker = LatestWorker::new("picker-settings-fixture", |_| Ok(()));
    app
}

#[gpui::test]
/// An async answer survives locales/periodic refresh; choosing the current root preserves text, selection and scan state.
fn delayed_choice_preserves_current_project(cx: &mut TestAppContext) {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path().join("current");
    let (app, cx) = cx.add_window_view(|_, cx| host(cx));
    app.update(cx, |app, cx| {
        app.open(root.clone(), cx);
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
    let recent = fixture.path().join("recent");
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
        let target = root.join("next");
        let ticket = app.begin_repository_choice().unwrap();
        app.finish_repository_choice(ticket, Ok(Some(target.clone())), cx);
        assert_eq!(app.root.as_deref(), Some(root));
        let confirmation = app.confirmation.as_ref().unwrap();
        assert!(confirmation.kind == ConfirmationKind::Dirty);
        assert!(matches!(&confirmation.intent, Intent::Open(path) if path == &target));
        assert!(app.begin_repository_choice().is_none());
        app.cancel_confirmation(cx);
        assert_eq!(result.read(cx).buffer.text(), draft);
        assert_eq!(app.root.as_deref(), Some(root));
    });
    assert_eq!(std::fs::read(root.join(".git/index")).unwrap(), index);
    assert_eq!(std::fs::read(root.join("f")).unwrap(), bytes);
}
