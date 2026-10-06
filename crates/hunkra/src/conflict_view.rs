//! Owned native conflict panels; every Git/filesystem action goes through the host writer.
use crate::{
    editor::{NativeEditor, TextChanged, TextRejected},
    i18n::{Locale, Translator},
    result_draft::{DraftRefresh, ResultDraft},
    theme,
};
use fluent_bundle::FluentArgs;
use gpui::{
    App, Context, Entity, EventEmitter, Focusable, KeyDownEvent, ScrollStrategy, Subscription,
    UniformListScrollHandle, Window, div, prelude::*, px, rgb, uniform_list,
};
use hunkra_core::{
    ConflictSide, ConflictSnapshot, CoreError, Document, Operation, Presentation,
    conflict_chunks::{ChunkChoice, chunks},
    diff::MAX_BYTES,
};
use std::sync::Arc;

pub enum ConflictEvent {
    Save(Arc<ConflictSnapshot>, Document),
    Resolve(Arc<ConflictSnapshot>),
    Reload,
    Changed,
    Error(CoreError),
}

pub struct ConflictView {
    pub snapshot: Arc<ConflictSnapshot>,
    pub result: Entity<NativeEditor>,
    pub draft: ResultDraft,
    a: Entity<NativeEditor>,
    b: Entity<NativeEditor>,
    base: Entity<NativeEditor>,
    translator: Translator,
    show_base: bool,
    busy: bool,
    chunk: usize,
    chunk_scroll: UniformListScrollHandle,
    subscriptions: Vec<Subscription>,
}
impl EventEmitter<ConflictEvent> for ConflictView {}

/// Creates one read-only native surface without lossy conversion of Git or worktree bytes.
fn source_editor(side: &ConflictSide, notice: String, cx: &mut App) -> Entity<NativeEditor> {
    cx.new(|cx| {
        let mut editor = NativeEditor::new(MAX_BYTES, String::new(), notice, cx);
        editor.enabled = false;
        if let Ok(text) = std::str::from_utf8(&side.worktree.bytes) {
            editor.buffer.reset(text.to_owned());
        }
        editor
    })
}

/// Creates compact owned controls matching the application's existing panel style.
fn control(id: &'static str, text: String, active: bool, ready: bool) -> gpui::Stateful<gpui::Div> {
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
        .opacity(if ready { 1. } else { 0.5 })
        .hover(|style| style.bg(rgb(theme::HOVER)))
        .child(text)
}

