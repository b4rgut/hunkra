//! Native GPUI shell. Blocking repository reads run in separate bounded workers.
use gpui::{
    App, Application, Bounds, Context, Entity, EntityInputHandler, FocusHandle, Focusable,
    KeyDownEvent, Keystroke, PathPromptOptions, ScrollStrategy, SharedString, Subscription, Task,
    TitlebarOptions, UniformListScrollHandle, Window, WindowBounds, WindowOptions, div, prelude::*,
    px, rgb, size, uniform_list,
};
use hunkra::{
    conflict_view::{ConflictEvent, ConflictView},
    editor::{NativeEditor, TextChanged, TextRejected, TextSubmitted},
    i18n::{Locale, Translator},
    settings::{self, Settings},
    state::ViewState,
    theme::{ACCENT, BACKGROUND as BG, BORDER as LINE, HOVER, MUTED, PANEL, SELECTED, TEXT},
    worker::LatestWorker,
};
use hunkra_core::{
    Branch, BranchSnapshot, BranchWrite, ChangeKind, CommitSnapshot, CommitWrite, ConflictSnapshot,
    ConflictWrite, CoreError, DiffKind, DiffRow, DiffSnapshot, Document, IndexLimitation,
    IndexWrite, Line, LineSelection, Operation, OperationAction, OperationOutcome,
    OperationSnapshot, OperationWrite, Presentation, RepositorySnapshot, Selection,
};
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};

const SMOKE_MESSAGE: &str = "Подготовить A+B 🦀\n\nСообщение из поля";
const SMOKE_RESULT: &str = "Результат 🦀\r\nВведён вручную без последнего LF";

type DiffQuery = (PathBuf, PathBuf, DiffKind, bool);
struct DiffRead {
    diff: DiffSnapshot,
    conflict: Option<ConflictSnapshot>,
}
/// Reads a selected diff and recoverable conflict sources on the existing bounded worker.
fn read_selected((root, path, kind, active_operation): DiffQuery) -> Result<DiffRead, CoreError> {
    let diff = hunkra_core::read_diff(&root, &path, kind)?;
    let conflict = if !active_operation && diff.presentation != Presentation::Conflict {
        None
    } else {
        match hunkra_core::read_conflict(&root, &path) {
            Ok(conflict) => Some(conflict),
            Err(CoreError::NoConflict) => None,
            Err(error) => return Err(error),
        }
    };
    Ok(DiffRead { diff, conflict })
}
struct StatusRead {
    repository: RepositorySnapshot,
    branches: BranchSnapshot,
    operation: OperationSnapshot,
}
/// Reads a consistent status/branch/operation bundle through the one bounded status worker.
fn read_status(root: &std::path::Path) -> Result<StatusRead, CoreError> {
    let repository = hunkra_core::scan(root)?;
    let branches = hunkra_core::read_branches(root)?;
    let operation = hunkra_core::prepare_operation(root)?;
    if repository.commit != branches.repository || repository.commit != operation.repository {
        return Err(CoreError::Stale);
    }
    Ok(StatusRead {
        repository,
        branches,
        operation,
    })
}
enum WriteQuery {
    Index(PathBuf, std::sync::Arc<DiffSnapshot>, Selection),
    Commit(CommitSnapshot, String),
    Branch(CommitSnapshot, String),
    Operation(std::sync::Arc<OperationSnapshot>, OperationAction),
    SaveResult(std::sync::Arc<ConflictSnapshot>, Document),
    Resolve(std::sync::Arc<ConflictSnapshot>),
}
enum WriteResult {
    Index(IndexWrite),
    Commit(CommitWrite),
    Branch(BranchWrite),
    Operation(OperationWrite),
    SaveResult(ConflictWrite, Document),
    Resolve(ConflictWrite),
}

#[derive(Clone)]
enum Intent {
    Select(PathBuf),
    Open(PathBuf),
    Close,
    Reload,
    Operation(OperationAction),
    SaveDeletion(std::sync::Arc<ConflictSnapshot>, Document),
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum ConfirmationKind {
    Dirty,
    Destructive,
    Deletion,
}
struct Confirmation {
    intent: Intent,
    kind: ConfirmationKind,
}

#[derive(Default)]
struct SmokeOptions {
    stage: bool,
    close: bool,
    refusal: bool,
    commit: bool,
    hook: bool,
    operation: Option<String>,
}
struct Smoke {
    report: PathBuf,
    phase: usize,
    started: Instant,
    last_frame: usize,
    locales: Vec<String>,
    stage: bool,
    worktree: Option<Vec<u8>>,
    index_versions: Vec<String>,
    close_on_write: bool,
    refusal: bool,
    commit: bool,
    hook_refusal: bool,
    operation: Option<String>,
    message_ready: bool,
    input_prepared: bool,
    message_selection: Option<hunkra::text::Selection>,
    result_ready: bool,
    result_selection: Option<hunkra::text::Selection>,
    result_text: Option<String>,
    conflict_guard: Option<OperationSnapshot>,
    conflict_path: Option<PathBuf>,
}
struct Hunkra {
    view: ViewState,
    translator: Translator,
    settings: Settings,
    status_worker: LatestWorker<PathBuf, Result<StatusRead, CoreError>>,
    diff_worker: LatestWorker<DiffQuery, Result<DiffRead, CoreError>>,
    settings_worker: LatestWorker<Settings, std::io::Result<()>>,
    write_worker: LatestWorker<WriteQuery, Result<WriteResult, CoreError>>,
    message: Entity<NativeEditor>,
    branch_input: Entity<NativeEditor>,
    conflict: Option<Entity<ConflictView>>,
    conflict_subscription: Option<Subscription>,
    confirmation: Option<Confirmation>,
    after_save: Option<Intent>,
    applying_intent: bool,
    branches: Option<BranchSnapshot>,
    operation: Option<std::sync::Arc<OperationSnapshot>>,
    reviewed_operation: Option<std::sync::Arc<OperationSnapshot>>,
    selected_branch: Option<Branch>,
    branch_created: Option<BranchWrite>,
    operation_written: Option<OperationWrite>,
    branch_tab: bool,
    branch_scroll: UniformListScrollHandle,
    _message_subscriptions: Vec<Subscription>,
    committed: Option<CommitWrite>,
    reviewed_commit: Option<CommitSnapshot>,
    writing: bool,
    close_requested: bool,
    write_error_key: Option<&'static str>,
    last_write_backend: Option<&'static str>,
    last_operation_route: Option<hunkra_core::OperationRoute>,
    anchor_removed: Option<usize>,
    anchor_added: Option<usize>,
    root: Option<PathBuf>,
    scan_generation: u64,
    scanning: bool,
    diff_loading: bool,
    refresh_queued: bool,
    last_scan: Instant,
    mode: DiffKind,
    hunk: usize,
    error_key: Option<&'static str>,
    focus: FocusHandle,
    file_scroll: UniformListScrollHandle,
    diff_scroll: UniformListScrollHandle,
    poll_task: Option<Task<()>>,
    frames: usize,
    smoke: Option<Smoke>,
}

/// Maps domain failures to localized owned text, never displaying arbitrary config/secrets.
fn error_key(error: &CoreError) -> &'static str {
    match error {
        CoreError::Git(_) => "error-git",
        CoreError::Io(_) => "error-io",
        CoreError::Bare => "error-bare",
        CoreError::InvalidPath => "error-path",
        CoreError::TooManyFiles => "error-many-files",
        CoreError::Stale => "error-stale",
        CoreError::UnsupportedWrite => "error-write-unsupported",
        CoreError::IndexVariant(reason) => match reason {
            IndexLimitation::CaseFolding => "error-index-case",
            IndexLimitation::UnicodeComposition => "error-index-unicode",
            IndexLimitation::Sparse => "error-index-sparse",
            IndexLimitation::Split => "error-index-split",
            IndexLimitation::SkipWorktree => "error-index-skip",
            IndexLimitation::PathCollision => "error-index-collision",
            IndexLimitation::SharedPermissions => "error-index-shared",
            IndexLimitation::IndexHook => "error-index-hook",
            IndexLimitation::Symlink => "error-index-symlink",
            IndexLimitation::UnknownExtension => "error-index-extension",
        },
        CoreError::SelectionBoundary => "error-boundary",
        CoreError::EmptySelection => "error-selection",
        CoreError::IndexLock => "error-index-lock",
        CoreError::CommitMessage => "error-commit-message",
        CoreError::NothingStaged => "error-commit-empty",
        CoreError::Identity => "error-commit-identity",
        CoreError::Unresolved => "error-commit-conflict",
        CoreError::ActiveOperation => "error-active-operation",
        CoreError::Hook(name) => match *name {
            "pre-commit" => "error-hook-pre-commit",
            "prepare-commit-msg" => "error-hook-prepare",
            "commit-msg" => "error-hook-message",
            "reference-transaction" => "error-hook-reference",
            _ => "error-commit-hook",
        },
        CoreError::Signing => "error-commit-signing",
        CoreError::ReferenceUpdate => "error-commit-reference",
        CoreError::CommitUnsupported(reason) => match *reason {
            "encoding" => "error-commit-encoding",
            "sparse" => "error-commit-sparse",
            "cleanup" | "comment-auto" => "error-commit-cleanup",
            "symbolic-head" => "error-reference-format",
            _ => "error-commit-unsupported",
        },
        CoreError::BranchName => "error-branch-name",
        CoreError::BranchExists => "error-branch-exists",
        CoreError::Unborn => "error-branch-unborn",
        CoreError::ReferenceLimit => "error-reference-limit",
        CoreError::OperationMetadata => "error-operation-metadata",
        CoreError::BranchMissing => "error-branch-missing",
        CoreError::NoOperation => "error-operation-none",
        CoreError::OperationAction => "error-operation-action",
        CoreError::OperationExecution { .. } => "error-operation-execution",
        CoreError::NoConflict => "error-conflict-none",
        CoreError::ConflictUnsupported => "error-conflict-unsupported",
        CoreError::ConflictChunk => "error-conflict-chunk",
        CoreError::ConflictBoundary => "error-conflict-boundary",
        CoreError::WorktreeLock => "error-worktree-lock",
    }
}

/// Builds consistent native buttons; callers supply their own action listener.
fn button(id: impl Into<gpui::ElementId>, text: String, active: bool) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .px_2()
        .py_1()
        .rounded_sm()
        .text_sm()
        .cursor_pointer()
        .bg(rgb(if active { SELECTED } else { PANEL }))
        .text_color(rgb(if active { ACCENT } else { TEXT }))
        .hover(|style| style.bg(rgb(HOVER)))
        .child(text)
}

