//! Native immutable history panels; one bounded worker owns traversal, commit and selected diff reads.
use crate::{
    editor::NativeEditor,
    history_state::{HistoryCache, HistoryService},
    horizontal_diff::HorizontalDiff,
    i18n::{Locale, Translator},
    theme,
    worker::LatestWorker,
};
use fluent_bundle::FluentArgs;
use gpui::{
    App, Context, Entity, FocusHandle, Focusable, KeyDownEvent, ScrollStrategy,
    UniformListScrollHandle, Window, div, prelude::*, px, rgb, uniform_list,
};
use hunkra_core::{
    CommitBase, CommitDetails, CommitSummary, CoreError, DiffRow, DiffSnapshot, HistoryPage, Line,
    Presentation,
};
use std::{path::PathBuf, sync::Arc};

enum Query {
    Page {
        root: PathBuf,
        session: u64,
        offset: usize,
    },
    Commit {
        root: PathBuf,
        id: String,
        base: CommitBase,
    },
    Diff(Arc<CommitDetails>, PathBuf),
}
enum Read {
    Page(Arc<HistoryPage>),
    Commit(Arc<CommitDetails>),
    Diff(Arc<DiffSnapshot>),
}

pub struct HistoryView {
    root: PathBuf,
    translator: Translator,
    worker: LatestWorker<Query, Result<Read, CoreError>>,
    session: u64,
    pub generation: u64,
    pub cache: HistoryCache,
    pub selected: Option<CommitSummary>,
    pub details: Option<Arc<CommitDetails>>,
    pub selected_file: Option<PathBuf>,
    pub diff: Option<Arc<DiffSnapshot>>,
    pub loading: bool,
    pub error: Option<&'static str>,
    pub message: Entity<NativeEditor>,
    message_id: Option<String>,
    commit_scroll: UniformListScrollHandle,
    file_scroll: UniformListScrollHandle,
    diff_scroll: UniformListScrollHandle,
    pub horizontal: HorizontalDiff,
    focus: FocusHandle,
    hunk: usize,
    pub frames: usize,
}

/// Builds compact owned tabs/buttons with the same palette as the standalone workspace.
fn button(
    id: impl Into<gpui::ElementId>,
    text: String,
    active: bool,
    enabled: bool,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .px_2()
        .py_1()
        .rounded_sm()
        .cursor_pointer()
        .bg(rgb(if active {
            theme::SELECTED
        } else {
            theme::PANEL
        }))
        .text_color(rgb(if active { theme::ACCENT } else { theme::TEXT }))
        .opacity(if enabled { 1. } else { 0.5 })
        .hover(|style| style.bg(rgb(theme::HOVER)))
        .child(text)
}
/// Maps owned errors to bundled UI explanations without showing raw Git/user diagnostics.
fn error_key(error: &CoreError) -> &'static str {
    match error {
        CoreError::HistoryLimit | CoreError::TooManyFiles => "error-history-limit",
        CoreError::HistoryBase => "error-history-base",
        CoreError::HistoryRequest | CoreError::InvalidPath => "error-history-request",
        CoreError::Io(_) => "error-io",
        _ => "error-git",
    }
}