impl ConflictView {
    /// Loads independently editable Result and immutable A/B/Base into owned GPUI entities.
    pub fn new(snapshot: Arc<ConflictSnapshot>, locale: Locale, cx: &mut Context<Self>) -> Self {
        let translator = Translator::new(locale);
        let a = source_editor(
            &snapshot.a,
            translator.text("editor-preview-limit", None),
            cx,
        );
        let b = source_editor(
            &snapshot.b,
            translator.text("editor-preview-limit", None),
            cx,
        );
        let base = source_editor(
            &snapshot.base,
            translator.text("editor-preview-limit", None),
            cx,
        );
        let result = cx.new(|cx| {
            let mut editor = NativeEditor::new(
                MAX_BYTES,
                translator.text("result-placeholder", None),
                translator.text("editor-preview-limit", None),
                cx,
            );
            editor.enabled = snapshot.presentation == Presentation::Text;
            if editor.enabled
                && let Ok(text) = std::str::from_utf8(&snapshot.result.bytes)
            {
                editor.buffer.reset(text.to_owned());
            }
            editor
        });
        let changed = cx.subscribe(&result, |view, _, _: &TextChanged, cx| {
            view.draft.present = true;
            view.chunk = 0;
            cx.emit(ConflictEvent::Changed);
            cx.notify();
        });
        let rejected = cx.subscribe(&result, |_, _, _: &TextRejected, cx| {
            cx.emit(ConflictEvent::Error(CoreError::ConflictUnsupported));
            cx.notify();
        });
        let draft = ResultDraft::new(
            snapshot.result.clone(),
            snapshot.a.worktree.mode.or(snapshot.b.worktree.mode),
        );
        Self {
            snapshot,
            result,
            draft,
            a,
            b,
            base,
            translator,
            show_base: false,
            busy: false,
            chunk: 0,
            chunk_scroll: UniformListScrollHandle::new(),
            subscriptions: vec![changed, rejected],
        }
    }
    /// Formats owned labels without issuing Git work or mutating editor buffers.
    fn t(&self, key: &str) -> String {
        self.translator.text(key, None)
    }
    /// Toggles only the read-only Base pane, leaving Result input and history untouched.
    pub fn toggle_base(&mut self, cx: &mut Context<Self>) {
        self.show_base = !self.show_base;
        cx.notify();
    }
    /// Reports an unsaved editable Result; unsupported source views cannot acquire a fake draft.
    pub fn dirty(&self, cx: &App) -> bool {
        self.snapshot.presentation == Presentation::Text
            && self.draft.dirty(self.result.read(cx).buffer.text())
    }
    /// Changes only labels and placeholders; every source/result selection and undo stack stays intact.
    pub fn set_locale(&mut self, locale: Locale, cx: &mut Context<Self>) {
        self.translator = Translator::new(locale);
        let placeholder = self.t("result-placeholder");
        self.result.update(cx, |editor, cx| {
            editor.placeholder = placeholder.into();
            cx.notify();
        });
        let notice = self.t("editor-preview-limit");
        for editor in [&self.result, &self.a, &self.b, &self.base] {
            editor.update(cx, |editor, cx| {
                editor.preview_notice = notice.clone().into();
                cx.notify();
            });
        }
        cx.notify();
    }
    /// Freezes edits while the host serializes an accepted write, leaving source selection/copy available.
    pub fn set_busy(&mut self, busy: bool, cx: &mut Context<Self>) {
        self.busy = busy;
        let enabled = !busy && self.snapshot.presentation == Presentation::Text;
        self.result.update(cx, |editor, cx| {
            editor.enabled = enabled;
            cx.notify();
        });
        cx.notify();
    }
    /// Accepts refresh metadata only when it cannot replace unsaved text or retarget a changed conflict.
    pub fn refresh(&mut self, fresh: Arc<ConflictSnapshot>, cx: &mut Context<Self>) {
        let same_sources = self.snapshot.path == fresh.path
            && self.snapshot.base == fresh.base
            && self.snapshot.a == fresh.a
            && self.snapshot.b == fresh.b;
        let same_operation = self.snapshot.operation.state == fresh.operation.state;
        let text = self.result.read(cx).buffer.text();
        let outcome = if self.snapshot.presentation == Presentation::Text {
            self.draft
                .refresh(&fresh.result, same_sources, same_operation, text)
        } else {
            DraftRefresh::Replace
        };
        if outcome == DraftRefresh::Stale {
            cx.notify();
            return;
        }
        if outcome == DraftRefresh::Replace {
            self.draft = ResultDraft::new(
                fresh.result.clone(),
                fresh.a.worktree.mode.or(fresh.b.worktree.mode),
            );
            self.result.update(cx, |editor, cx| {
                editor
                    .buffer
                    .reset(if fresh.presentation == Presentation::Text {
                        String::from_utf8(fresh.result.bytes.clone()).expect("validated text")
                    } else {
                        String::new()
                    });
                cx.notify();
            });
            self.chunk = 0;
        }
        for (editor, side) in [
            (&self.a, &fresh.a),
            (&self.b, &fresh.b),
            (&self.base, &fresh.base),
        ] {
            if !same_sources {
                editor.update(cx, |editor, cx| {
                    editor.buffer.reset(
                        std::str::from_utf8(&side.worktree.bytes)
                            .unwrap_or("")
                            .to_owned(),
                    );
                    cx.notify();
                });
            }
        }
        self.snapshot = fresh;
        self.set_busy(self.busy, cx);
    }
    /// Records only a confirmed Save Result, preserving selection/history until fresh guards arrive.
    pub fn saved(&mut self, document: Document, cx: &mut Context<Self>) {
        self.draft.saved(document);
        cx.notify();
    }
    /// Disables actions that could overwrite changed sources; manual input and copy remain usable.
    fn ready(&self) -> bool {
        !self.busy && !self.draft.stale && !self.draft.awaiting_refresh
    }
    /// Submits the separately saved Result; Mark resolved and Continue are never implied.
    pub fn save(&mut self, cx: &mut Context<Self>) {
        if self.ready() && self.dirty(cx) && self.snapshot.presentation == Presentation::Text {
            cx.emit(ConflictEvent::Save(
                self.snapshot.clone(),
                self.draft.document(self.result.read(cx).buffer.text()),
            ));
        }
    }
    /// Submits explicit index resolution only for saved text or an externally edited regular file.
    pub fn resolve(&mut self, cx: &mut Context<Self>) {
        if self.ready() && !self.dirty(cx) && !self.snapshot.result_staged {
            cx.emit(ConflictEvent::Resolve(self.snapshot.clone()));
        }
    }
    /// Replaces the full Result as one undoable edit; an absent side requires a separate delete choice.
    pub fn take_side(&mut self, a: bool, cx: &mut Context<Self>) {
        if self.busy || self.snapshot.presentation != Presentation::Text {
            return;
        }
        let side = if a {
            &self.snapshot.a
        } else {
            &self.snapshot.b
        };
        if !side.worktree.present {
            return;
        }
        let text = std::str::from_utf8(&side.worktree.bytes).expect("validated text");
        self.result.update(cx, |editor, cx| {
            editor.buffer.replace_document(text);
            cx.notify();
        });
        self.draft.present = true;
        self.draft.mode = side.worktree.mode;
        cx.emit(ConflictEvent::Changed);
        cx.notify();
    }
    /// Chooses keep/delete separately from text edits, retaining the buffer until Save is confirmed.
    pub fn choose_presence(&mut self, present: bool, cx: &mut Context<Self>) {
        if !self.busy && self.snapshot.presentation == Presentation::Text {
            self.draft.present = present;
            cx.emit(ConflictEvent::Changed);
            cx.notify();
        }
    }
    /// Chooses only a chunk from the exact rendered text revision, retaining native undo/redo.
    pub fn take_chunk(
        &mut self,
        index: usize,
        revision: u64,
        choice: ChunkChoice,
        cx: &mut Context<Self>,
    ) {
        if self.busy
            || self.snapshot.presentation != Presentation::Text
            || self.result.read(cx).buffer.revision != revision
        {
            return;
        }
        let chosen = self.snapshot.choose_chunk(
            self.result.read(cx).buffer.text().as_bytes(),
            index,
            choice,
        );
        match chosen {
            Ok(bytes) => {
                let text = String::from_utf8(bytes).expect("validated text");
                self.result.update(cx, |editor, cx| {
                    editor.buffer.replace_document(&text);
                    cx.notify();
                });
                self.draft.present = true;
                self.chunk = 0;
                cx.emit(ConflictEvent::Changed);
                cx.notify();
            }
            Err(error) => cx.emit(ConflictEvent::Error(error)),
        }
    }
    /// Selects and scrolls to one marker block from the displayed revision without changing text.
    fn select_chunk(
        &mut self,
        index: usize,
        revision: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let editor = self.result.read(cx);
        if editor.buffer.revision != revision {
            return;
        }
        let Ok(blocks) = chunks(editor.buffer.text().as_bytes()) else {
            return;
        };
        let Some(block) = blocks.get(index) else {
            return;
        };
        let range = block.range.clone();
        self.chunk = index;
        self.result.update(cx, |editor, cx| {
            editor.reveal_range(range, cx);
        });
        window.focus(&self.result.read(cx).focus_handle(cx));
        self.chunk_scroll
            .scroll_to_item(index, ScrollStrategy::Center);
        cx.notify();
    }
    /// Handles Result shortcuts locally after the focused text input has handled editing keys.
    fn key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if event.keystroke.modifiers.control && event.keystroke.key == "s" {
            self.save(cx);
            window.prevent_default();
            cx.stop_propagation();
        } else if event.keystroke.modifiers.control
            && event.keystroke.modifiers.shift
            && event.keystroke.key == "r"
        {
            self.resolve(cx);
            window.prevent_default();
            cx.stop_propagation();
        }
    }
    /// Displays exact provenance and file absence above a read-only side, without ours/theirs ambiguity.
    fn source_panel(
        &self,
        label: &str,
        side: &ConflictSide,
        editor: &Entity<NativeEditor>,
    ) -> gpui::Div {
        let mut panel = div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .border_r_1()
            .border_color(rgb(theme::BORDER));
        panel = panel.child(
            div()
                .h(px(32.))
                .flex_none()
                .px_2()
                .flex()
                .items_center()
                .gap_2()
                .bg(rgb(theme::PANEL))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_ellipsis()
                        .child(self.t(label)),
                )
                .child(
                    div().text_xs().text_color(rgb(theme::MUTED)).child(
                        side.id
                            .as_deref()
                            .map(|id| id.chars().take(10).collect::<String>())
                            .unwrap_or_else(|| self.t("conflict-absent")),
                    ),
                ),
        );
        if side.worktree.present {
            panel.child(div().flex_1().min_h_0().child(editor.clone()))
        } else {
            panel.child(
                div()
                    .p_3()
                    .text_color(rgb(theme::MUTED))
                    .child(self.t("conflict-absent")),
            )
        }
    }
}