impl Hunkra {
    /// Creates the standalone host and starts only the requested repository read.
    fn new(
        root: Option<PathBuf>,
        smoke: Option<PathBuf>,
        smoke_options: SmokeOptions,
        cx: &mut Context<Self>,
    ) -> Self {
        let settings = settings::load();
        let locale = settings.locale;
        let translator = Translator::new(locale);
        let message = cx.new(|cx| {
            NativeEditor::new(64 * 1024, translator.text("commit-placeholder", None), cx)
        });
        let message_subscription = cx.subscribe(&message, |_, _, _: &TextChanged, cx| cx.notify());
        let rejected_subscription = cx.subscribe(&message, |app, _, _: &TextRejected, cx| {
            app.write_error_key = Some("error-commit-message");
            cx.notify();
        });
        let branch_input = cx.new(|cx| {
            NativeEditor::single_line(4096, translator.text("branch-placeholder", None), cx)
        });
        let branch_changed = cx.subscribe(&branch_input, |_, _, _: &TextChanged, cx| cx.notify());
        let branch_rejected = cx.subscribe(&branch_input, |app, _, _: &TextRejected, cx| {
            app.write_error_key = Some("error-branch-name");
            cx.notify();
        });
        let branch_submitted = cx.subscribe(&branch_input, |app, _, _: &TextSubmitted, cx| {
            app.create_branch(cx);
        });
        let mut app = Self {
            view: ViewState {
                locale,
                ..Default::default()
            },
            translator,
            message,
            branch_input,
            conflict: None,
            conflict_subscription: None,
            confirmation: None,
            after_save: None,
            applying_intent: false,
            branches: None,
            operation: None,
            reviewed_operation: None,
            selected_branch: None,
            branch_created: None,
            operation_written: None,
            branch_tab: false,
            branch_scroll: UniformListScrollHandle::new(),
            _message_subscriptions: vec![
                message_subscription,
                rejected_subscription,
                branch_changed,
                branch_rejected,
                branch_submitted,
            ],
            committed: None,
            reviewed_commit: None,
            settings,
            status_worker: LatestWorker::new("hunkra-status", |root: PathBuf| read_status(&root)),
            diff_worker: LatestWorker::new("hunkra-diff", read_selected),
            settings_worker: LatestWorker::new("hunkra-settings", settings::save),
            write_worker: LatestWorker::new("hunkra-write", |query: WriteQuery| match query {
                WriteQuery::Index(root, diff, selection) => {
                    hunkra_core::update_index(&root, &diff, &selection).map(WriteResult::Index)
                }
                WriteQuery::Commit(snapshot, message) => {
                    hunkra_core::create_commit(&snapshot, &message).map(WriteResult::Commit)
                }
                WriteQuery::Branch(snapshot, name) => {
                    hunkra_core::create_branch(&snapshot, &name).map(WriteResult::Branch)
                }
                WriteQuery::Operation(snapshot, action) => {
                    hunkra_core::run_operation(&snapshot, &action).map(WriteResult::Operation)
                }
                WriteQuery::SaveResult(snapshot, document) => {
                    hunkra_core::save_result(&snapshot, &document)
                        .map(|write| WriteResult::SaveResult(write, document))
                }
                WriteQuery::Resolve(snapshot) => {
                    hunkra_core::mark_resolved(&snapshot).map(WriteResult::Resolve)
                }
            }),
            writing: false,
            close_requested: false,
            write_error_key: None,
            last_write_backend: None,
            last_operation_route: None,
            anchor_removed: None,
            anchor_added: None,
            root: None,
            scan_generation: 0,
            scanning: false,
            diff_loading: false,
            refresh_queued: false,
            last_scan: Instant::now(),
            mode: DiffKind::Unstaged,
            hunk: 0,
            error_key: None,
            focus: cx.focus_handle(),
            file_scroll: UniformListScrollHandle::new(),
            diff_scroll: UniformListScrollHandle::new(),
            poll_task: None,
            frames: 0,
            smoke: smoke.map(|report| Smoke {
                report,
                phase: 0,
                started: Instant::now(),
                last_frame: 0,
                locales: Vec::new(),
                stage: smoke_options.stage,
                worktree: None,
                index_versions: Vec::new(),
                close_on_write: smoke_options.close,
                refusal: smoke_options.refusal,
                commit: smoke_options.commit,
                hook_refusal: smoke_options.hook,
                operation: smoke_options.operation,
                message_ready: false,
                input_prepared: false,
                message_selection: None,
                result_ready: false,
                result_selection: None,
                result_text: None,
                conflict_guard: None,
                conflict_path: None,
            }),
        };
        if let Some(root) = root {
            app.open(root, cx);
        }
        let timer = cx.background_executor().clone();
        app.poll_task = Some(cx.spawn(async move |entity, cx| {
            loop {
                timer.timer(Duration::from_millis(100)).await;
                if entity.update(cx, |app, cx| app.poll(cx)).is_err() {
                    break;
                }
            }
        }));
        app
    }
    /// Formats one bundled message using the active label profile.
    fn t(&self, key: &str) -> String {
        self.translator.text(key, None)
    }
    /// Defers navigation or destructive Git actions when they would discard an editable Result.
    fn guard_intent(&mut self, intent: Intent, cx: &mut Context<Self>) -> bool {
        if self.applying_intent {
            return false;
        }
        if self.confirmation.is_some() {
            return true;
        }
        if self
            .conflict
            .as_ref()
            .is_some_and(|view| view.read(cx).dirty(cx))
        {
            self.show_confirmation(intent, ConfirmationKind::Dirty, cx);
            return true;
        }
        if matches!(
            intent,
            Intent::Operation(OperationAction::Abort | OperationAction::Skip)
        ) {
            self.show_confirmation(intent, ConfirmationKind::Destructive, cx);
            return true;
        }
        false
    }
    /// Keeps the concrete pending action visible and freezes Result while the product asks for a choice.
    fn show_confirmation(
        &mut self,
        intent: Intent,
        kind: ConfirmationKind,
        cx: &mut Context<Self>,
    ) {
        self.confirmation = Some(Confirmation { intent, kind });
        if let Some(view) = &self.conflict {
            view.update(cx, |view, cx| view.set_busy(true, cx));
        }
        cx.notify();
    }
    /// Cancels the product prompt without losing Result, source selection or undo history.
    fn cancel_confirmation(&mut self, cx: &mut Context<Self>) {
        self.confirmation = None;
        self.after_save = None;
        if let Some(view) = &self.conflict {
            view.update(cx, |view, cx| view.set_busy(self.writing, cx));
        }
        cx.notify();
    }
    /// Saves before file navigation/close; Git continuation always requires its separate explicit action.
    fn save_before_intent(&mut self, cx: &mut Context<Self>) {
        let Some(pending) = &self.confirmation else {
            return;
        };
        if pending.kind != ConfirmationKind::Dirty {
            return;
        }
        let Some(view) = self.conflict.clone() else {
            return;
        };
        if view.read(cx).draft.stale || view.read(cx).draft.awaiting_refresh {
            return;
        }
        let pending = self.confirmation.take().expect("checked prompt");
        self.after_save = if matches!(pending.intent, Intent::Operation(_)) {
            None
        } else {
            Some(pending.intent)
        };
        view.update(cx, |view, cx| {
            view.set_busy(false, cx);
            view.save(cx);
        });
        cx.notify();
    }
    /// Applies explicit discard separately from the destructive operation's own concrete confirmation.
    fn confirm_intent(&mut self, cx: &mut Context<Self>) {
        let Some(pending) = self.confirmation.take() else {
            return;
        };
        if pending.kind == ConfirmationKind::Dirty {
            self.conflict = None;
            self.conflict_subscription = None;
            if matches!(
                pending.intent,
                Intent::Operation(OperationAction::Abort | OperationAction::Skip)
            ) {
                self.show_confirmation(pending.intent, ConfirmationKind::Destructive, cx);
                return;
            }
        }
        if let Some(view) = &self.conflict {
            view.update(cx, |view, cx| view.set_busy(false, cx));
        }
        self.apply_intent(pending.intent, cx);
    }
    /// Executes only a user-approved pending intent; core snapshots still guard every write.
    fn apply_intent(&mut self, intent: Intent, cx: &mut Context<Self>) {
        self.applying_intent = true;
        match intent {
            Intent::Select(path) => self.select(path, cx),
            Intent::Open(path) => self.open(path, cx),
            Intent::Close => cx.quit(),
            Intent::Reload => {
                self.conflict = None;
                self.conflict_subscription = None;
                if let Some(path) = self.view.selected.clone() {
                    self.select(path, cx);
                }
            }
            Intent::Operation(action) => self.perform_operation(action, cx),
            Intent::SaveDeletion(snapshot, document) => {
                self.begin_write(cx);
                self.write_worker
                    .submit(0, WriteQuery::SaveResult(snapshot, document));
            }
        }
        self.applying_intent = false;
        cx.notify();
    }
    /// Renders a product confirmation for a named action, without any implicit stash/reset/discard.
    fn confirmation_panel(&self, cx: &Context<Self>) -> Option<gpui::Div> {
        let pending = self.confirmation.as_ref()?;
        let body = match (&pending.intent, pending.kind) {
            (_, ConfirmationKind::Dirty) => "confirm-dirty-result",
            (Intent::Operation(OperationAction::Skip), _) => "confirm-skip",
            (Intent::Operation(OperationAction::Abort), _) => "confirm-abort",
            _ => "confirm-delete-result",
        };
        let mut panel = div()
            .p_3()
            .flex_none()
            .bg(rgb(0x3c3430))
            .flex()
            .items_center()
            .gap_3()
            .child(div().flex_1().min_w_0().child(self.t(body)));
        if pending.kind == ConfirmationKind::Dirty {
            let save_ready = self.conflict.as_ref().is_some_and(|view| {
                !view.read(cx).draft.stale && !view.read(cx).draft.awaiting_refresh
            });
            panel = panel.child(
                button(
                    "confirm-save",
                    self.t(if matches!(pending.intent, Intent::Operation(_)) {
                        "save-result"
                    } else {
                        "save-and-leave"
                    }),
                    false,
                )
                .opacity(if save_ready { 1. } else { 0.5 })
                .on_click(cx.listener(|app, _, _, cx| app.save_before_intent(cx))),
            );
        }
        Some(
            panel
                .child(
                    button(
                        "confirm-action",
                        self.t(if pending.kind == ConfirmationKind::Dirty {
                            "discard-and-leave"
                        } else {
                            "confirm-action"
                        }),
                        false,
                    )
                    .on_click(cx.listener(|app, _, _, cx| app.confirm_intent(cx))),
                )
                .child(
                    button("confirm-cancel", self.t("cancel-action"), true)
                        .on_click(cx.listener(|app, _, _, cx| app.cancel_confirmation(cx))),
                ),
        )
    }
    /// Retains the native Result entity across same-file refresh; new files acquire their own editor.
    fn show_conflict(
        &mut self,
        snapshot: std::sync::Arc<ConflictSnapshot>,
        cx: &mut Context<Self>,
    ) {
        if let Some(view) = &self.conflict
            && view.read(cx).snapshot.path == snapshot.path
        {
            view.update(cx, |view, cx| view.refresh(snapshot, cx));
            return;
        }
        let view = cx.new(|cx| ConflictView::new(snapshot, self.view.locale, cx));
        view.update(cx, |view, cx| {
            view.set_busy(self.writing || self.confirmation.is_some(), cx)
        });
        self.conflict_subscription =
            Some(cx.subscribe(&view, |app, _, event: &ConflictEvent, cx| {
                match event {
                    ConflictEvent::Save(snapshot, document) if !app.writing => {
                        if document.present {
                            app.begin_write(cx);
                            app.write_worker.submit(
                                0,
                                WriteQuery::SaveResult(snapshot.clone(), document.clone()),
                            );
                            if app.smoke.as_ref().is_some_and(|smoke| {
                                smoke.close_on_write
                                    && smoke
                                        .operation
                                        .as_deref()
                                        .is_some_and(|action| action.starts_with("conflict"))
                            }) {
                                assert!(!app.request_close(cx));
                            }
                        } else {
                            app.show_confirmation(
                                Intent::SaveDeletion(snapshot.clone(), document.clone()),
                                ConfirmationKind::Deletion,
                                cx,
                            );
                        }
                    }
                    ConflictEvent::Resolve(snapshot) if !app.writing => {
                        app.begin_write(cx);
                        app.write_worker
                            .submit(0, WriteQuery::Resolve(snapshot.clone()));
                    }
                    ConflictEvent::Reload if !app.writing => {
                        if !app.guard_intent(Intent::Reload, cx) {
                            app.apply_intent(Intent::Reload, cx);
                        }
                    }
                    ConflictEvent::Error(error) => app.write_error_key = Some(error_key(error)),
                    _ => {}
                }
                cx.notify();
            }));
        self.conflict = Some(view);
    }
    /// Defers ordinary window closure until an accepted index transaction has finished.
    fn request_close(&mut self, cx: &mut Context<Self>) -> bool {
        if self.writing {
            self.close_requested = true;
            cx.notify();
            false
        } else {
            !self.guard_intent(Intent::Close, cx)
        }
    }
    /// Switches a profile without issuing repository requests or replacing diff state.
    fn set_locale(&mut self, locale: Locale, cx: &mut Context<Self>) {
        self.view.switch_locale(locale);
        self.translator = Translator::new(locale);
        if let Some(conflict) = &self.conflict {
            conflict.update(cx, |view, cx| view.set_locale(locale, cx));
        }
        let placeholder = self.t("commit-placeholder");
        self.message.update(cx, |editor, cx| {
            editor.placeholder = placeholder.into();
            cx.notify();
        });
        let placeholder = self.t("branch-placeholder");
        self.branch_input.update(cx, |editor, cx| {
            editor.placeholder = placeholder.into();
            cx.notify();
        });
        self.settings.locale = locale;
        if self.smoke.is_none() {
            self.settings_worker.submit(0, self.settings.clone());
        }
        cx.notify();
    }
    /// Opens a path by generation, invalidating pending replies for the previous repository.
    fn open(&mut self, root: PathBuf, cx: &mut Context<Self>) {
        if self.writing {
            return;
        }
        if self.guard_intent(Intent::Open(root.clone()), cx) {
            return;
        }
        self.committed = None;
        self.conflict = None;
        self.conflict_subscription = None;
        self.reviewed_commit = None;
        self.branches = None;
        self.operation = None;
        self.reviewed_operation = None;
        self.selected_branch = None;
        self.branch_created = None;
        self.operation_written = None;
        self.branch_input.update(cx, |editor, cx| {
            editor.buffer.reset(String::new());
            cx.notify();
        });
        self.message.update(cx, |editor, cx| {
            editor.buffer.reset(String::new());
            cx.notify();
        });
        self.root = Some(root);
        self.view.repository = None;
        self.view.selected = None;
        self.view.diff = None;
        self.view.selected_rows.clear();
        self.view.line_selection = Default::default();
        self.anchor_removed = None;
        self.anchor_added = None;
        self.write_error_key = None;
        self.last_write_backend = None;
        self.last_operation_route = None;
        self.view.diff_generation += 1;
        self.diff_loading = false;
        self.error_key = None;
        self.refresh_queued = false;
        self.refresh(cx);
    }
    /// Coalesces manual refresh requests while a status scan is active.
    fn refresh(&mut self, cx: &mut Context<Self>) {
        if self.writing {
            self.refresh_queued = true;
            return;
        }
        let Some(root) = self.root.clone() else {
            return;
        };
        self.scan_generation += 1;
        self.scanning = true;
        self.last_scan = Instant::now();
        self.status_worker.submit(self.scan_generation, root);
        cx.notify();
    }
    /// Uses GPUI's asynchronous native directory picker.
    fn choose_repository(&mut self, cx: &mut Context<Self>) {
        if self.writing {
            return;
        }
        let receiver = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some(self.t("open").into()),
        });
        cx.spawn(async move |entity, cx| match receiver.await {
            Ok(Ok(Some(paths))) => {
                if let Some(path) = paths.into_iter().next() {
                    let _ = entity.update(cx, |app, cx| app.open(path, cx));
                }
            }
            Ok(Ok(None)) => {}
            _ => {
                let _ = entity.update(cx, |app, cx| {
                    app.error_key = Some("error-io");
                    cx.notify();
                });
            }
        })
        .detach();
    }
    /// Requests one diff and retains the old rendering while the same document refreshes.
    fn select(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        if self.writing {
            return;
        }
        if self.view.selected.as_ref() != Some(&path)
            && self.guard_intent(Intent::Select(path.clone()), cx)
        {
            return;
        }
        let Some(root) = self.root.clone() else {
            return;
        };
        let changed_document = self.view.selected.as_ref() != Some(&path)
            || self
                .view
                .diff
                .as_ref()
                .is_some_and(|diff| diff.kind != self.mode);
        let generation = self.view.select(path.clone(), self.mode);
        self.diff_loading = true;
        if changed_document {
            self.hunk = 0;
            self.anchor_removed = None;
            self.anchor_added = None;
            self.diff_scroll.scroll_to_item(0, ScrollStrategy::Top);
        }
        let active_operation = self
            .operation
            .as_ref()
            .is_some_and(|snapshot| snapshot.state.is_some());
        self.diff_worker
            .submit(generation, (root, path, self.mode, active_operation));
        cx.notify();
    }
    /// Changes the comparison explicitly; HEAD→worktree is never used for either tab.
    fn set_mode(&mut self, mode: DiffKind, cx: &mut Context<Self>) {
        if self.writing {
            return;
        }
        if self.mode == mode {
            return;
        }
        self.mode = mode;
        self.anchor_removed = None;
        self.anchor_added = None;
        if let Some(path) = self.view.selected.clone() {
            self.view.diff = None;
            self.view.selected_rows.clear();
            self.view.line_selection = Default::default();
            self.select(path, cx);
        }
        cx.notify();
    }
    /// Selects added/deleted halves independently; shift-click spans only changed lines.
    fn toggle_line(&mut self, left: bool, number: usize, range: bool, cx: &mut Context<Self>) {
        if self.writing {
            return;
        }
        let Some(diff) = &self.view.diff else {
            return;
        };
        let selected = if left {
            &mut self.view.line_selection.removed
        } else {
            &mut self.view.line_selection.added
        };
        let anchor = if left {
            &mut self.anchor_removed
        } else {
            &mut self.anchor_added
        };
        if range && let Some(anchor) = *anchor {
            let bounds = anchor.min(number)..=anchor.max(number);
            for row in diff.rows.iter().filter(|row| row.changed) {
                let line = if left { &row.left } else { &row.right };
                if let Some(line) = line
                    && bounds.contains(&(line.number - 1))
                {
                    selected.insert(line.number - 1);
                }
            }
        } else if !selected.insert(number) {
            selected.remove(&number);
        }
        *anchor = Some(number);
        cx.notify();
    }
    /// Submits the currently selected changed lines with their immutable diff precondition.
    fn write_selected(&mut self, cx: &mut Context<Self>) {
        self.write_index(Selection::Lines(self.view.line_selection.clone()), cx);
    }
    /// Selects every +/- line in the current hunk, keeping other hunks untouched.
    fn write_hunk(&mut self, cx: &mut Context<Self>) {
        let Some(diff) = &self.view.diff else {
            return;
        };
        let Some(range) = diff.hunks.get(self.hunk) else {
            return;
        };
        let mut lines = LineSelection::default();
        for row in &diff.rows[range.clone()] {
            if let Some(line) = &row.left {
                lines.removed.insert(line.number - 1);
            }
            if let Some(line) = &row.right {
                lines.added.insert(line.number - 1);
            }
        }
        self.write_index(Selection::Lines(lines), cx);
    }
    /// Serializes index writes; active reads are invalidated and fresh status follows completion.
    fn write_index(&mut self, selection: Selection, cx: &mut Context<Self>) {
        if self.writing || self.confirmation.is_some() {
            return;
        }
        let (Some(root), Some(diff)) = (self.root.clone(), self.view.diff.clone()) else {
            return;
        };
        self.begin_write(cx);
        self.write_worker
            .submit(0, WriteQuery::Index(root, diff, selection));
        cx.notify();
    }
    /// Captures the reviewed repository and user text before entering the shared writer.
    fn commit(&mut self, cx: &mut Context<Self>) {
        if self.writing || self.scanning || self.diff_loading || self.confirmation.is_some() {
            return;
        }
        let Some(snapshot) = self.reviewed_commit.clone() else {
            return;
        };
        let message = self.message.read(cx).buffer.text().to_owned();
        if message.trim().is_empty() {
            self.write_error_key = Some("error-commit-message");
            cx.notify();
            return;
        }
        self.begin_write(cx);
        self.write_worker
            .submit(0, WriteQuery::Commit(snapshot, message));
        cx.notify();
    }
    /// Creates a branch from the last displayed HEAD, preserving input after any refusal.
    fn create_branch(&mut self, cx: &mut Context<Self>) {
        if self.writing
            || self.scanning
            || self.confirmation.is_some()
            || self
                .operation
                .as_ref()
                .is_some_and(|snapshot| snapshot.state.is_some())
        {
            return;
        }
        let Some(snapshot) = self.reviewed_commit.clone() else {
            return;
        };
        let name = self.branch_input.read(cx).buffer.text().to_owned();
        self.begin_write(cx);
        self.write_worker
            .submit(0, WriteQuery::Branch(snapshot, name));
        cx.notify();
    }
    /// Uses the last displayed operation snapshot and immutable selected target, never an unseen fresh read.
    fn perform_operation(&mut self, action: OperationAction, cx: &mut Context<Self>) {
        if self.writing || self.scanning {
            return;
        }
        if self.guard_intent(Intent::Operation(action.clone()), cx) {
            return;
        }
        let Some(snapshot) = self.reviewed_operation.clone() else {
            return;
        };
        if matches!(
            action,
            OperationAction::Switch(_) | OperationAction::Merge(_) | OperationAction::Rebase(_)
        ) && snapshot.state.is_some()
        {
            return;
        }
        if action == OperationAction::Continue
            && self.conflict.as_ref().is_some_and(|view| {
                let view = view.read(cx);
                view.draft.awaiting_refresh || !view.snapshot.result_staged
            })
        {
            self.write_error_key = Some("error-result-index");
            cx.notify();
            return;
        }
        if action == OperationAction::Continue
            && self
                .view
                .repository
                .as_ref()
                .is_some_and(|repo| repo.files.iter().any(|file| file.conflicted))
        {
            return;
        }
        let route = hunkra_core::operation_route(&snapshot, &action).ok();
        self.begin_write(cx);
        self.last_operation_route = route;
        self.write_worker
            .submit(0, WriteQuery::Operation(snapshot, action));
        cx.notify();
    }
    /// Invalidates pending reads and freezes message edits for either kind of accepted write.
    fn begin_write(&mut self, cx: &mut Context<Self>) {
        self.writing = true;
        self.committed = None;
        self.branch_created = None;
        self.operation_written = None;
        if let Some(conflict) = &self.conflict {
            conflict.update(cx, |view, cx| view.set_busy(true, cx));
        }
        self.branch_input.update(cx, |editor, cx| {
            editor.enabled = false;
            cx.notify();
        });
        self.message.update(cx, |editor, cx| {
            editor.enabled = false;
            cx.notify();
        });
        self.error_key = None;
        self.write_error_key = None;
        self.last_write_backend = None;
        self.last_operation_route = None;
        self.scan_generation += 1;
        self.scanning = false;
        self.view.diff_generation += 1;
        self.diff_loading = false;
    }
    /// Reads completed mailboxes, rejecting obsolete generations before touching UI state.
    fn poll(&mut self, cx: &mut Context<Self>) {
        let mut changed = false;
        if let Some((generation, result)) = self.status_worker.take()
            && generation == self.scan_generation
        {
            self.scanning = false;
            changed = true;
            match result {
                Ok(read) => {
                    let snapshot = read.repository;
                    self.selected_branch = self.selected_branch.take().and_then(|selected| {
                        read.branches
                            .branches
                            .iter()
                            .find(|branch| branch.name == selected.name)
                            .cloned()
                    });
                    self.branches = Some(read.branches);
                    self.operation = Some(std::sync::Arc::new(read.operation));
                    self.error_key = None;
                    self.root = Some(snapshot.root.clone());
                    let retain_conflict = self.confirmation.is_some()
                        || self.conflict.as_ref().is_some_and(|conflict| {
                            let view = conflict.read(cx);
                            view.dirty(cx)
                                || self.operation.as_ref().is_some_and(|operation| {
                                    operation.state.is_some()
                                        && view.snapshot.operation.state == operation.state
                                })
                        });
                    let selected = self
                        .view
                        .selected
                        .clone()
                        .filter(|path| {
                            retain_conflict || snapshot.files.iter().any(|file| &file.path == path)
                        })
                        .or_else(|| {
                            snapshot
                                .files
                                .iter()
                                .find(|file| file.staged.is_some() && file.unstaged.is_some())
                                .or_else(|| snapshot.files.first())
                                .map(|file| file.path.clone())
                        });
                    if self.settings.recent.first() != Some(&snapshot.root) && self.smoke.is_none()
                    {
                        self.settings.recent.retain(|path| path != &snapshot.root);
                        self.settings.recent.insert(0, snapshot.root.clone());
                        self.settings.recent.truncate(10);
                        self.settings_worker.submit(0, self.settings.clone());
                    }
                    self.view.repository = Some(snapshot);
                    if let Some(path) = selected {
                        if self.view.selected.as_ref() != Some(&path) || !self.diff_loading {
                            self.select(path, cx);
                        }
                    } else {
                        self.view.selected = None;
                        self.view.diff = None;
                        self.view.selected_rows.clear();
                        self.view.line_selection = Default::default();
                    }
                }
                Err(error) => self.error_key = Some(error_key(&error)),
            }
        }
        if let Some((generation, result)) = self.diff_worker.take()
            && generation == self.view.diff_generation
        {
            self.diff_loading = false;
            changed = true;
            match result {
                Ok(read) => {
                    let diff = read.diff;
                    match read.conflict {
                        Some(snapshot) => self.show_conflict(std::sync::Arc::new(snapshot), cx),
                        None if self
                            .conflict
                            .as_ref()
                            .is_some_and(|view| view.read(cx).dirty(cx)) =>
                        {
                            if let Some(view) = &self.conflict {
                                view.update(cx, |view, cx| {
                                    view.draft.stale = true;
                                    cx.notify();
                                });
                            }
                        }
                        None => {
                            self.conflict = None;
                            self.conflict_subscription = None;
                        }
                    }
                    if self.view.diff.as_ref().is_some_and(|previous| {
                        previous.left != diff.left || previous.right != diff.right
                    }) {
                        self.anchor_removed = None;
                        self.anchor_added = None;
                    }
                    self.view.apply_diff(generation, diff);
                    if let Some(diff) = &self.view.diff {
                        self.hunk = self.hunk.min(diff.hunks.len().saturating_sub(1));
                    }
                }
                Err(error) => {
                    self.view.diff = None;
                    self.error_key = Some(error_key(&error));
                }
            }
        }
        if let Some((_, Err(_))) = self.settings_worker.take() {
            self.error_key = Some("error-settings");
            changed = true;
        }
        if let Some((_, result)) = self.write_worker.take() {
            self.writing = false;
            if let Some(conflict) = &self.conflict {
                conflict.update(cx, |view, cx| view.set_busy(false, cx));
            }
            self.branch_input.update(cx, |editor, cx| {
                editor.enabled = true;
                cx.notify();
            });
            self.message.update(cx, |editor, cx| {
                editor.enabled = true;
                cx.notify();
            });
            if self.view.finish_write(&result) {
                self.anchor_removed = None;
                self.anchor_added = None;
            }
            self.diff_loading = false;
            let mut saved_result = false;
            match result {
                Ok(WriteResult::Index(result)) => {
                    self.write_error_key = None;
                    self.last_write_backend = Some(result.backend);
                }
                Ok(WriteResult::Commit(result)) => {
                    self.write_error_key = None;
                    self.last_write_backend = Some(result.backend);
                    self.message.update(cx, |editor, cx| {
                        editor.buffer.reset(String::new());
                        cx.notify();
                    });
                    self.committed = Some(result);
                }
                Ok(WriteResult::Branch(result)) => {
                    self.write_error_key = None;
                    self.last_write_backend = Some(result.backend);
                    self.selected_branch = Some(result.branch.clone());
                    self.branch_input.update(cx, |editor, cx| {
                        editor.buffer.reset(String::new());
                        cx.notify();
                    });
                    self.branch_created = Some(result);
                }
                Ok(WriteResult::Operation(result)) => {
                    self.write_error_key = None;
                    self.last_write_backend = Some(result.route.backend);
                    self.operation_written = Some(result);
                }
                Ok(WriteResult::SaveResult(result, document)) => {
                    saved_result = true;
                    self.write_error_key = None;
                    self.last_write_backend = Some(result.backend);
                    if let Some(conflict) = &self.conflict {
                        conflict.update(cx, |view, cx| view.saved(document, cx));
                    }
                }
                Ok(WriteResult::Resolve(result)) => {
                    self.write_error_key = None;
                    self.last_write_backend = Some(result.backend);
                }
                Err(error) => {
                    self.after_save = None;
                    self.write_error_key = Some(error_key(&error));
                }
            }
            if saved_result && let Some(intent) = self.after_save.take() {
                self.apply_intent(intent, cx);
            }
            if self.close_requested && self.last_write_backend.is_some() {
                if let Some(smoke) = &self.smoke {
                    let report = serde_json::json!({"ok":self.last_write_backend==Some(if smoke.operation.as_deref().is_some_and(|action| action.starts_with("conflict")) {"filesystem"} else if smoke.operation.as_deref().is_some_and(|action| !action.starts_with("branch")) {"git-cli"} else {"gix"}), "close_deferred":true, "writer_finished":true, "write_backend":self.last_write_backend, "commit_id":self.committed.as_ref().map(|commit| &commit.id), "message_cleared":self.message.read(cx).buffer.text().is_empty()});
                    std::fs::write(&smoke.report, serde_json::to_vec_pretty(&report).unwrap())
                        .expect("close smoke report");
                }
                cx.quit();
                return;
            }
            self.close_requested = false;
            self.refresh_queued = false;
            self.refresh(cx);
            changed = true;
        }
        if self.smoke.is_some() {
            self.smoke_tick(cx);
        } else if !self.writing && !self.scanning && self.refresh_queued {
            self.refresh_queued = false;
            self.refresh(cx);
        } else if !self.writing
            && self.root.is_some()
            && self.last_scan.elapsed() > Duration::from_secs(2)
        {
            if self.scanning {
                self.refresh_queued = true;
            } else {
                self.refresh_queued = false;
                self.refresh(cx);
            }
        }
        if changed {
            cx.notify();
        }
    }
    /// Navigates aligned hunk ranges in the one shared scroll surface.
    fn move_hunk(&mut self, forward: bool, cx: &mut Context<Self>) {
        let Some(diff) = &self.view.diff else {
            return;
        };
        if diff.hunks.is_empty() {
            return;
        }
        if forward {
            self.hunk = (self.hunk + 1) % diff.hunks.len();
        } else {
            self.hunk = (self.hunk + diff.hunks.len() - 1) % diff.hunks.len();
        }
        self.diff_scroll
            .scroll_to_item(diff.hunks[self.hunk].start, ScrollStrategy::Center);
        cx.notify();
    }
    /// Provides file/hunk navigation and open/refresh keyboard shortcuts.
    fn key_down(&mut self, event: &KeyDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let key = event.keystroke.key.as_str();
        if self.confirmation.is_some() {
            if key == "escape" {
                self.cancel_confirmation(cx);
            }
            return;
        }
        if event.keystroke.modifiers.control && !event.keystroke.modifiers.shift && key == "enter" {
            self.commit(cx);
            return;
        }
        if event.keystroke.modifiers.control && key == "o" {
            self.choose_repository(cx);
            return;
        }
        if key == "f5" {
            if self.scanning {
                self.refresh_queued = true;
            } else {
                self.refresh(cx);
            }
            return;
        }
        if key == "f7" {
            self.move_hunk(!event.keystroke.modifiers.shift, cx);
            return;
        }
        if event.keystroke.modifiers.control && matches!(key, "1" | "2") {
            self.branch_tab = key == "2";
            cx.notify();
            return;
        }
        if event.keystroke.modifiers.control && event.keystroke.modifiers.shift && key == "enter" {
            self.perform_operation(OperationAction::Continue, cx);
            return;
        }
        if self.branch_tab && key == "enter" {
            if let Some(branch) = self.selected_branch.clone() {
                self.perform_operation(OperationAction::Switch(branch), cx);
            }
            return;
        }
        if self.branch_tab && matches!(key, "up" | "down") {
            if let Some(branches) = &self.branches
                && !branches.branches.is_empty()
            {
                let current = self
                    .selected_branch
                    .as_ref()
                    .and_then(|selected| {
                        branches
                            .branches
                            .iter()
                            .position(|branch| branch == selected)
                    })
                    .unwrap_or(0);
                let next = if key == "up" {
                    current.saturating_sub(1)
                } else {
                    (current + 1).min(branches.branches.len() - 1)
                };
                self.selected_branch = Some(branches.branches[next].clone());
                self.branch_scroll
                    .scroll_to_item(next, ScrollStrategy::Center);
                cx.notify();
            }
            return;
        }
        if matches!(key, "up" | "down")
            && let Some(repo) = &self.view.repository
        {
            if repo.files.is_empty() {
                return;
            }
            let current = repo
                .files
                .iter()
                .position(|file| Some(&file.path) == self.view.selected.as_ref())
                .unwrap_or(0);
            let next = if key == "up" {
                current.saturating_sub(1)
            } else {
                (current + 1).min(repo.files.len() - 1)
            };
            let path = repo.files[next].path.clone();
            self.file_scroll
                .scroll_to_item(next, ScrollStrategy::Center);
            self.select(path, cx);
        }
    }
    /// Runs a native scripted render scenario, with a finite timeout and isolated settings.
    fn smoke_tick(&mut self, cx: &mut Context<Self>) {
        let smoke = self.smoke.as_ref().expect("smoke state");
        if smoke.started.elapsed() > Duration::from_secs(30) {
            let report = serde_json::json!({"ok":false,"reason":"timeout","frames":self.frames,"error":self.error_key});
            let _ = std::fs::write(&smoke.report, serde_json::to_vec_pretty(&report).unwrap());
            cx.quit();
            return;
        }
        if smoke
            .operation
            .as_deref()
            .is_some_and(|action| action.starts_with("conflict"))
        {
            self.smoke_conflict_tick(cx);
            return;
        }
        if smoke.operation.is_some() {
            self.smoke_operation_tick(cx);
            return;
        }
        if smoke.commit && !smoke.message_ready {
            cx.notify();
            return;
        }
        if self.writing
            || self.scanning
            || self.diff_loading
            || self.view.diff.is_none()
            || self.frames <= smoke.last_frame
        {
            return;
        }
        let phase = smoke.phase;
        if phase == 0 {
            if let Some(repo) = &self.view.repository
                && let Some(file) = repo
                    .files
                    .iter()
                    .find(|file| file.staged.is_some() && file.unstaged.is_some())
            {
                let path = file.path.clone();
                self.select(path, cx);
            }
        } else if phase <= 3 {
            let locale = [Locale::En, Locale::Ru, Locale::RuDev][phase - 1];
            if phase == 1 {
                let diff = self.view.diff.as_ref().unwrap();
                let numbers = [true, false].map(|left| {
                    diff.rows
                        .iter()
                        .filter(|row| row.changed)
                        .filter_map(|row| {
                            let line = if left { &row.left } else { &row.right };
                            line.as_ref().map(|line| line.number - 1)
                        })
                        .collect::<Vec<_>>()
                });
                for (left, numbers) in [true, false].into_iter().zip(numbers) {
                    if let Some(first) = numbers.first() {
                        self.toggle_line(left, *first, false, cx);
                        self.toggle_line(left, *numbers.last().unwrap(), true, cx);
                        let selected = if left {
                            &self.view.line_selection.removed
                        } else {
                            &self.view.line_selection.added
                        };
                        assert_eq!(*selected, numbers.into_iter().collect());
                    }
                }
            }
            let line_selection = self.view.line_selection.clone();
            let generation = self.view.diff_generation;
            let selected = self.view.selected.clone();
            let diff = self.view.diff.clone();
            let message = self.message.read(cx).buffer.text().to_owned();
            let message_selection = self.message.read(cx).buffer.selection;
            let message_revision = self.message.read(cx).buffer.revision;
            self.set_locale(locale, cx);
            assert_eq!(self.message.read(cx).buffer.text(), message);
            assert_eq!(self.message.read(cx).buffer.selection, message_selection);
            assert_eq!(self.message.read(cx).buffer.revision, message_revision);
            assert_eq!(self.view.diff_generation, generation);
            assert_eq!(self.view.line_selection.removed, line_selection.removed);
            assert_eq!(self.view.line_selection.added, line_selection.added);
            assert_eq!(self.view.selected, selected);
            assert!(std::sync::Arc::ptr_eq(
                diff.as_ref().unwrap(),
                self.view.diff.as_ref().unwrap()
            ));
            let label = self.t("commit");
            self.smoke.as_mut().unwrap().locales.push(label);
        } else if phase == 4 {
            self.set_mode(DiffKind::Staged, cx);
        } else if phase == 5 {
            self.set_mode(DiffKind::Unstaged, cx);
        } else if phase == 6 && self.smoke.as_ref().unwrap().commit {
            self.commit(cx);
            assert!(self.writing);
            assert!(!self.message.read(cx).enabled);
            if self.smoke.as_ref().unwrap().close_on_write {
                assert!(!self.request_close(cx));
            }
        } else if phase == 7 && self.smoke.as_ref().unwrap().commit {
            let smoke = self.smoke.as_ref().unwrap();
            let message = &self.message.read(cx).buffer;
            let retained = message.text() == SMOKE_MESSAGE
                && Some(message.selection) == smoke.message_selection;
            let ok = if smoke.hook_refusal {
                self.write_error_key == Some("error-hook-message")
                    && self.committed.is_none()
                    && retained
            } else {
                self.write_error_key.is_none()
                    && self.committed.is_some()
                    && message.text().is_empty()
            };
            let report = serde_json::json!({"ok":ok,"frames":self.frames,"native_display":true,"commit_id":self.committed.as_ref().map(|commit| &commit.id),"write_backend":self.last_write_backend,"write_error":self.write_error_key,"message_cleared":message.text().is_empty(),"message_retained":retained,"input_checked":smoke.message_ready,"locales":smoke.locales});
            std::fs::write(&smoke.report, serde_json::to_vec_pretty(&report).unwrap())
                .expect("commit smoke report");
            cx.quit();
            return;
        } else if phase == 6 && self.smoke.as_ref().unwrap().stage {
            let path = self
                .root
                .as_ref()
                .unwrap()
                .join(self.view.selected.as_ref().unwrap());
            self.smoke.as_mut().unwrap().worktree =
                Some(std::fs::read(path).expect("smoke working file"));
            let diff = self.view.diff.as_ref().unwrap();
            if let Some(range) = diff.hunks.first() {
                for row in &diff.rows[range.clone()] {
                    if let Some(line) = &row.left {
                        self.view.line_selection.removed.insert(line.number - 1);
                    }
                    if let Some(line) = &row.right {
                        self.view.line_selection.added.insert(line.number - 1);
                    }
                }
            }
            self.write_selected(cx);
            if self.smoke.as_ref().unwrap().close_on_write {
                assert!(!self.request_close(cx));
            }
        } else if phase == 7 && self.smoke.as_ref().unwrap().refusal {
            let diff = self.view.diff.as_ref().unwrap();
            let range = diff.hunks.first().unwrap();
            let expected = LineSelection {
                removed: diff.rows[range.clone()]
                    .iter()
                    .filter_map(|row| row.left.as_ref().map(|line| line.number - 1))
                    .collect(),
                added: diff.rows[range.clone()]
                    .iter()
                    .filter_map(|row| row.right.as_ref().map(|line| line.number - 1))
                    .collect(),
            };
            let preserved = self.view.line_selection.removed == expected.removed
                && self.view.line_selection.added == expected.added;
            let report = serde_json::json!({"ok": preserved && self.write_error_key == Some("error-index-lock") && self.last_write_backend.is_none(), "selection_preserved":preserved,"write_error":self.write_error_key,"frames":self.frames});
            std::fs::write(
                &self.smoke.as_ref().unwrap().report,
                serde_json::to_vec_pretty(&report).unwrap(),
            )
            .expect("refusal smoke report");
            cx.quit();
            return;
        } else if phase == 7 && self.smoke.as_ref().unwrap().stage {
            let diff = self.view.diff.as_ref().unwrap();
            self.smoke
                .as_mut()
                .unwrap()
                .index_versions
                .push(String::from_utf8(diff.left.bytes.clone()).expect("UTF-8 smoke fixture"));
            self.set_mode(DiffKind::Staged, cx);
        } else if phase == 8 && self.smoke.as_ref().unwrap().stage {
            self.hunk = self
                .view
                .diff
                .as_ref()
                .unwrap()
                .hunks
                .len()
                .saturating_sub(1);
            self.write_hunk(cx);
        } else if phase == 9 && self.smoke.as_ref().unwrap().stage {
            let diff = self.view.diff.as_ref().unwrap();
            self.smoke
                .as_mut()
                .unwrap()
                .index_versions
                .push(String::from_utf8(diff.right.bytes.clone()).expect("UTF-8 smoke fixture"));
            self.set_mode(DiffKind::Unstaged, cx);
        } else {
            let smoke = self.smoke.as_ref().unwrap();
            let diff = self.view.diff.as_ref().unwrap();
            if let Some(before) = &smoke.worktree {
                assert_eq!(
                    *before,
                    std::fs::read(self.root.as_ref().unwrap().join(&diff.path))
                        .expect("smoke working file")
                );
            }
            let report = serde_json::json!({"ok":self.frames>0&&diff.presentation==Presentation::Text&&!diff.hunks.is_empty()&&self.write_error_key.is_none()&&(!smoke.stage||(self.last_write_backend==Some("gix")&&smoke.index_versions.len()==2)),
                "frames":self.frames,"file":diff.path,"locales":smoke.locales,"rows":diff.rows.len(),"hunks":diff.hunks.len(),
                "mode":"index-to-worktree","backend":diff.backend,"native_display":true, "index_written":self.last_write_backend.is_some(), "write_backend":self.last_write_backend, "write_error": self.write_error_key,
                "index_versions":smoke.index_versions, "files":self.view.repository.as_ref().map(|repo| repo.files.len()), "elapsed_ms":smoke.started.elapsed().as_millis()});
            std::fs::write(&smoke.report, serde_json::to_vec_pretty(&report).unwrap())
                .expect("smoke report");
            cx.quit();
            return;
        }
        let smoke = self.smoke.as_mut().unwrap();
        smoke.phase += 1;
        smoke.last_frame = self.frames;
        cx.notify();
    }
    /// Runs branch/coordinator handlers in a real finite native window using only caller-owned disposable fixtures.
    fn smoke_operation_tick(&mut self, cx: &mut Context<Self>) {
        let smoke = self.smoke.as_ref().unwrap();
        if self.writing
            || self.scanning
            || self.diff_loading
            || self.view.repository.is_none()
            || self.reviewed_operation.is_none()
            || !smoke.input_prepared
            || self.frames <= smoke.last_frame
        {
            return;
        }
        let phase = smoke.phase;
        let operation = smoke.operation.clone().unwrap();
        if phase == 0 {
            self.branch_tab = operation != "preview-changes";
        } else if phase <= 3 {
            let message = &self.message.read(cx).buffer;
            let preserved = (
                message.text().to_owned(),
                message.selection,
                message.revision,
            );
            let input = &self.branch_input.read(cx).buffer;
            let branch_preserved = (input.text().to_owned(), input.selection, input.revision);
            let generation = self.scan_generation;
            self.set_locale([Locale::En, Locale::Ru, Locale::RuDev][phase - 1], cx);
            let message = &self.message.read(cx).buffer;
            assert_eq!(
                (
                    message.text().to_owned(),
                    message.selection,
                    message.revision
                ),
                preserved
            );
            let input = &self.branch_input.read(cx).buffer;
            assert_eq!(
                (input.text().to_owned(), input.selection, input.revision),
                branch_preserved
            );
            assert_eq!(self.scan_generation, generation);
            let label = self.t("branches");
            self.smoke.as_mut().unwrap().locales.push(label);
        } else if phase == 4 {
            if operation.starts_with("preview") {
                if self.selected_branch.is_none() {
                    self.selected_branch = self
                        .branches
                        .as_ref()
                        .and_then(|snapshot| snapshot.branches.first())
                        .cloned();
                }
                if self.smoke.as_ref().unwrap().started.elapsed() < Duration::from_secs(5) {
                    cx.notify();
                    return;
                }
            } else if operation.starts_with("branch") {
                self.create_branch(cx);
            } else {
                let target = |name| {
                    self.branches
                        .as_ref()
                        .unwrap()
                        .branches
                        .iter()
                        .find(|branch| branch.name == name)
                        .unwrap()
                        .clone()
                };
                let action = match operation.as_str() {
                    "switch" => OperationAction::Switch(target("other")),
                    "merge" | "merge-hook" => OperationAction::Merge(target("other")),
                    "rebase" => OperationAction::Rebase(target("main")),
                    "continue" => OperationAction::Continue,
                    "abort" => OperationAction::Abort,
                    "skip" => OperationAction::Skip,
                    _ => panic!("unknown isolated operation smoke"),
                };
                self.perform_operation(action, cx);
                if matches!(operation.as_str(), "abort" | "skip") {
                    assert!(
                        self.confirmation
                            .as_ref()
                            .is_some_and(|prompt| prompt.kind == ConfirmationKind::Destructive)
                    );
                    self.confirm_intent(cx);
                }
            }
            if !operation.starts_with("preview") {
                assert!(self.writing);
                assert!(!self.message.read(cx).enabled && !self.branch_input.read(cx).enabled);
            }
            if self.smoke.as_ref().unwrap().close_on_write {
                assert!(!self.request_close(cx));
            }
        } else {
            let smoke = self.smoke.as_ref().unwrap();
            let message_retained = self.message.read(cx).buffer.text() == SMOKE_MESSAGE;
            let branch_retained = self.branch_input.read(cx).buffer.text() == "задача/интерфейс";
            let refusal = operation == "branch-refusal" || operation == "merge-hook";
            let ok = message_retained
                && self.frames > 0
                && smoke.locales.len() == 3
                && if operation.starts_with("preview") {
                    self.write_error_key.is_none() && self.last_write_backend.is_none()
                } else if refusal {
                    branch_retained
                        && self.write_error_key.is_some()
                        && self.last_write_backend.is_none()
                        && !self.close_requested
                } else {
                    self.write_error_key.is_none()
                        && self.last_write_backend.is_some()
                        && (operation != "branch" || !branch_retained)
                };
            let state = self.operation.as_ref().unwrap().state.as_ref();
            let report = serde_json::json!({"ok":ok,"native_display":true,"frames":self.frames,"action":operation,
                "write_backend":self.last_write_backend,"write_error":self.write_error_key,
                "route":self.last_operation_route.map(|route| format!("{:?}", route.capability)),
                "message_retained":message_retained,"branch_retained":branch_retained,"locales":smoke.locales,
                "head":self.view.repository.as_ref().unwrap().head,
                "branch":self.view.repository.as_ref().unwrap().branch,
                "operation":state.map(|state| format!("{:?}", state.kind)),
                "progress":state.and_then(|state| state.progress),
                "outcome":self.operation_written.as_ref().map(|write| format!("{:?}",write.outcome)),
                "branch_created":self.branch_created.as_ref().map(|write| &write.branch.name)});
            std::fs::write(&smoke.report, serde_json::to_vec_pretty(&report).unwrap()).unwrap();
            cx.quit();
            return;
        }
        let smoke = self.smoke.as_mut().unwrap();
        smoke.phase += 1;
        smoke.last_frame = self.frames;
        cx.notify();
    }
    /// Exercises chunk choices, real Result input and keyboard undo/redo in a finite fixture window.
    fn smoke_conflict_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.smoke.as_ref().is_some_and(|smoke| {
            smoke
                .operation
                .as_deref()
                .is_some_and(|action| action.starts_with("conflict"))
                && !smoke.result_ready
        }) {
            return;
        }
        let Some(view) = self.conflict.clone() else {
            return;
        };
        let snapshot = view.read(cx).snapshot.clone();
        if self.smoke.as_ref().unwrap().operation.as_deref() == Some("conflict-external") {
            assert_ne!(snapshot.presentation, Presentation::Text);
            assert!(!view.read(cx).result.read(cx).enabled && !view.read(cx).dirty(cx));
            let smoke = self.smoke.as_mut().unwrap();
            smoke.conflict_guard = Some(snapshot.operation.clone());
            smoke.conflict_path = Some(snapshot.path.clone());
            smoke.worktree = Some(snapshot.result.bytes.clone());
            smoke.result_ready = true;
            self.branch_tab = false;
            return;
        }
        assert_eq!(snapshot.presentation, Presentation::Text);
        let target = if snapshot.result.bytes == SMOKE_RESULT.as_bytes() {
            format!("{SMOKE_RESULT}\r\nВторая редакция")
        } else {
            SMOKE_RESULT.to_owned()
        };
        self.smoke.as_mut().unwrap().result_text = Some(target.clone());
        self.smoke.as_mut().unwrap().conflict_guard = Some(snapshot.operation.clone());
        self.smoke.as_mut().unwrap().conflict_path = Some(snapshot.path.clone());
        self.smoke.as_mut().unwrap().worktree = Some(snapshot.result.bytes.clone());
        view.update(cx, |view, cx| {
            let original = view.result.read(cx).buffer.text().to_owned();
            let revision = view.result.read(cx).buffer.revision;
            let key = |stroke| KeyDownEvent {
                keystroke: Keystroke::parse(stroke).unwrap(),
                is_held: false,
            };
            if !hunkra_core::conflict_chunks::chunks(original.as_bytes())
                .unwrap()
                .is_empty()
            {
                view.take_chunk(
                    0,
                    revision,
                    hunkra_core::conflict_chunks::ChunkChoice::AThenB,
                    cx,
                );
                view.result
                    .update(cx, |editor, cx| editor.key_down(&key("ctrl-z"), window, cx));
                assert_eq!(view.result.read(cx).buffer.text(), original);
            }
            view.result.update(cx, |editor, cx| {
                window.focus(&editor.focus_handle(cx));
                let length = editor.buffer.utf16_offset(editor.buffer.text().len());
                editor.replace_text_in_range(Some(0..length), &target, window, cx);
                editor.key_down(&key("ctrl-z"), window, cx);
                assert_eq!(editor.buffer.text(), original);
                editor.key_down(&key("ctrl-shift-z"), window, cx);
                assert_eq!(editor.buffer.text(), target);
                editor.buffer.select(0, false);
                editor.buffer.select("Результат".len(), true);
                cx.notify();
            });
        });
        self.smoke.as_mut().unwrap().result_selection =
            Some(view.read(cx).result.read(cx).buffer.selection);
        self.smoke.as_mut().unwrap().result_ready = true;
        self.branch_tab = false;
    }
    /// Verifies native Result persistence/resolve/continue and product confirmations on disposable repos.
    fn smoke_conflict_tick(&mut self, cx: &mut Context<Self>) {
        let smoke = self.smoke.as_ref().unwrap();
        if !smoke.result_ready
            || self.writing
            || self.scanning
            || self.diff_loading
            || self.frames <= smoke.last_frame
        {
            return;
        }
        let phase = smoke.phase;
        let action = smoke.operation.clone().unwrap();
        if action == "conflict-external" {
            self.smoke_external_conflict_tick(cx);
            return;
        }
        let target = smoke.result_text.clone().unwrap();
        if phase == 9 && !action.contains("restage") || phase == 11 {
            assert_eq!(self.last_write_backend, Some("git-cli"));
            self.finish_conflict_smoke(cx);
            return;
        }
        let Some(view) = self.conflict.clone() else {
            return;
        };
        if phase == 0 {
            assert!(view.read(cx).dirty(cx));
        } else if phase <= 3 {
            let editor = &view.read(cx).result.read(cx).buffer;
            assert_eq!(editor.text(), target);
            assert_eq!(Some(editor.selection), smoke.result_selection);
            let revision = editor.revision;
            let generation = self.scan_generation;
            self.set_locale([Locale::En, Locale::Ru, Locale::RuDev][phase - 1], cx);
            if phase <= 2 {
                view.update(cx, |view, cx| view.toggle_base(cx));
            }
            assert_eq!(view.read(cx).result.read(cx).buffer.revision, revision);
            assert_eq!(self.scan_generation, generation);
            let label = self.t("result");
            self.smoke.as_mut().unwrap().locales.push(label);
            if phase == 3 {
                assert!(!self.request_close(cx));
                assert!(
                    self.confirmation
                        .as_ref()
                        .is_some_and(|prompt| prompt.kind == ConfirmationKind::Dirty)
                );
                self.cancel_confirmation(cx);
                let selected = self.view.selected.clone();
                self.select(PathBuf::from("unreviewed-navigation"), cx);
                assert!(self.confirmation.is_some());
                assert_eq!(self.view.selected, selected);
                self.write_index(Selection::File, cx);
                self.commit(cx);
                self.create_branch(cx);
                assert!(!self.writing);
                self.cancel_confirmation(cx);
                assert!(view.read(cx).dirty(cx));
                if action == "conflict-save-navigate" {
                    self.select(PathBuf::from("g"), cx);
                    assert!(self.confirmation.is_some());
                    self.save_before_intent(cx);
                    assert!(self.confirmation.is_none());
                }
                if action.contains("stale") {
                    let path = self
                        .root
                        .as_ref()
                        .unwrap()
                        .join(self.smoke.as_ref().unwrap().conflict_path.as_ref().unwrap());
                    std::fs::write(path, b"external edit\r\n").unwrap();
                }
                if action != "conflict-save-navigate" {
                    self.refresh(cx);
                }
            }
        } else if phase == 4 {
            if action == "conflict-save-navigate" {
                assert_eq!(
                    self.view.selected.as_deref(),
                    Some(std::path::Path::new("g"))
                );
                assert_eq!(self.last_write_backend, Some("filesystem"));
                assert_eq!(
                    hunkra_core::prepare_operation(self.root.as_ref().unwrap()).unwrap(),
                    self.smoke.as_ref().unwrap().conflict_guard.clone().unwrap()
                );
                self.finish_conflict_smoke(cx);
                return;
            }
            assert_eq!(view.read(cx).result.read(cx).buffer.text(), target);
            assert_eq!(
                Some(view.read(cx).result.read(cx).buffer.selection),
                self.smoke.as_ref().unwrap().result_selection
            );
            if action.contains("stale") {
                assert!(view.read(cx).draft.stale);
                view.update(cx, |view, cx| view.save(cx));
                assert!(!self.writing);
                self.finish_conflict_smoke(cx);
                return;
            }
            if action.contains("delete") {
                view.update(cx, |view, cx| view.choose_presence(false, cx));
            }
            view.update(cx, |view, cx| view.save(cx));
        } else if phase == 5 && action.contains("delete") {
            assert!(
                self.confirmation
                    .as_ref()
                    .is_some_and(|prompt| prompt.kind == ConfirmationKind::Deletion)
            );
            let path = self
                .root
                .as_ref()
                .unwrap()
                .join(self.smoke.as_ref().unwrap().conflict_path.as_ref().unwrap());
            assert!(path.exists());
            if action.contains("cancel") {
                self.cancel_confirmation(cx);
                assert!(view.read(cx).dirty(cx));
                assert_eq!(
                    hunkra_core::prepare_operation(self.root.as_ref().unwrap()).unwrap(),
                    self.smoke.as_ref().unwrap().conflict_guard.clone().unwrap()
                );
                self.finish_conflict_smoke(cx);
                return;
            }
            self.confirm_intent(cx);
        } else if phase == 6 {
            if action.contains("refusal") {
                assert!(self.write_error_key.is_some());
                assert!(!self.close_requested);
                assert!(view.read(cx).dirty(cx));
                assert_eq!(view.read(cx).result.read(cx).buffer.text(), target);
                self.finish_conflict_smoke(cx);
                return;
            }
            assert_eq!(self.last_write_backend, Some("filesystem"));
            assert_eq!(
                hunkra_core::prepare_operation(self.root.as_ref().unwrap()).unwrap(),
                self.smoke.as_ref().unwrap().conflict_guard.clone().unwrap()
            );
            assert!(!view.read(cx).snapshot.resolved);
            assert!(!view.read(cx).dirty(cx));
            let path = self
                .root
                .as_ref()
                .unwrap()
                .join(self.smoke.as_ref().unwrap().conflict_path.as_ref().unwrap());
            if action.contains("delete") {
                assert!(!path.exists());
            } else {
                assert_eq!(std::fs::read(path).unwrap(), target.as_bytes());
            }
            if action.ends_with("save") {
                self.finish_conflict_smoke(cx);
                return;
            }
        } else if phase == 7 {
            view.update(cx, |view, cx| view.resolve(cx));
        } else if phase == 8 {
            assert_eq!(self.last_write_backend, Some("gix"));
            assert!(view.read(cx).snapshot.resolved && view.read(cx).snapshot.result_staged);
            if action.contains("restage") {
                view.update(cx, |view, cx| {
                    view.take_side(false, cx);
                    assert!(view.dirty(cx));
                    view.save(cx);
                });
            } else {
                if !action.ends_with("continue") {
                    self.finish_conflict_smoke(cx);
                    return;
                }
                self.perform_operation(OperationAction::Continue, cx);
                assert!(self.writing);
            }
        } else if phase == 9 {
            assert_eq!(self.last_write_backend, Some("filesystem"));
            assert!(view.read(cx).snapshot.resolved && !view.read(cx).snapshot.result_staged);
            self.perform_operation(OperationAction::Continue, cx);
            assert!(!self.writing);
            assert_eq!(self.write_error_key, Some("error-result-index"));
            view.update(cx, |view, cx| view.resolve(cx));
        } else if phase == 10 {
            assert_eq!(self.last_write_backend, Some("gix"));
            assert!(view.read(cx).snapshot.result_staged);
            self.perform_operation(OperationAction::Continue, cx);
            assert!(self.writing);
        }
        let smoke = self.smoke.as_mut().unwrap();
        smoke.phase += 1;
        smoke.last_frame = self.frames;
        cx.notify();
    }
    /// Exercises read-only unsupported sources and explicit resolution of externally saved fixture bytes.
    fn smoke_external_conflict_tick(&mut self, cx: &mut Context<Self>) {
        let phase = self.smoke.as_ref().unwrap().phase;
        let view = self.conflict.clone().unwrap();
        assert_ne!(view.read(cx).snapshot.presentation, Presentation::Text);
        assert!(!view.read(cx).result.read(cx).enabled && !view.read(cx).dirty(cx));
        if phase <= 3 {
            assert_eq!(self.last_write_backend, None);
            let before = hunkra_core::prepare_operation(self.root.as_ref().unwrap()).unwrap();
            assert_eq!(
                before,
                self.smoke.as_ref().unwrap().conflict_guard.clone().unwrap()
            );
            if phase > 0 {
                let generation = self.scan_generation;
                self.set_locale([Locale::En, Locale::Ru, Locale::RuDev][phase - 1], cx);
                view.update(cx, |view, cx| {
                    view.toggle_base(cx);
                    view.save(cx);
                });
                assert!(!self.writing);
                assert_eq!(self.scan_generation, generation);
                let label = self.t("result");
                self.smoke.as_mut().unwrap().locales.push(label);
            }
        } else if phase == 4 {
            view.update(cx, |view, cx| view.resolve(cx));
        } else {
            assert_eq!(self.last_write_backend, Some("gix"));
            assert!(view.read(cx).snapshot.resolved && view.read(cx).snapshot.result_staged);
            let path = self
                .root
                .as_ref()
                .unwrap()
                .join(&view.read(cx).snapshot.path);
            assert_eq!(
                std::fs::read(path).unwrap(),
                self.smoke.as_ref().unwrap().worktree.clone().unwrap()
            );
            self.finish_conflict_smoke(cx);
            return;
        }
        let smoke = self.smoke.as_mut().unwrap();
        smoke.phase += 1;
        smoke.last_frame = self.frames;
        cx.notify();
    }
    /// Writes compact fixture evidence only after the native flow has reached its asserted state.
    fn finish_conflict_smoke(&mut self, cx: &mut Context<Self>) {
        let smoke = self.smoke.as_ref().unwrap();
        let report = serde_json::json!({"ok":true,"native_display":true,"frames":self.frames,"locales":smoke.locales,
            "scenario":smoke.operation,"writer_backend":self.last_write_backend,"dirty_confirmation":smoke.operation.as_deref()!=Some("conflict-external"),
            "delete_confirmation":smoke.operation.as_deref().is_some_and(|action| action.contains("delete")),
            "draft_retained":true,"close_cancelled":!self.close_requested,"saved_file":smoke.conflict_path,
            "second_edit":smoke.result_text.as_deref().is_some_and(|text| text != SMOKE_RESULT)});
        std::fs::write(&smoke.report, serde_json::to_vec_pretty(&report).unwrap()).unwrap();
        cx.quit();
    }
    /// Exercises real native input callbacks, Cyrillic composition and editing keys in the smoke window.
    fn smoke_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.smoke_conflict_input(window, cx);
        if self
            .smoke
            .as_ref()
            .is_some_and(|smoke| smoke.operation.is_some() && !smoke.input_prepared)
        {
            self.message.update(cx, |editor, cx| {
                editor.replace_text_in_range(None, SMOKE_MESSAGE, window, cx);
                editor.buffer.select(0, false);
                editor.buffer.select("Подготовить".len(), true);
                cx.notify();
            });
            self.branch_input.update(cx, |editor, cx| {
                editor.replace_text_in_range(None, "задача/интерфейс", window, cx);
                editor.buffer.select(0, false);
                editor.buffer.select("задача".len(), true);
                cx.notify();
            });
            let smoke = self.smoke.as_mut().unwrap();
            smoke.input_prepared = true;
            smoke.message_ready = true;
        }
        let Some(smoke) = self
            .smoke
            .as_ref()
            .filter(|smoke| smoke.commit && !smoke.message_ready)
        else {
            return;
        };
        if !smoke.input_prepared {
            self.message.update(cx, |editor, cx| {
                window.focus(&editor.focus_handle(cx));
                editor.replace_text_in_range(None, "Подготовить A+B 🦀\n\n", window, cx);
                editor.replace_and_mark_text_in_range(None, "Соо", Some(1..2), window, cx);
                editor.replace_and_mark_text_in_range(None, "Сообщение", Some(2..4), window, cx);
                assert_eq!(&editor.buffer.text()[editor.buffer.selection.range()], "об");
                editor.replace_text_in_range(None, "Сообщение из поля", window, cx);
                let key = |stroke| KeyDownEvent {
                    keystroke: Keystroke::parse(stroke).unwrap(),
                    is_held: false,
                };
                editor.key_down(&key("ctrl-z"), window, cx);
                assert_eq!(editor.buffer.text(), "Подготовить A+B 🦀\n\n");
                editor.key_down(&key("ctrl-shift-z"), window, cx);
                assert_eq!(editor.buffer.text(), SMOKE_MESSAGE);
                editor.buffer.select("Подготовить".len(), false);
                editor.buffer.select(0, true);
                assert!(
                    editor
                        .selected_text_range(false, window, cx)
                        .unwrap()
                        .reversed
                );
                cx.notify();
            });
            let smoke = self.smoke.as_mut().unwrap();
            smoke.input_prepared = true;
            smoke.message_selection = Some(self.message.read(cx).buffer.selection);
            cx.notify();
        } else {
            let checked = self.message.update(cx, |editor, cx| {
                let selected = editor.selected_text_range(false, window, cx).unwrap();
                let Some(bounds) =
                    editor.bounds_for_range(selected.range, Bounds::default(), window, cx)
                else {
                    return false;
                };
                let index = editor.character_index_for_point(bounds.origin, window, cx);
                assert_eq!(index, Some(0));
                assert!(bounds.size.width > px(0.));
                true
            });
            self.smoke.as_mut().unwrap().message_ready = checked;
        }
    }
    /// Builds one virtualized file row with independent staged and unstaged indicators.
    fn file_row(&self, index: usize, cx: &Context<Self>) -> gpui::Stateful<gpui::Div> {
        let file = &self.view.repository.as_ref().unwrap().files[index];
        let path = file.path.clone();
        let active = self.view.selected.as_ref() == Some(&path);
        let symbol = |change: Option<ChangeKind>| match change {
            Some(ChangeKind::Added) => "+",
            Some(ChangeKind::Deleted) => "−",
            Some(ChangeKind::Conflict) => "!",
            Some(ChangeKind::Renamed) => "R",
            Some(ChangeKind::TypeChanged) => "T",
            Some(ChangeKind::IntentToAdd) => "?",
            Some(ChangeKind::Modified) => "M",
            None => "·",
        };
        div()
            .id(("file", index))
            .h(px(30.))
            .px_3()
            .flex()
            .items_center()
            .gap_2()
            .cursor_pointer()
            .overflow_hidden()
            .bg(rgb(if active { SELECTED } else { PANEL }))
            .border_l_2()
            .border_color(rgb(if active { ACCENT } else { PANEL }))
            .hover(|style| style.bg(rgb(HOVER)))
            .child(
                div()
                    .w(px(32.))
                    .text_color(rgb(ACCENT))
                    .text_sm()
                    .child(format!("{} {}", symbol(file.staged), symbol(file.unstaged))),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_sm()
                    .text_ellipsis()
                    .child(path.to_string_lossy().into_owned()),
            )
            .on_click(cx.listener(move |app, _, _, cx| app.select(path.clone(), cx)))
    }
    /// Renders one aligned pair with independently selected old and new lines.
    fn diff_row(&self, index: usize, cx: &Context<Self>) -> gpui::Stateful<gpui::Div> {
        let row = &self.view.diff.as_ref().unwrap().rows[index];
        div()
            .id(("diff", index))
            .h(px(25.))
            .flex()
            .w_full()
            .overflow_hidden()
            .child(self.diff_cell(&row.left, row, true, index, cx))
            .child(self.diff_cell(&row.right, row, false, index, cx))
    }
    /// Keeps an empty side as an aligned blank; display text never changes document bytes.
    fn diff_cell(
        &self,
        line: &Option<Line>,
        row: &DiffRow,
        left: bool,
        index: usize,
        cx: &Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let changed = row.changed && line.is_some();
        let (number, text, ending) = line
            .as_ref()
            .map(|line| {
                (
                    line.number.to_string(),
                    line.text.replace('\t', "    "),
                    line.ending,
                )
            })
            .unwrap_or_default();
        let marker = if changed {
            if left { "−" } else { "+" }
        } else {
            " "
        };
        let selected = line.as_ref().is_some_and(|line| {
            let selected = if left {
                &self.view.line_selection.removed
            } else {
                &self.view.line_selection.added
            };
            selected.contains(&(line.number - 1))
        });
        let line_number = line.as_ref().map(|line| line.number - 1);
        let version = self.view.diff.clone();
        let mut cell = div()
            .id((if left { "old-line" } else { "new-line" }, index))
            .on_click(cx.listener(move |app, event: &gpui::ClickEvent, _, cx| {
                if changed
                    && let Some(number) = line_number
                    && let Some(rendered) = &version
                    && app.view.is_current_diff(rendered)
                {
                    app.toggle_line(left, number, event.modifiers().shift, cx);
                }
            }))
            .flex()
            .flex_1()
            .min_w_0()
            .h_full()
            .items_center()
            .font_family("DejaVu Sans Mono")
            .text_size(px(12.))
            .gap_2()
            .border_r_1()
            .border_color(rgb(LINE))
            .bg(rgb(if selected {
                SELECTED
            } else if changed {
                if left { 0x3a2d36 } else { 0x2b3834 }
            } else {
                BG
            }))
            .child(
                div()
                    .w(px(45.))
                    .flex_none()
                    .text_right()
                    .text_color(rgb(MUTED))
                    .child(number),
            )
            .child(
                div()
                    .w(px(12.))
                    .flex_none()
                    .text_color(rgb(if left { 0xf29d9d } else { ACCENT }))
                    .child(marker),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .child(text),
            );
        if ending == "EOF" && line.is_some() {
            cell = cell.child(div().px_2().text_color(rgb(0xe1be86)).child("∅"));
        }
        cell
    }
    /// Builds the standalone sidebar, while lazily rendering only visible file rows.
    fn sidebar(&self, cx: &Context<Self>) -> gpui::Div {
        let mut sidebar = div()
            .w(px(280.))
            .flex_none()
            .flex()
            .flex_col()
            .bg(rgb(PANEL))
            .border_r_1()
            .border_color(rgb(LINE));
        sidebar = sidebar.child(
            div()
                .h(px(36.))
                .flex()
                .items_center()
                .gap_1()
                .px_2()
                .border_b_1()
                .border_color(rgb(LINE))
                .child(
                    button("changes-tab", self.t("changes"), !self.branch_tab).on_click(
                        cx.listener(|app, _, _, cx| {
                            app.branch_tab = false;
                            cx.notify();
                        }),
                    ),
                )
                .child(
                    button("branches-tab", self.t("branches"), self.branch_tab).on_click(
                        cx.listener(|app, _, _, cx| {
                            app.branch_tab = true;
                            cx.notify();
                        }),
                    ),
                ),
        );
        if self.branch_tab {
            return sidebar.child(self.branch_panel(cx));
        }
        let mut info = div()
            .p_3()
            .flex()
            .flex_col()
            .gap_2()
            .border_b_1()
            .border_color(rgb(LINE));
        if let Some(repo) = &self.view.repository {
            info = info.child(
                div()
                    .text_sm()
                    .text_color(rgb(MUTED))
                    .child(repo.root.to_string_lossy().into_owned()),
            );
            let branch = repo.branch.clone().unwrap_or_else(|| self.t("detached"));
            info = info.child(div().text_color(rgb(ACCENT)).child(format!("⑂  {branch}")));
            if repo.unborn {
                info = info.child(
                    div()
                        .text_sm()
                        .text_color(rgb(MUTED))
                        .child(self.t("unborn")),
                );
            }
            if let Some(operation) = repo.operation {
                info = info.child(div().text_sm().text_color(rgb(0xe1be86)).child(self.t(
                    match operation {
                        Operation::Merge => "operation-merge",
                        Operation::Rebase => "operation-rebase",
                        Operation::Other => "operation-other",
                    },
                )));
            }
            info = info.child(
                div()
                    .text_sm()
                    .text_color(rgb(MUTED))
                    .child(self.translator.count(repo.files.len())),
            );
        } else {
            info = info.child(self.t(if self.scanning { "loading" } else { "changes" }));
        }
        info = info.child(div().text_xs().text_color(rgb(MUTED)).child(format!(
            "{}  /  {}",
            self.t("staged"),
            self.t("unstaged")
        )));
        sidebar = sidebar.child(info);
        if let Some(repo) = &self.view.repository
            && !repo.files.is_empty()
        {
            sidebar = sidebar.child(
                uniform_list(
                    "files",
                    repo.files.len(),
                    cx.processor(|app, range: std::ops::Range<usize>, _, cx| {
                        range
                            .map(|index| app.file_row(index, cx))
                            .collect::<Vec<_>>()
                    }),
                )
                .flex_1()
                .min_h_0()
                .track_scroll(self.file_scroll.clone()),
            );
        } else {
            sidebar = sidebar.child(
                div()
                    .p_4()
                    .text_sm()
                    .text_color(rgb(MUTED))
                    .child(self.t("empty")),
            );
        }
        sidebar
    }
    /// Renders a local branch selection separately from the explicit switch/merge/rebase actions.
    fn branch_row(&self, index: usize, cx: &Context<Self>) -> gpui::Stateful<gpui::Div> {
        let snapshot = self.branches.as_ref().unwrap();
        let branch = snapshot.branches[index].clone();
        let root = snapshot.repository.root.clone();
        let selected = self.selected_branch.as_ref() == Some(&branch);
        let current = self
            .view
            .repository
            .as_ref()
            .and_then(|repo| repo.branch.as_deref())
            == Some(&branch.name);
        div()
            .id(("branch-row", index))
            .h(px(30.))
            .px_3()
            .flex()
            .items_center()
            .gap_2()
            .bg(rgb(if selected { SELECTED } else { PANEL }))
            .cursor_pointer()
            .hover(|style| style.bg(rgb(HOVER)))
            .child(
                div()
                    .text_color(rgb(if current { ACCENT } else { MUTED }))
                    .child(if current { "●" } else { "⑂" }),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_sm()
                    .text_ellipsis()
                    .child(branch.name.clone()),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(MUTED))
                    .child(branch.id.chars().take(7).collect::<String>()),
            )
            .on_click(cx.listener(move |app, _, window, cx| {
                if app.root.as_ref() == Some(&root) && !app.writing {
                    app.selected_branch = Some(branch.clone());
                    window.focus(&app.focus);
                    cx.notify();
                }
            }))
    }
    /// Builds the compact branch panel with a native single-line name field and virtual branch rows.
    fn branch_panel(&self, cx: &Context<Self>) -> gpui::Div {
        let ready = !self.writing
            && !self.scanning
            && self.reviewed_operation.is_some()
            && self
                .operation
                .as_ref()
                .is_some_and(|snapshot| snapshot.state.is_none());
        let mut panel = div().flex_1().min_h_0().flex().flex_col().child(
            div()
                .px_3()
                .py_2()
                .flex()
                .flex_col()
                .gap_2()
                .child(div().h(px(40.)).child(self.branch_input.clone()))
                .child(
                    button("create-branch", self.t("create-branch"), false)
                        .opacity(if ready { 1. } else { 0.5 })
                        .on_click(cx.listener(|app, _, _, cx| app.create_branch(cx))),
                ),
        );
        if let Some(snapshot) = &self.branches {
            panel = panel.child(
                uniform_list(
                    "branches",
                    snapshot.branches.len(),
                    cx.processor(|app, range: std::ops::Range<usize>, _, cx| {
                        range
                            .map(|index| app.branch_row(index, cx))
                            .collect::<Vec<_>>()
                    }),
                )
                .flex_1()
                .min_h_0()
                .track_scroll(self.branch_scroll.clone()),
            );
        }
        if let Some(branch) = &self.selected_branch {
            let switch = branch.clone();
            let merge = branch.clone();
            let rebase = branch.clone();
            panel = panel.child(
                div()
                    .p_3()
                    .border_t_1()
                    .border_color(rgb(LINE))
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(div().text_sm().text_ellipsis().child(branch.name.clone()))
                    .child(
                        div()
                            .flex()
                            .gap_1()
                            .child(
                                button("switch-branch", self.t("switch-branch"), false)
                                    .opacity(if ready { 1. } else { 0.5 })
                                    .on_click(cx.listener(move |app, _, _, cx| {
                                        app.perform_operation(
                                            OperationAction::Switch(switch.clone()),
                                            cx,
                                        )
                                    })),
                            )
                            .child(
                                button("merge-branch", self.t("merge-branch"), false)
                                    .opacity(if ready { 1. } else { 0.5 })
                                    .on_click(cx.listener(move |app, _, _, cx| {
                                        app.perform_operation(
                                            OperationAction::Merge(merge.clone()),
                                            cx,
                                        )
                                    })),
                            )
                            .child(
                                button("rebase-branch", self.t("rebase-branch"), false)
                                    .opacity(if ready { 1. } else { 0.5 })
                                    .on_click(cx.listener(move |app, _, _, cx| {
                                        app.perform_operation(
                                            OperationAction::Rebase(rebase.clone()),
                                            cx,
                                        )
                                    })),
                            ),
                    ),
            );
        }
        panel
    }
    /// Presents recovered operation progress and separate continue/abort/skip controls in the shared workspace.
    fn operation_bar(&self, cx: &Context<Self>) -> Option<gpui::Div> {
        let state = self.operation.as_ref()?.state.as_ref()?;
        let ready = !self.writing && !self.scanning;
        let result_ready = self.conflict.as_ref().is_none_or(|view| {
            let view = view.read(cx);
            !view.dirty(cx) && !view.draft.awaiting_refresh && view.snapshot.result_staged
        });
        let unresolved = self
            .view
            .repository
            .as_ref()
            .is_some_and(|repo| repo.files.iter().any(|file| file.conflicted));
        let supported = matches!(state.kind, Operation::Merge | Operation::Rebase);
        let label = self.t(match state.kind {
            Operation::Merge => "operation-merge",
            Operation::Rebase => "operation-rebase",
            Operation::Other => "operation-other",
        });
        let mut bar = div()
            .px_3()
            .py_2()
            .flex()
            .items_center()
            .gap_2()
            .bg(rgb(PANEL))
            .border_b_1()
            .border_color(rgb(LINE))
            .child(div().text_sm().text_color(rgb(0xe2bb77)).child(label));
        if let Some((current, total)) = state.progress {
            bar = bar.child(
                div()
                    .text_sm()
                    .text_color(rgb(MUTED))
                    .child(format!("{current}/{total}")),
            );
        }
        if let Some(branch) = &state.original_branch {
            bar = bar.child(
                div()
                    .text_sm()
                    .min_w_0()
                    .text_ellipsis()
                    .child(branch.clone()),
            );
        }
        bar = bar.child(div().flex_1());
        if supported {
            bar = bar
                .child(
                    button("continue-operation", self.t("continue-operation"), false)
                        .opacity(if ready && !unresolved && result_ready {
                            1.
                        } else {
                            0.5
                        })
                        .on_click(cx.listener(|app, _, _, cx| {
                            app.perform_operation(OperationAction::Continue, cx)
                        })),
                )
                .child(
                    button("abort-operation", self.t("abort-operation"), false)
                        .opacity(if ready { 1. } else { 0.5 })
                        .on_click(cx.listener(|app, _, _, cx| {
                            app.perform_operation(OperationAction::Abort, cx)
                        })),
                );
            if state.kind == Operation::Rebase {
                bar = bar.child(
                    button("skip-operation", self.t("skip-operation"), false)
                        .opacity(if ready { 1. } else { 0.5 })
                        .on_click(cx.listener(|app, _, _, cx| {
                            app.perform_operation(OperationAction::Skip, cx)
                        })),
                );
            }
        }
        Some(bar)
    }
    /// Displays correctly labeled comparison sources and a virtual shared diff surface.
    fn diff_panel(&self, cx: &Context<Self>) -> gpui::Div {
        if let Some(conflict) = &self.conflict
            && self.view.selected.as_ref() == Some(&conflict.read(cx).snapshot.path)
        {
            return div().flex_1().min_h_0().min_w_0().child(conflict.clone());
        }
        let mut panel = div().flex_1().min_w_0().flex().flex_col().overflow_hidden();
        let header = div()
            .p_3()
            .flex()
            .items_center()
            .justify_between()
            .gap_3()
            .border_b_1()
            .border_color(rgb(LINE))
            .child(
                div().flex_1().min_w_0().text_ellipsis().child(
                    self.view
                        .selected
                        .as_ref()
                        .map(|path| path.to_string_lossy().into_owned())
                        .unwrap_or_else(|| self.t("select-file")),
                ),
            )
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child(
                        button(
                            "unstaged",
                            self.t("unstaged"),
                            self.mode == DiffKind::Unstaged,
                        )
                        .on_click(
                            cx.listener(|app, _, _, cx| app.set_mode(DiffKind::Unstaged, cx)),
                        ),
                    )
                    .child(
                        button("staged", self.t("staged"), self.mode == DiffKind::Staged).on_click(
                            cx.listener(|app, _, _, cx| app.set_mode(DiffKind::Staged, cx)),
                        ),
                    ),
            );
        panel = panel.child(header);
        let Some(diff) = &self.view.diff else {
            return panel.child(div().p_8().text_color(rgb(MUTED)).child(self.t(
                if self.diff_loading {
                    "loading-diff"
                } else {
                    "select-file"
                },
            )));
        };
        let selected_version = diff.clone();
        let hunk_version = diff.clone();
        let file_version = diff.clone();
        panel = panel.child(
            div()
                .px_3()
                .py_2()
                .flex()
                .gap_2()
                .items_center()
                .border_b_1()
                .border_color(rgb(LINE))
                .child(
                    button(
                        "write-selected",
                        self.t(if self.writing {
                            "write-busy"
                        } else if self.mode == DiffKind::Unstaged {
                            "stage-selection"
                        } else {
                            "unstage-selection"
                        }),
                        false,
                    )
                    .on_click(cx.listener(move |app, _, _, cx| {
                        if app.view.is_current_diff(&selected_version) {
                            app.write_selected(cx);
                        }
                    })),
                )
                .child(
                    button(
                        "write-hunk",
                        self.t(if self.mode == DiffKind::Unstaged {
                            "stage-hunk"
                        } else {
                            "unstage-hunk"
                        }),
                        false,
                    )
                    .on_click(cx.listener(move |app, _, _, cx| {
                        if app.view.is_current_diff(&hunk_version) {
                            app.write_hunk(cx);
                        }
                    })),
                )
                .child(
                    button(
                        "write-file",
                        self.t(if self.mode == DiffKind::Unstaged {
                            "stage-file"
                        } else {
                            "unstage-file"
                        }),
                        false,
                    )
                    .on_click(cx.listener(move |app, _, _, cx| {
                        if app.view.is_current_diff(&file_version) {
                            app.write_index(Selection::File, cx);
                        }
                    })),
                ),
        );
        let labels =
            div()
                .h(px(34.))
                .flex()
                .bg(rgb(PANEL))
                .border_b_1()
                .border_color(rgb(LINE))
                .text_sm()
                .text_color(rgb(MUTED))
                .child(div().flex_1().px_4().py_2().child(self.t(
                    if self.mode == DiffKind::Staged {
                        "head"
                    } else {
                        "index"
                    },
                )))
                .child(div().flex_1().px_4().py_2().child(self.t(
                    if self.mode == DiffKind::Staged {
                        "index"
                    } else {
                        "worktree"
                    },
                )));
        panel = panel.child(labels);
        let limitation = match diff.presentation {
            Presentation::Binary => Some("binary"),
            Presentation::InvalidEncoding => Some("encoding"),
            Presentation::TooLarge => Some("large"),
            Presentation::Conflict => Some("conflict"),
            Presentation::Unsupported => Some("unsupported"),
            Presentation::Text => None,
        };
        if let Some(key) = limitation {
            panel = panel.child(div().p_8().text_color(rgb(0xe1be86)).child(self.t(key)));
        } else if diff.rows.is_empty() {
            panel = panel.child(
                div()
                    .p_8()
                    .text_color(rgb(MUTED))
                    .child(self.t("no-text-changes")),
            );
        } else {
            panel = panel.child(
                uniform_list(
                    "diff-rows",
                    diff.rows.len(),
                    cx.processor(|app, range: std::ops::Range<usize>, _, cx| {
                        range
                            .map(|index| app.diff_row(index, cx))
                            .collect::<Vec<_>>()
                    }),
                )
                .flex_1()
                .min_h_0()
                .track_scroll(self.diff_scroll.clone()),
            );
        }
        let mode = |document: &hunkra_core::Document| {
            document
                .mode
                .map(|mode| format!("{mode:06o}"))
                .unwrap_or_else(|| self.t("absent"))
        };
        let mut footer = div()
            .p_3()
            .flex()
            .flex_col()
            .gap_2()
            .border_t_1()
            .border_color(rgb(LINE))
            .text_sm()
            .text_color(rgb(MUTED))
            .child(format!(
                "{}: {} → {}    ·    {} / {} B",
                self.t("mode"),
                mode(&diff.left),
                mode(&diff.right),
                diff.left.size,
                diff.right.size
            ));
        if self.mode == DiffKind::Unstaged {
            footer = footer.child(self.t("normalized"));
        }
        footer = footer.child(
            div()
                .flex()
                .items_center()
                .gap_3()
                .child(
                    button("previous", format!("↑  {}", self.t("previous-hunk")), false)
                        .on_click(cx.listener(|app, _, _, cx| app.move_hunk(false, cx))),
                )
                .child(
                    button("next", format!("↓  {}", self.t("next-hunk")), false)
                        .on_click(cx.listener(|app, _, _, cx| app.move_hunk(true, cx))),
                )
                .child(format!(
                    "{} / {}",
                    if diff.hunks.is_empty() {
                        0
                    } else {
                        self.hunk + 1
                    },
                    diff.hunks.len()
                ))
                .child(format!("∅  {}", self.t("no-newline"))),
        );
        panel.child(footer.child(self.t("select-lines")))
    }
}

