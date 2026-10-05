//! Native GPUI shell. Blocking repository reads run in separate bounded workers.
use gpui::{
    App, Application, Bounds, Context, FocusHandle, KeyDownEvent, PathPromptOptions,
    ScrollStrategy, SharedString, Task, TitlebarOptions, UniformListScrollHandle, Window,
    WindowBounds, WindowOptions, div, prelude::*, px, rgb, size, uniform_list,
};
use hunkra::{
    i18n::{Locale, Translator},
    settings::{self, Settings},
    state::ViewState,
    worker::LatestWorker,
};
use hunkra_core::{
    ChangeKind, CoreError, DiffKind, DiffRow, DiffSnapshot, Line, Operation, Presentation,
    RepositorySnapshot,
};
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};

const BG: u32 = 0x101419;
const PANEL: u32 = 0x181e25;
const LINE: u32 = 0x2b3541;
const TEXT: u32 = 0xdce3ed;
const MUTED: u32 = 0x99a8b9;
const ACCENT: u32 = 0x93c9bd;

type DiffQuery = (PathBuf, PathBuf, DiffKind);

struct Smoke {
    report: PathBuf,
    phase: usize,
    started: Instant,
    last_frame: usize,
    locales: Vec<String>,
}
struct Hunkra {
    view: ViewState,
    translator: Translator,
    settings: Settings,
    status_worker: LatestWorker<PathBuf, Result<RepositorySnapshot, CoreError>>,
    diff_worker: LatestWorker<DiffQuery, Result<DiffSnapshot, CoreError>>,
    settings_worker: LatestWorker<Settings, std::io::Result<()>>,
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
    }
}

/// Builds consistent native buttons; callers supply their own action listener.
fn button(id: impl Into<gpui::ElementId>, text: String, active: bool) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .px_3()
        .py_2()
        .rounded_md()
        .text_sm()
        .cursor_pointer()
        .bg(rgb(if active { 0x304940 } else { 0x252e38 }))
        .text_color(rgb(if active { ACCENT } else { TEXT }))
        .hover(|style| style.bg(rgb(0x35414e)))
        .child(text)
}