impl Render for ConflictView {
    /// Renders A / independent Result / B, with Base replacing A only on explicit toggle.
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let _ = &self.subscriptions;
        let ready = self.ready();
        let dirty = self.dirty(cx);
        let text = self.result.read(cx).buffer.text();
        let revision = self.result.read(cx).buffer.revision;
        let blocks = chunks(text.as_bytes());
        let count = blocks.as_ref().map_or(0, Vec::len);
        self.chunk = self.chunk.min(count.saturating_sub(1));
        let chunk = self.chunk;
        let editable = self.snapshot.presentation == Presentation::Text && !self.busy;
        let mut panel = div()
            .size_full()
            .flex()
            .flex_col()
            .min_w_0()
            .min_h_0()
            .on_key_down(cx.listener(Self::key_down));
        panel =
            panel.child(
                div()
                    .h(px(40.))
                    .flex_none()
                    .px_2()
                    .flex()
                    .items_center()
                    .gap_2()
                    .border_b_1()
                    .border_color(rgb(theme::BORDER))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_ellipsis()
                            .child(self.snapshot.path.to_string_lossy().into_owned()),
                    )
                    .child(
                        control("show-base", self.t("conflict-base"), self.show_base, true)
                            .on_click(cx.listener(|view, _, _, cx| {
                                view.toggle_base(cx);
                            })),
                    )
                    .child(
                        control("save-result", self.t("save-result"), false, ready && dirty)
                            .on_click(cx.listener(|view, _, _, cx| view.save(cx))),
                    )
                    .child(
                        control(
                            "mark-resolved",
                            self.t("mark-resolved"),
                            false,
                            ready && !dirty && !self.snapshot.result_staged,
                        )
                        .on_click(cx.listener(|view, _, _, cx| view.resolve(cx))),
                    )
                    .child(
                        control("reload-result", self.t("reload-result"), false, !self.busy)
                            .on_click(cx.listener(|view, _, _, cx| {
                                if !view.busy {
                                    cx.emit(ConflictEvent::Reload);
                                }
                            })),
                    ),
            );
        if self.draft.stale {
            panel = panel.child(
                div()
                    .p_2()
                    .text_color(rgb(0xe2bb77))
                    .child(self.t("result-stale")),
            );
        }
        if self.snapshot.presentation != Presentation::Text {
            let reason = match self.snapshot.presentation {
                Presentation::Binary => "binary",
                Presentation::InvalidEncoding => "encoding",
                Presentation::TooLarge => "large",
                _ => "unsupported",
            };
            return panel.child(
                div()
                    .p_4()
                    .text_color(rgb(0xe2bb77))
                    .child(self.t(reason))
                    .child(self.t("conflict-external")),
            );
        }
        let state = if self.snapshot.resolved && self.snapshot.result_staged {
            "conflict-resolved"
        } else if self.snapshot.resolved && !dirty {
            "result-not-staged"
        } else if dirty {
            "result-unsaved"
        } else {
            "conflict-unresolved"
        };
        let mut choices = div()
            .h(px(34.))
            .flex_none()
            .px_2()
            .flex()
            .items_center()
            .gap_2()
            .child(
                div()
                    .flex_1()
                    .text_xs()
                    .text_color(rgb(theme::MUTED))
                    .child(self.t(state)),
            );
        if count > 0 {
            for (id, key, choice) in [
                ("take-chunk-a", "take-a", ChunkChoice::A),
                ("take-chunk-b", "take-b", ChunkChoice::B),
                ("take-ab", "take-ab", ChunkChoice::AThenB),
                ("take-ba", "take-ba", ChunkChoice::BThenA),
            ] {
                choices = choices.child(control(id, self.t(key), false, editable).on_click(
                    cx.listener(move |view, _, _, cx| view.take_chunk(chunk, revision, choice, cx)),
                ));
            }
        } else {
            choices = choices.child(div().text_xs().text_color(rgb(theme::MUTED)).child(self.t(
                if blocks.is_err() {
                    "chunks-manual"
                } else {
                    "chunks-none"
                },
            )));
        }
        panel = panel.child(choices);
        if count > 0 {
            panel = panel.child(
                uniform_list(
                    "conflict-chunks",
                    count,
                    cx.processor(move |view, range: std::ops::Range<usize>, _, cx| {
                        range
                            .map(|index| {
                                div()
                                    .id(("conflict-chunk", index))
                                    .h(px(24.))
                                    .px_3()
                                    .cursor_pointer()
                                    .bg(rgb(if view.chunk == index {
                                        theme::SELECTED
                                    } else {
                                        theme::BACKGROUND
                                    }))
                                    .child({
                                        let mut args = FluentArgs::new();
                                        args.set("number", index + 1);
                                        view.translator.text("chunk-number", Some(&args))
                                    })
                                    .on_click(cx.listener(move |view, _, window, cx| {
                                        view.select_chunk(index, revision, window, cx)
                                    }))
                            })
                            .collect::<Vec<_>>()
                    }),
                )
                .h(px((count.min(3) * 24) as f32))
                .flex_none()
                .track_scroll(self.chunk_scroll.clone()),
            );
        }
        let rebase = self
            .snapshot
            .operation
            .state
            .as_ref()
            .is_some_and(|state| state.kind == Operation::Rebase);
        let left = if self.show_base {
            self.source_panel("conflict-base", &self.snapshot.base, &self.base)
        } else {
            self.source_panel(
                if rebase {
                    "source-a-rebase"
                } else {
                    "source-a-merge"
                },
                &self.snapshot.a,
                &self.a,
            )
        };
        let mut center = div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .border_r_1()
            .border_color(rgb(theme::BORDER))
            .child(
                div()
                    .h(px(32.))
                    .flex_none()
                    .px_2()
                    .flex()
                    .items_center()
                    .bg(rgb(theme::PANEL))
                    .child(self.t("result")),
            )
            .child(div().flex_1().min_h_0().child(self.result.clone()));
        if !self.draft.present {
            center = center.child(
                div()
                    .p_2()
                    .text_color(rgb(0xe2bb77))
                    .child(self.t("result-delete")),
            );
        }
        panel = panel.child(
            div()
                .flex_1()
                .min_h_0()
                .flex()
                .child(left)
                .child(center)
                .child(self.source_panel(
                    if rebase {
                        "source-b-rebase"
                    } else {
                        "source-b-merge"
                    },
                    &self.snapshot.b,
                    &self.b,
                )),
        );
        panel.child(
            div()
                .h(px(36.))
                .flex_none()
                .px_2()
                .flex()
                .items_center()
                .gap_2()
                .border_t_1()
                .border_color(rgb(theme::BORDER))
                .child(
                    control(
                        "take-file-a",
                        self.t("take-file-a"),
                        false,
                        editable && self.snapshot.a.worktree.present,
                    )
                    .on_click(cx.listener(|view, _, _, cx| view.take_side(true, cx))),
                )
                .child(
                    control(
                        "take-file-b",
                        self.t("take-file-b"),
                        false,
                        editable && self.snapshot.b.worktree.present,
                    )
                    .on_click(cx.listener(|view, _, _, cx| view.take_side(false, cx))),
                )
                .child(div().flex_1())
                .child(
                    control(
                        "keep-result",
                        self.t("keep-result"),
                        self.draft.present,
                        editable,
                    )
                    .on_click(cx.listener(|view, _, _, cx| {
                        if !view.busy {
                            view.choose_presence(true, cx);
                        }
                    })),
                )
                .child(
                    control(
                        "delete-result",
                        self.t("delete-result"),
                        !self.draft.present,
                        editable,
                    )
                    .on_click(cx.listener(|view, _, _, cx| {
                        if !view.busy {
                            view.choose_presence(false, cx);
                        }
                    })),
                ),
        )
    }
}