impl Render for Hunkra {
    /// Renders native elements from existing snapshots; no blocking Git work occurs here.
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.frames += 1;
        self.smoke_input(window, cx);
        self.reviewed_operation = self.operation.clone();
        self.reviewed_commit = self
            .view
            .repository
            .as_ref()
            .map(|repo| repo.commit.clone());
        let mut content = div()
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(BG))
            .text_color(rgb(TEXT))
            .font_family("DejaVu Sans")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::key_down));
        let mut toolbar = div()
            .px_3()
            .h(px(42.))
            .flex_none()
            .flex()
            .items_center()
            .gap_3()
            .border_b_1()
            .border_color(rgb(LINE))
            .child(
                div()
                    .text_sm()
                    .font_weight(gpui::FontWeight::BOLD)
                    .text_color(rgb(ACCENT))
                    .child("Hunkra"),
            )
            .child(
                button("open", self.t("open"), false)
                    .on_click(cx.listener(|app, _, _, cx| app.choose_repository(cx))),
            )
            .child(
                button(
                    "refresh",
                    self.t(if self.scanning { "loading" } else { "refresh" }),
                    false,
                )
                .on_click(cx.listener(|app, _, _, cx| {
                    if app.scanning {
                        app.refresh_queued = true;
                    } else {
                        app.refresh(cx);
                    }
                })),
            )
            .child(
                button(
                    "commit",
                    self.t(if self.writing { "write-busy" } else { "commit" }),
                    false,
                )
                .opacity(
                    if self.writing
                        || self.scanning
                        || self.diff_loading
                        || self.reviewed_commit.is_none()
                    {
                        0.5
                    } else {
                        1.
                    },
                )
                .on_click(cx.listener(|app, _, _, cx| app.commit(cx))),
            )
            .child(div().flex_1());
        for (index, locale) in [Locale::En, Locale::Ru, Locale::RuDev]
            .into_iter()
            .enumerate()
        {
            toolbar = toolbar.child(
                button(
                    ("locale", index),
                    locale.label().to_owned(),
                    self.view.locale == locale,
                )
                .on_click(cx.listener(move |app, _, _, cx| app.set_locale(locale, cx))),
            );
        }
        content = content.child(toolbar);
        if let Some(prompt) = self.confirmation_panel(cx) {
            content = content.child(prompt);
        }
        if let Some(error) = self.write_error_key.or(self.error_key) {
            content = content.child(
                div()
                    .p_3()
                    .bg(rgb(0x422b2a))
                    .text_color(rgb(0xf5b3a4))
                    .child(self.t(error)),
            );
        }
        if let Some(bar) = self.operation_bar(cx) {
            content = content.child(bar);
        }
        if self.root.is_some() {
            content = content.child(
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .overflow_hidden()
                    .child(self.sidebar(cx))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(self.diff_panel(cx))
                            .child(
                                div()
                                    .h(px(138.))
                                    .flex_none()
                                    .p_3()
                                    .flex()
                                    .flex_col()
                                    .gap_2()
                                    .border_t_1()
                                    .border_color(rgb(LINE))
                                    .child(
                                        div()
                                            .text_sm()
                                            .text_color(rgb(MUTED))
                                            .child(self.t("commit-hint")),
                                    )
                                    .child(div().flex_1().min_h_0().child(self.message.clone())),
                            ),
                    ),
            );
        } else {
            let mut welcome = div()
                .flex_1()
                .p_8()
                .flex()
                .flex_col()
                .gap_4()
                .justify_center()
                .child(div().text_3xl().child(self.t("welcome")))
                .child(div().text_color(rgb(MUTED)).child(self.t("welcome-detail")));
            for (index, path) in self.settings.recent.iter().cloned().enumerate() {
                welcome = welcome.child(
                    button(
                        ("recent", index),
                        path.to_string_lossy().into_owned(),
                        false,
                    )
                    .on_click(cx.listener(move |app, _, _, cx| app.open(path.clone(), cx))),
                );
            }
            content = content.child(welcome);
        }
        if let Some(commit) = &self.committed {
            content = content.child(
                div()
                    .px_4()
                    .py_2()
                    .text_sm()
                    .text_color(rgb(ACCENT))
                    .child(format!("{}: {}", self.t("commit-created"), commit.id)),
            );
            if commit.post_hook_failed {
                content = content.child(
                    div()
                        .px_4()
                        .py_2()
                        .text_sm()
                        .text_color(rgb(0xe1be86))
                        .child(self.t("commit-post-warning")),
                );
            }
            if commit.reference_warning {
                content = content.child(
                    div()
                        .px_4()
                        .py_2()
                        .text_sm()
                        .text_color(rgb(0xe1be86))
                        .child(self.t("commit-reference-warning")),
                );
            }
        }
        if self
            .branch_created
            .as_ref()
            .is_some_and(|branch| branch.post_hook_failed || branch.reference_warning)
        {
            content = content.child(
                div()
                    .px_3()
                    .py_2()
                    .text_sm()
                    .text_color(rgb(0xe2bb77))
                    .child(self.t("branch-warning")),
            );
        }
        let mut status = div()
            .h(px(26.))
            .flex_none()
            .px_3()
            .flex()
            .items_center()
            .gap_3()
            .bg(rgb(PANEL))
            .border_t_1()
            .border_color(rgb(LINE))
            .text_xs()
            .text_color(rgb(MUTED));
        if let Some(repo) = &self.view.repository {
            status = status
                .child(format!(
                    "⑂ {}",
                    repo.branch.clone().unwrap_or_else(|| self.t("detached"))
                ))
                .child(self.translator.count(repo.files.len()));
        }
        if let Some(branch) = &self.branch_created {
            status = status.child(format!(
                "{}: {}",
                self.t("branch-created"),
                branch.branch.name
            ));
        } else if let Some(operation) = &self.operation_written {
            status = status.child(self.t(match operation.outcome {
                OperationOutcome::Completed => "operation-completed",
                OperationOutcome::StoppedWithConflicts => "operation-conflicts",
                OperationOutcome::StoppedForReview => "operation-review",
            }));
        } else if self.writing || self.scanning {
            status = status.child(self.t(if self.writing {
                "write-busy"
            } else {
                "loading"
            }));
        }
        status = status.child(div().flex_1()).child(self.t("view-only"));
        content.child(status)
    }
}