impl Hunkra {
    /// Creates the standalone host and starts only the requested repository read.
    fn new(root: Option<PathBuf>, smoke: Option<PathBuf>, cx: &mut Context<Self>) -> Self {
        let settings = settings::load();
        let locale = settings.locale;
        let mut app = Self {
            view: ViewState {
                locale,
                ..Default::default()
            },
            translator: Translator::new(locale),
            settings,
            status_worker: LatestWorker::new("hunkra-status", |root: PathBuf| {
                hunkra_core::scan(&root)
            }),
            diff_worker: LatestWorker::new("hunkra-diff", |(root, path, kind): DiffQuery| {
                hunkra_core::read_diff(&root, &path, kind)
            }),
            settings_worker: LatestWorker::new("hunkra-settings", settings::save),
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
    /// Switches a profile without issuing repository requests or replacing diff state.
    fn set_locale(&mut self, locale: Locale, cx: &mut Context<Self>) {
        self.view.switch_locale(locale);
        self.translator = Translator::new(locale);
        self.settings.locale = locale;
        if self.smoke.is_none() {
            self.settings_worker.submit(0, self.settings.clone());
        }
        cx.notify();
    }
    /// Opens a path by generation, invalidating pending replies for the previous repository.
    fn open(&mut self, root: PathBuf, cx: &mut Context<Self>) {
        self.root = Some(root);
        self.view.repository = None;
        self.view.selected = None;
        self.view.diff = None;
        self.view.selected_rows.clear();
        self.view.diff_generation += 1;
        self.diff_loading = false;
        self.error_key = None;
        self.refresh_queued = false;
        self.refresh(cx);
    }
    /// Coalesces manual refresh requests while a status scan is active.
    fn refresh(&mut self, cx: &mut Context<Self>) {
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
            self.diff_scroll.scroll_to_item(0, ScrollStrategy::Top);
        }
        self.diff_worker.submit(generation, (root, path, self.mode));
        cx.notify();
    }
    /// Changes the comparison explicitly; HEAD→worktree is never used for either tab.
    fn set_mode(&mut self, mode: DiffKind, cx: &mut Context<Self>) {
        if self.mode == mode {
            return;
        }
        self.mode = mode;
        if let Some(path) = self.view.selected.clone() {
            self.view.diff = None;
            self.view.selected_rows.clear();
            self.select(path, cx);
        }
        cx.notify();
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
                Ok(snapshot) => {
                    self.error_key = None;
                    self.root = Some(snapshot.root.clone());
                    let selected = self
                        .view
                        .selected
                        .clone()
                        .filter(|path| snapshot.files.iter().any(|file| &file.path == path))
                        .or_else(|| snapshot.files.first().map(|file| file.path.clone()));
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
                Ok(diff) => {
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
        if self.smoke.is_some() {
            self.smoke_tick(cx);
        } else if !self.scanning && self.refresh_queued {
            self.refresh_queued = false;
            self.refresh(cx);
        } else if self.root.is_some() && self.last_scan.elapsed() > Duration::from_secs(2) {
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
        if self.scanning
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
            let generation = self.view.diff_generation;
            let selected = self.view.selected.clone();
            let diff = self.view.diff.clone();
            self.set_locale(locale, cx);
            assert_eq!(self.view.diff_generation, generation);
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
        } else {
            let smoke = self.smoke.as_ref().unwrap();
            let diff = self.view.diff.as_ref().unwrap();
            let report = serde_json::json!({"ok":self.frames>0&&diff.presentation==Presentation::Text&&!diff.hunks.is_empty(),
                "frames":self.frames,"file":diff.path,"locales":smoke.locales,"rows":diff.rows.len(),"hunks":diff.hunks.len(),
                "mode":"index-to-worktree","backend":diff.backend,"native_display":true});
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
            .h(px(44.))
            .px_3()
            .flex()
            .items_center()
            .gap_2()
            .cursor_pointer()
            .overflow_hidden()
            .bg(rgb(if active { 0x2a383c } else { PANEL }))
            .border_l_2()
            .border_color(rgb(if active { ACCENT } else { PANEL }))
            .hover(|style| style.bg(rgb(0x24303a)))
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
    /// Renders one aligned pair: signs, line numbers, newline markers and shared selection.
    fn diff_row(&self, index: usize, cx: &Context<Self>) -> gpui::Stateful<gpui::Div> {
        let row = &self.view.diff.as_ref().unwrap().rows[index];
        let selected = self.view.selected_rows.contains(&index);
        div()
            .id(("diff", index))
            .h(px(25.))
            .flex()
            .w_full()
            .overflow_hidden()
            .bg(rgb(if selected { 0x344b52 } else { BG }))
            .child(self.diff_cell(&row.left, row, true))
            .child(self.diff_cell(&row.right, row, false))
            .on_click(cx.listener(move |app, _, _, cx| {
                if !app.view.selected_rows.insert(index) {
                    app.view.selected_rows.remove(&index);
                }
                cx.notify();
            }))
    }
    /// Keeps an empty side as an aligned blank; display text never changes document bytes.
    fn diff_cell(&self, line: &Option<Line>, row: &DiffRow, left: bool) -> gpui::Div {
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
        let mut cell = div()
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
            .bg(rgb(if changed {
                if left { 0x342326 } else { 0x18302a }
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
        let mut info = div()
            .p_4()
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
    /// Displays correctly labeled comparison sources and a virtual shared diff surface.
    fn diff_panel(&self, cx: &Context<Self>) -> gpui::Div {
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
        panel.child(footer)
    }
}

impl Render for Hunkra {
    /// Renders native elements from existing snapshots; no blocking Git work occurs here.
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.frames += 1;
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
            .p_3()
            .h(px(65.))
            .flex_none()
            .flex()
            .items_center()
            .gap_3()
            .border_b_1()
            .border_color(rgb(LINE))
            .child(
                div()
                    .text_xl()
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
                div()
                    .bg(rgb(PANEL))
                    .text_color(rgb(MUTED))
                    .rounded_md()
                    .px_3()
                    .py_2()
                    .text_sm()
                    .child(self.t("commit")),
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
        if let Some(error) = self.error_key {
            content = content.child(
                div()
                    .p_3()
                    .bg(rgb(0x422b2a))
                    .text_color(rgb(0xf5b3a4))
                    .child(self.t(error)),
            );
        }
        if self.root.is_some() {
            content = content.child(
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .overflow_hidden()
                    .child(self.sidebar(cx))
                    .child(self.diff_panel(cx)),
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
        content.child(
            div()
                .h(px(30.))
                .flex_none()
                .px_4()
                .flex()
                .items_center()
                .border_t_1()
                .border_color(rgb(LINE))
                .text_sm()
                .text_color(rgb(MUTED))
                .child(format!(
                    "{}  ·  {}",
                    self.t("view-only"),
                    self.t("write-unavailable")
                )),
        )
    }
}

/// Starts a normal desktop window, or a finite native smoke test with an explicit report.
fn main() {
    let mut args = std::env::args_os().skip(1);
    let mut root = None;
    let mut smoke = None;
    while let Some(arg) = args.next() {
        if arg == "--smoke-test" {
            smoke = args.next().map(PathBuf::from);
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
                    let app = Hunkra::new(root, smoke, cx);
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