impl HistoryView {
    /// Creates its thread-confined gix service inside the bounded worker, then requests only the first page.
    pub fn new(root: PathBuf, locale: Locale, cx: &mut Context<Self>) -> Self {
        let translator = Translator::new(locale);
        let message = cx.new(|cx| {
            let mut editor =
                NativeEditor::new(1024 * 1024, translator.text("history-encoding", None), cx);
            editor.enabled = false;
            editor
        });
        let worker = LatestWorker::with_state(
            "hunkra-history",
            HistoryService::default,
            |service, query| match query {
                Query::Page {
                    root,
                    session,
                    offset,
                } => service.page(&root, session, offset).map(Read::Page),
                Query::Commit { root, id, base } => hunkra_core::read_commit(&root, &id, base)
                    .map(|detail| Read::Commit(Arc::new(detail))),
                Query::Diff(detail, path) => hunkra_core::read_commit_diff(&detail, &path)
                    .map(|diff| Read::Diff(Arc::new(diff))),
            },
        );
        let mut view = Self {
            root,
            translator,
            worker,
            session: 0,
            generation: 0,
            cache: HistoryCache::default(),
            selected: None,
            details: None,
            selected_file: None,
            diff: None,
            loading: false,
            error: None,
            message,
            message_id: None,
            commit_scroll: UniformListScrollHandle::new(),
            file_scroll: UniformListScrollHandle::new(),
            diff_scroll: UniformListScrollHandle::new(),
            horizontal: HorizontalDiff::default(),
            focus: cx.focus_handle(),
            hunk: 0,
            frames: 0,
        };
        view.reload(cx);
        view
    }
    /// Formats owned labels without reading repository data.
    fn t(&self, key: &str) -> String {
        self.translator.text(key, None)
    }
    /// Changes only labels; pinned page/commit/base/file/diff and read-only message selection stay intact.
    pub fn set_locale(&mut self, locale: Locale, cx: &mut Context<Self>) {
        self.translator = Translator::new(locale);
        let placeholder = self.t("history-encoding");
        self.message.update(cx, |editor, cx| {
            editor.placeholder = placeholder.into();
            cx.notify();
        });
        cx.notify();
    }
    /// Invalidates old replies before submitting the one latest history query.
    fn request(&mut self, query: Query, cx: &mut Context<Self>) {
        self.generation += 1;
        self.loading = true;
        self.error = None;
        self.worker.submit(self.generation, query);
        cx.notify();
    }
    /// Starts a fresh current-HEAD history session, never touching the host's Result/message buffers.
    pub fn reload(&mut self, cx: &mut Context<Self>) {
        self.session += 1;
        self.cache.clear();
        self.selected = None;
        self.details = None;
        self.selected_file = None;
        self.diff = None;
        self.commit_scroll.scroll_to_item(0, ScrollStrategy::Top);
        self.request(
            Query::Page {
                root: self.root.clone(),
                session: self.session,
                offset: 0,
            },
            cx,
        );
    }
    /// Selects an actual displayed immutable commit and names its initial parent/empty-tree base.
    pub fn select_commit(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(commit) = self
            .cache
            .current()
            .and_then(|page| page.commits.get(index))
            .cloned()
        else {
            return;
        };
        let base = if commit.parents.is_empty() {
            CommitBase::EmptyTree
        } else {
            CommitBase::Parent(0)
        };
        self.selected = Some(commit.clone());
        self.details = None;
        self.selected_file = None;
        self.diff = None;
        self.file_scroll.scroll_to_item(0, ScrollStrategy::Top);
        self.request(
            Query::Commit {
                root: self.root.clone(),
                id: commit.id,
                base,
            },
            cx,
        );
    }
    /// Changes the explicitly displayed merge parent, retaining read-only message selection.
    pub fn select_parent(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(commit) = self.selected.clone() else {
            return;
        };
        if index >= commit.parents.len() {
            return;
        }
        if self
            .details
            .as_ref()
            .is_some_and(|detail| detail.base == CommitBase::Parent(index))
        {
            return;
        }
        self.diff = None;
        self.selected_file = None;
        self.request(
            Query::Commit {
                root: self.root.clone(),
                id: commit.id,
                base: CommitBase::Parent(index),
            },
            cx,
        );
    }
    /// Requests only one changed historical path from the reviewed explicit-parent snapshot.
    pub fn select_file(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(detail) = self.details.clone() else {
            return;
        };
        let Some(file) = detail.files.get(index) else {
            return;
        };
        let path = file.path.clone();
        self.horizontal.reset();
        self.selected_file = Some(path.clone());
        self.diff = None;
        self.hunk = 0;
        self.diff_scroll.scroll_to_item(0, ScrollStrategy::Top);
        self.request(Query::Diff(detail, path), cx);
    }
    /// Moves backward only through retained pages; no duplicated traversal is launched.
    pub fn previous_page(&mut self, cx: &mut Context<Self>) {
        if !self.loading && self.cache.previous() {
            self.select_commit(0, cx);
        }
    }
    /// Uses a retained next page first, otherwise advances exactly the worker's next expected offset.
    pub fn next_page(&mut self, cx: &mut Context<Self>) {
        if self.loading {
            return;
        }
        if self.cache.next_cached() {
            self.select_commit(0, cx);
        } else if let Some(offset) = self.cache.next_offset() {
            self.request(
                Query::Page {
                    root: self.root.clone(),
                    session: self.session,
                    offset,
                },
                cx,
            );
        }
    }
    /// Applies matching replies only; dropped page replies can be rerequested without losing a cursor.
    pub fn poll(&mut self, cx: &mut Context<Self>) {
        let Some((generation, result)) = self.worker.take() else {
            return;
        };
        if generation != self.generation {
            return;
        }
        self.loading = false;
        match result {
            Ok(Read::Page(page)) => {
                if !self.cache.install(page) {
                    self.error = Some("error-history-request");
                } else {
                    self.commit_scroll.scroll_to_item(0, ScrollStrategy::Top);
                    self.select_commit(0, cx);
                }
            }
            Ok(Read::Commit(detail)) => {
                if self.selected.as_ref().map(|commit| &commit.id) != Some(&detail.summary.id) {
                    return;
                }
                if self.message_id.as_ref() != Some(&detail.summary.id) {
                    self.message.update(cx, |editor, cx| {
                        editor
                            .buffer
                            .reset(detail.message.clone().unwrap_or_default());
                        cx.notify();
                    });
                    self.message_id = Some(detail.summary.id.clone());
                }
                self.details = Some(detail);
                self.select_file(0, cx);
            }
            Ok(Read::Diff(diff)) => {
                if self.selected_file.as_ref() == Some(&diff.path) {
                    self.diff = Some(diff);
                }
            }
            Err(error) => self.error = Some(error_key(&error)),
        }
        cx.notify();
    }
    /// Handles commit navigation and historical hunk movement without reaching host write shortcuts.
    fn key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let key = event.keystroke.key.as_str();
        if event.keystroke.modifiers.alt && matches!(key, "left" | "right" | "home") {
            self.horizontal.shift(if key == "home" {
                -px(f32::MAX)
            } else if key == "left" {
                -px(160.)
            } else {
                px(160.)
            });
            cx.notify();
        } else if key == "f6" || key == "tab" && !event.keystroke.modifiers.control {
            let handle = if self.focus.is_focused(window) {
                self.message.read(cx).focus_handle(cx)
            } else {
                self.focus.clone()
            };
            window.focus(&handle);
        } else if event.keystroke.modifiers.control && matches!(key, "up" | "down") {
            if let Some(detail) = &self.details
                && !detail.files.is_empty()
            {
                let current = detail
                    .files
                    .iter()
                    .position(|file| Some(&file.path) == self.selected_file.as_ref())
                    .unwrap_or(0);
                let next = if key == "up" {
                    current.saturating_sub(1)
                } else {
                    (current + 1).min(detail.files.len() - 1)
                };
                self.file_scroll
                    .scroll_to_item(next, ScrollStrategy::Center);
                self.select_file(next, cx);
            }
        } else if matches!(key, "up" | "down") && !event.keystroke.modifiers.control {
            if let Some(page) = self.cache.current() {
                let current = self
                    .selected
                    .as_ref()
                    .and_then(|selected| {
                        page.commits
                            .iter()
                            .position(|commit| commit.id == selected.id)
                    })
                    .unwrap_or(0);
                let next = if key == "up" {
                    current.saturating_sub(1)
                } else {
                    (current + 1).min(page.commits.len().saturating_sub(1))
                };
                self.select_commit(next, cx);
                self.commit_scroll
                    .scroll_to_item(next, ScrollStrategy::Center);
            }
        } else if key == "f7" {
            self.move_hunk(!event.keystroke.modifiers.shift, cx);
        } else {
            return;
        }
        window.prevent_default();
        cx.stop_propagation();
    }
    /// Scrolls one aligned hunk while leaving the original historical blob documents unchanged.
    fn move_hunk(&mut self, forward: bool, cx: &mut Context<Self>) {
        let Some(diff) = &self.diff else { return };
        if diff.hunks.is_empty() {
            return;
        }
        self.hunk = if forward {
            (self.hunk + 1) % diff.hunks.len()
        } else {
            (self.hunk + diff.hunks.len() - 1) % diff.hunks.len()
        };
        self.diff_scroll
            .scroll_to_item(diff.hunks[self.hunk].start, ScrollStrategy::Center);
        cx.notify();
    }
    /// Renders one virtual commit row with source metadata and an immutable short id.
    fn commit_row(&self, index: usize, cx: &Context<Self>) -> gpui::Stateful<gpui::Div> {
        let commit = &self.cache.current().unwrap().commits[index];
        let selected = self
            .selected
            .as_ref()
            .is_some_and(|selected| selected.id == commit.id);
        div()
            .id(("history-commit", index))
            .h(px(54.))
            .px_3()
            .py_1()
            .flex()
            .flex_col()
            .gap_1()
            .overflow_hidden()
            .cursor_pointer()
            .bg(rgb(if selected {
                theme::SELECTED
            } else {
                theme::PANEL
            }))
            .child(
                div().text_sm().text_ellipsis().child(
                    commit
                        .subject
                        .clone()
                        .unwrap_or_else(|| self.t("history-encoding")),
                ),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(theme::MUTED))
                    .text_ellipsis()
                    .child(format!(
                        "{} · {}",
                        &commit.id[..10],
                        commit
                            .author
                            .clone()
                            .unwrap_or_else(|| self.t("history-encoding"))
                    )),
            )
            .on_click(cx.listener(move |view, _, window, cx| {
                window.focus(&view.focus);
                view.select_commit(index, cx);
            }))
    }
    /// Renders one immutable changed file with a non-color-only change marker.
    fn file_row(&self, index: usize, cx: &Context<Self>) -> gpui::Stateful<gpui::Div> {
        let file = &self.details.as_ref().unwrap().files[index];
        let active = self.selected_file.as_ref() == Some(&file.path);
        let mark = match file.change {
            hunkra_core::ChangeKind::Added => "+",
            hunkra_core::ChangeKind::Deleted => "−",
            hunkra_core::ChangeKind::TypeChanged => "T",
            _ => "M",
        };
        div()
            .id(("history-file", index))
            .h(px(28.))
            .px_2()
            .flex()
            .items_center()
            .gap_2()
            .cursor_pointer()
            .overflow_hidden()
            .bg(rgb(if active {
                theme::SELECTED
            } else {
                theme::PANEL
            }))
            .child(div().text_color(rgb(theme::ACCENT)).child(mark))
            .child(
                div()
                    .min_w_0()
                    .text_sm()
                    .text_ellipsis()
                    .child(file.path.to_string_lossy().into_owned()),
            )
            .on_click(cx.listener(move |view, _, window, cx| {
                window.focus(&view.focus);
                view.select_file(index, cx);
            }))
    }
    /// Displays one side of an aligned raw-blob row; line endings remain metadata, never normalized bytes.
    fn cell(
        &self,
        line: &Option<Line>,
        changed: bool,
        left: bool,
        window: &mut Window,
    ) -> gpui::Div {
        let mut cell = div()
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .items_center()
            .overflow_hidden()
            .font_family("DejaVu Sans Mono")
            .bg(rgb(if changed {
                if left { 0x382c30 } else { 0x283c31 }
            } else {
                theme::BACKGROUND
            }));
        if let Some(line) = line {
            cell = cell
                .child(
                    div()
                        .w(px(48.))
                        .flex_none()
                        .px_2()
                        .text_xs()
                        .text_color(rgb(theme::MUTED))
                        .child(line.number.to_string()),
                )
                .child(div().w(px(18.)).flex_none().child(if changed {
                    if left { "−" } else { "+" }
                } else {
                    " "
                }))
                .child(self.horizontal.text(
                    if line.ending == "EOF" {
                        format!("{}  ∅", line.text.replace('\t', "    "))
                    } else {
                        line.text.replace('\t', "    ")
                    },
                    window,
                ));
        }
        cell
    }
    /// Keeps additions/deletions aligned through one shared virtual row list.
    fn diff_row(
        &self,
        index: usize,
        row: &DiffRow,
        window: &mut Window,
        cx: &Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        div()
            .id(("history-diff-row", index))
            .h(px(25.))
            .w_full()
            .flex()
            .overflow_hidden()
            .child(self.cell(&row.left, row.changed && row.left.is_some(), true, window))
            .child(self.cell(
                &row.right,
                row.changed && row.right.is_some(),
                false,
                window,
            ))
            .on_scroll_wheel(cx.listener(|view, event, _, cx| {
                if view.horizontal.wheel(event) {
                    cx.stop_propagation();
                    cx.notify();
                }
            }))
    }
    /// Builds selected-commit details, explicit parent controls and a read-only aligned diff.
    fn detail_panel(&self, cx: &Context<Self>) -> gpui::Div {
        let mut panel = div().flex_1().min_w_0().min_h_0().flex().flex_col();
        let Some(detail) = &self.details else {
            return panel.child(div().p_4().text_color(rgb(theme::MUTED)).child(self.t(
                if self.loading {
                    "loading"
                } else {
                    "history-select"
                },
            )));
        };
        let mut header = div()
            .p_3()
            .flex_none()
            .flex()
            .flex_col()
            .gap_1()
            .border_b_1()
            .border_color(rgb(theme::BORDER))
            .child(
                div()
                    .text_xs()
                    .text_ellipsis()
                    .child(detail.summary.id.clone()),
            );
        let mut parents = div().flex().flex_wrap().gap_1().items_center().child(
            div()
                .text_xs()
                .text_color(rgb(theme::MUTED))
                .child(self.t("history-base")),
        );
        if detail.summary.parents.is_empty() {
            parents = parents.child(self.t("history-empty-tree"));
        }
        for index in 0..detail.summary.parents.len() {
            let mut args = FluentArgs::new();
            args.set("number", index + 1);
            parents = parents.child(
                button(
                    ("history-parent", index),
                    self.translator.text("history-parent", Some(&args)),
                    detail.base == CommitBase::Parent(index),
                    !self.loading,
                )
                .on_click(cx.listener(move |view, _, _, cx| {
                    if !view.loading {
                        view.select_parent(index, cx);
                    }
                })),
            );
        }
        header = header.child(parents);
        if let Some(id) = &detail.base_id {
            header = header.child(
                div()
                    .text_xs()
                    .text_color(rgb(theme::MUTED))
                    .text_ellipsis()
                    .child(id.clone()),
            );
        }
        if detail.message.is_none() {
            header = header.child(div().text_xs().text_color(rgb(theme::MUTED)).child(format!(
                    "{}{}",
                    self.t("history-encoding"),
                    detail
                        .encoding
                        .as_deref()
                        .map(|encoding| format!(" · {encoding}"))
                        .unwrap_or_default()
                )));
        }
        panel = panel
            .child(header)
            .child(div().h(px(90.)).flex_none().child(self.message.clone()));
        let mut files = div()
            .w(px(220.))
            .flex_none()
            .min_h_0()
            .flex()
            .flex_col()
            .bg(rgb(theme::PANEL))
            .border_r_1()
            .border_color(rgb(theme::BORDER))
            .child(
                div()
                    .px_2()
                    .py_2()
                    .text_xs()
                    .text_color(rgb(theme::MUTED))
                    .child(self.translator.count(detail.files.len())),
            );
        if !detail.files.is_empty() {
            files = files.child(
                uniform_list(
                    "history-files",
                    detail.files.len(),
                    cx.processor(|view, range: std::ops::Range<usize>, _, cx| {
                        range
                            .map(|index| view.file_row(index, cx))
                            .collect::<Vec<_>>()
                    }),
                )
                .flex_1()
                .min_h_0()
                .track_scroll(self.file_scroll.clone()),
            );
        }
        let mut diff_panel = div().flex_1().min_w_0().min_h_0().flex().flex_col();
        if let Some(diff) = &self.diff {
            diff_panel = diff_panel.child(
                div()
                    .h(px(32.))
                    .flex_none()
                    .px_2()
                    .flex()
                    .items_center()
                    .text_sm()
                    .text_ellipsis()
                    .child(diff.path.to_string_lossy().into_owned()),
            );
            diff_panel = diff_panel.child(
                div()
                    .h(px(28.))
                    .flex_none()
                    .flex()
                    .text_xs()
                    .text_color(rgb(theme::MUTED))
                    .child(
                        div().flex_1().px_2().child(
                            detail
                                .base_id
                                .as_deref()
                                .map(|id| format!("{} · {}", self.t("history-base"), &id[..10]))
                                .unwrap_or_else(|| self.t("history-empty-tree")),
                        ),
                    )
                    .child(div().flex_1().px_2().child(format!(
                        "{} · {}",
                        self.t("history-commit"),
                        &detail.summary.id[..10]
                    ))),
            );
            let limitation = match diff.presentation {
                Presentation::Text => None,
                Presentation::Binary => Some("binary"),
                Presentation::InvalidEncoding => Some("encoding"),
                Presentation::TooLarge => Some("large"),
                _ => Some("unsupported"),
            };
            if let Some(reason) = limitation {
                diff_panel = diff_panel.child(
                    div()
                        .p_4()
                        .text_color(rgb(theme::MUTED))
                        .child(self.t(reason)),
                );
            } else if diff.rows.is_empty() {
                diff_panel = diff_panel.child(
                    div()
                        .p_4()
                        .text_color(rgb(theme::MUTED))
                        .child(self.t("no-text-changes")),
                );
            } else {
                diff_panel = diff_panel.child(
                    uniform_list(
                        "history-diff",
                        diff.rows.len(),
                        cx.processor(|view, range: std::ops::Range<usize>, window, cx| {
                            range
                                .map(|index| {
                                    view.diff_row(
                                        index,
                                        &view.diff.as_ref().unwrap().rows[index],
                                        window,
                                        cx,
                                    )
                                })
                                .collect::<Vec<_>>()
                        }),
                    )
                    .flex_1()
                    .min_h_0()
                    .track_scroll(self.diff_scroll.clone()),
                );
            }
            diff_panel = diff_panel.child(
                div()
                    .px_2()
                    .py_1()
                    .flex()
                    .items_center()
                    .gap_2()
                    .text_xs()
                    .text_color(rgb(theme::MUTED))
                    .child(self.t("horizontal-hint"))
                    .child(
                        button("history-horizontal-left", "←".into(), false, true).on_click(
                            cx.listener(|view, _, _, cx| {
                                view.horizontal.shift(-px(160.));
                                cx.notify();
                            }),
                        ),
                    )
                    .child(
                        button("history-horizontal-right", "→".into(), false, true).on_click(
                            cx.listener(|view, _, _, cx| {
                                view.horizontal.shift(px(160.));
                                cx.notify();
                            }),
                        ),
                    )
                    .child(
                        button(
                            "history-horizontal-reset",
                            self.t("horizontal-reset"),
                            false,
                            true,
                        )
                        .on_click(cx.listener(|view, _, _, cx| {
                            view.horizontal.shift(-px(f32::MAX));
                            cx.notify();
                        })),
                    ),
            );
            let mode = |mode: Option<u32>| {
                mode.map(|mode| format!("{mode:06o}"))
                    .unwrap_or_else(|| self.t("absent"))
            };
            diff_panel = diff_panel.child(
                div()
                    .px_2()
                    .py_1()
                    .text_xs()
                    .text_color(rgb(theme::MUTED))
                    .child(format!(
                        "{}: {} → {} · {} / {} B",
                        self.t("mode"),
                        mode(diff.left.mode),
                        mode(diff.right.mode),
                        diff.left.size,
                        diff.right.size
                    )),
            );
        } else {
            diff_panel = diff_panel.child(div().p_4().text_color(rgb(theme::MUTED)).child(self.t(
                if self.loading {
                    "loading"
                } else {
                    "history-no-files"
                },
            )));
        }
        panel
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .child(files)
                    .child(diff_panel),
            )
            .child(
                div()
                    .px_2()
                    .py_1()
                    .text_xs()
                    .text_color(rgb(theme::MUTED))
                    .child(self.t("history-raw")),
            )
    }
}
impl Render for HistoryView {
    /// Renders only retained pages and selected metadata/diff; no repository work occurs in render.
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.frames += 1;
        let mut panel = div()
            .size_full()
            .min_h_0()
            .min_w_0()
            .flex()
            .flex_col()
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::key_down));
        if let Some(error) = self.error {
            panel = panel.child(div().p_2().text_color(rgb(0xe2bb77)).child(self.t(error)));
        }
        let mut sidebar = div()
            .w(px(300.))
            .flex_none()
            .min_h_0()
            .flex()
            .flex_col()
            .bg(rgb(theme::PANEL))
            .border_r_1()
            .border_color(rgb(theme::BORDER));
        let mut bar = div()
            .h(px(36.))
            .px_2()
            .flex_none()
            .flex()
            .gap_1()
            .items_center()
            .child(
                button(
                    "history-newer",
                    self.t("history-newer"),
                    false,
                    !self.loading && self.cache.can_previous(),
                )
                .on_click(cx.listener(|view, _, _, cx| view.previous_page(cx))),
            )
            .child(
                button(
                    "history-older",
                    self.t("history-older"),
                    false,
                    !self.loading && self.cache.can_next(),
                )
                .on_click(cx.listener(|view, _, _, cx| view.next_page(cx))),
            )
            .child(div().flex_1());
        if let Some(page) = self.cache.current() {
            let mut args = FluentArgs::new();
            args.set(
                "first",
                if page.commits.is_empty() {
                    0
                } else {
                    page.offset + 1
                },
            );
            args.set("last", page.offset + page.commits.len());
            bar = bar.child(
                div()
                    .text_xs()
                    .text_color(rgb(theme::MUTED))
                    .child(self.translator.text("history-range", Some(&args))),
            );
            sidebar = sidebar.child(bar).child(
                div()
                    .px_2()
                    .py_1()
                    .text_xs()
                    .text_ellipsis()
                    .text_color(rgb(theme::MUTED))
                    .child(page.tip.clone().unwrap_or_else(|| self.t("unborn"))),
            );
            if page.commits.is_empty() {
                sidebar = sidebar.child(
                    div()
                        .p_3()
                        .text_color(rgb(theme::MUTED))
                        .child(self.t("history-empty")),
                );
            } else {
                sidebar = sidebar.child(
                    uniform_list(
                        "history-commits",
                        page.commits.len(),
                        cx.processor(|view, range: std::ops::Range<usize>, _, cx| {
                            range
                                .map(|index| view.commit_row(index, cx))
                                .collect::<Vec<_>>()
                        }),
                    )
                    .flex_1()
                    .min_h_0()
                    .track_scroll(self.commit_scroll.clone()),
                );
            }
        } else {
            sidebar = sidebar.child(bar).child(
                div()
                    .p_3()
                    .text_color(rgb(theme::MUTED))
                    .child(self.t("loading")),
            );
        }
        panel.child(
            div()
                .flex_1()
                .min_h_0()
                .flex()
                .child(sidebar)
                .child(self.detail_panel(cx)),
        )
    }
}
impl Focusable for HistoryView {
    /// Gives native keyboard navigation to the owned history panel without an independent window.
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}