/// Starts a normal desktop window, or a finite native smoke test with an explicit report.
fn main() {
    let mut args = std::env::args_os().skip(1);
    let mut root = None;
    let mut smoke = None;
    let mut smoke_options = SmokeOptions::default();
    while let Some(arg) = args.next() {
        if arg == "--smoke-test" {
            smoke = args.next().map(PathBuf::from);
        } else if arg == "--smoke-stage" {
            smoke_options.stage = true;
        } else if arg == "--smoke-close" {
            smoke_options.close = true;
        } else if arg == "--smoke-refusal" {
            smoke_options.refusal = true;
        } else if arg == "--smoke-commit" {
            smoke_options.commit = true;
        } else if arg == "--smoke-operation" {
            smoke_options.operation = args
                .next()
                .map(|value| value.to_string_lossy().into_owned());
        } else if arg == "--smoke-hook" {
            smoke_options.hook = true;
        } else {
            root = Some(PathBuf::from(arg));
        }
    }
    Application::new().run(move |cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(1280.), px(820.)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions {
                    title: Some(SharedString::from("Hunkra")),
                    ..Default::default()
                }),
                ..Default::default()
            },
            move |window, cx| {
                cx.new(|cx| {
                    let app = Hunkra::new(root, smoke, smoke_options, cx);
                    let entity = cx.entity().downgrade();
                    window.on_window_should_close(cx, move |_, cx| {
                        entity
                            .update(cx, |app, cx| app.request_close(cx))
                            .unwrap_or(true)
                    });
                    window.focus(&app.focus);
                    app
                })
            },
        )
        .expect("open native Hunkra window");
        cx.activate(true);
        cx.on_window_closed(|cx| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();
    });
}
