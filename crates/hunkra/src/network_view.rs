//! Compact native remotes/project creation controls; reads are bounded and writes are emitted to the host.
use crate::{
    editor::{NativeEditor, TextChanged},
    i18n::{Locale, Translator},
    network_action::NetworkCommand,
    theme,
    worker::LatestWorker,
};
use gpui::{
    App, Context, Entity, EventEmitter, FocusHandle, Focusable, KeyDownEvent, Subscription,
    UniformListScrollHandle, Window, div, prelude::*, px, rgb, uniform_list,
};
use hunkra_core::{
    CoreError,
    network::{NetworkSnapshot, ObjectFormat, PushRequest, prepare_network},
};
use std::{path::PathBuf, sync::Arc};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum NetworkPage {
    Remotes,
    Init,
    Clone,
}
pub struct NetworkEvent(pub NetworkCommand);
pub struct NetworkView {
    root: Option<PathBuf>,
    translator: Translator,
    worker: LatestWorker<PathBuf, Result<NetworkSnapshot, CoreError>>,
    pub snapshot: Option<Arc<NetworkSnapshot>>,
    reviewed: Option<Arc<NetworkSnapshot>>,
    pub selected_remote: Option<String>,
    pub branch: Entity<NativeEditor>,
    pub destination: Entity<NativeEditor>,
    pub url: Entity<NativeEditor>,
    pub initial_branch: Entity<NativeEditor>,
    pub page: NetworkPage,
    pub generation: u64,
    pub loading: bool,
    pub busy: bool,
    pub set_upstream: bool,
    pub format: ObjectFormat,
    pub error: Option<&'static str>,
    pub status: Option<&'static str>,
    pub frames: usize,
    branch_initialized: bool,
    remote_scroll: UniformListScrollHandle,
    branch_scroll: UniformListScrollHandle,
    focus: FocusHandle,
    _subscriptions: Vec<Subscription>,
}
impl EventEmitter<NetworkEvent> for NetworkView {}

/// Builds the compact panel's owned controls; handlers enforce enabled state independently of appearance.
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
impl NetworkView {
    /// Initializes finite inputs and one latest-only reader without touching credentials or settings.
    pub fn new(root: Option<PathBuf>, locale: Locale, cx: &mut Context<Self>) -> Self {
        let translator = Translator::new(locale);
        let branch = cx.new(|cx| {
            NativeEditor::single_line(
                4096,
                translator.text("network-branch-placeholder", None),
                translator.text("editor-preview-limit", None),
                cx,
            )
        });
        let destination = cx.new(|cx| {
            NativeEditor::single_line(
                16 * 1024,
                translator.text("network-destination-placeholder", None),
                translator.text("editor-preview-limit", None),
                cx,
            )
        });
        let url = cx.new(|cx| {
            NativeEditor::single_line(
                16 * 1024,
                translator.text("network-url-placeholder", None),
                translator.text("editor-preview-limit", None),
                cx,
            )
        });
        let initial_branch = cx.new(|cx| {
            let mut editor = NativeEditor::single_line(
                4096,
                translator.text("branch-placeholder", None),
                translator.text("editor-preview-limit", None),
                cx,
            );
            editor.buffer.reset("main".into());
            editor
        });
        let mut subscriptions = Vec::new();
        for editor in [&branch, &destination, &url, &initial_branch] {
            subscriptions.push(cx.subscribe(editor, |_, _, _: &TextChanged, cx| cx.notify()));
        }
        let mut view = Self {
            root,
            translator,
            worker: LatestWorker::new("hunkra-remotes", |root: PathBuf| prepare_network(&root)),
            snapshot: None,
            reviewed: None,
            selected_remote: None,
            branch,
            destination,
            url,
            initial_branch,
            page: NetworkPage::Remotes,
            generation: 0,
            loading: false,
            busy: false,
            set_upstream: true,
            format: ObjectFormat::Sha1,
            error: None,
            status: None,
            frames: 0,
            branch_initialized: false,
            remote_scroll: UniformListScrollHandle::new(),
            branch_scroll: UniformListScrollHandle::new(),
            focus: cx.focus_handle(),
            _subscriptions: subscriptions,
        };
        view.reload(cx);
        view
    }
    /// Formats only bundled labels; paths, refs and entered URL text remain user data.
    fn t(&self, key: &str) -> String {
        self.translator.text(key, None)
    }
    /// Changes labels/placeholders without queries, editor reset, selection movement or undo changes.
    pub fn set_locale(&mut self, locale: Locale, cx: &mut Context<Self>) {
        self.translator = Translator::new(locale);
        for (editor, key) in [
            (&self.branch, "network-branch-placeholder"),
            (&self.destination, "network-destination-placeholder"),
            (&self.url, "network-url-placeholder"),
            (&self.initial_branch, "branch-placeholder"),
        ] {
            let placeholder = self.t(key);
            let notice = self.t("editor-preview-limit");
            editor.update(cx, |editor, cx| {
                editor.placeholder = placeholder.into();
                editor.preview_notice = notice.into();
                cx.notify();
            });
        }
        cx.notify();
    }
    /// Freezes every action input when the shared writer or product confirmation has accepted a request.
    pub fn set_busy(&mut self, busy: bool, cx: &mut Context<Self>) {
        self.busy = busy;
        for editor in [
            &self.branch,
            &self.destination,
            &self.url,
            &self.initial_branch,
        ] {
            editor.update(cx, |editor, cx| {
                editor.enabled = !busy;
                cx.notify();
            });
        }
        if busy {
            self.generation += 1;
            self.loading = false;
            self.reviewed = None;
        }
        cx.notify();
    }
    /// Starts an explicit remote-state refresh, retaining branch input and selected remote by name.
    pub fn reload(&mut self, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        if let Some(root) = &self.root {
            self.generation += 1;
            self.loading = true;
            self.error = None;
            self.worker.submit(self.generation, root.clone());
            cx.notify();
        }
    }
    /// Applies only the latest read; initial defaults are written once rather than clobbering a user's draft on refresh.
    pub fn poll(&mut self, cx: &mut Context<Self>) {
        if let Some((generation, result)) = self.worker.take()
            && generation == self.generation
            && !self.busy
        {
            self.loading = false;
            match result {
                Ok(snapshot) => {
                    self.selected_remote = self
                        .selected_remote
                        .take()
                        .filter(|name| snapshot.remotes.iter().any(|remote| &remote.name == name))
                        .or_else(|| {
                            snapshot
                                .upstream
                                .as_ref()
                                .map(|upstream| upstream.remote.clone())
                        })
                        .or_else(|| snapshot.remotes.first().map(|remote| remote.name.clone()));
                    if !self.branch_initialized {
                        let branch = snapshot
                            .upstream
                            .as_ref()
                            .and_then(|upstream| upstream.branch.strip_prefix("refs/heads/"))
                            .or(snapshot.current_branch.as_deref())
                            .unwrap_or_default()
                            .to_owned();
                        self.branch.update(cx, |editor, cx| {
                            editor.buffer.reset(branch);
                            cx.notify();
                        });
                        self.branch_initialized = true;
                    }
                    self.snapshot = Some(Arc::new(snapshot));
                    self.error = None;
                }
                Err(error) => {
                    self.snapshot = None;
                    self.reviewed = None;
                    self.error = Some(match error {
                        CoreError::Stale => "error-stale",
                        CoreError::NetworkConfig => "error-network-config",
                        CoreError::ReferenceLimit => "error-reference-limit",
                        _ => "error-git",
                    });
                }
            }
            cx.notify();
        }
    }
    /// Selects a visible configured remote without touching its URL or replacing the target branch input.
    pub fn select_remote(&mut self, index: usize, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        if let Some(remote) = self
            .snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.remotes.get(index))
        {
            self.selected_remote = Some(remote.name.clone());
            cx.notify();
        }
    }
    /// Copies only a chosen tracking ref's branch component into the explicit target field.
    pub fn select_branch(&mut self, index: usize, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let Some(snapshot) = &self.snapshot else {
            return;
        };
        let Some(branch) = snapshot.branches.get(index) else {
            return;
        };
        let Some(remote) = self.selected_remote.as_ref() else {
            return;
        };
        let Some(name) = branch.name.strip_prefix(&format!("{remote}/")) else {
            return;
        };
        if name == "HEAD" {
            return;
        }
        let name = name.to_owned();
        self.branch.update(cx, |editor, cx| {
            editor.buffer.reset(name);
            cx.notify();
        });
        cx.notify();
    }
    /// Changes the visible creation/remotes page while retaining every input and the host's existing drafts.
    pub fn select_page(&mut self, page: NetworkPage, cx: &mut Context<Self>) {
        if !self.busy {
            self.page = page;
            cx.notify();
        }
    }
    /// Emits one displayed snapshot and explicit target; the host owns serialization, dirty/close guards and error refresh.
    pub fn action(&mut self, key: &str, cx: &mut Context<Self>) {
        if self.busy || self.loading || self.error.is_some() {
            return;
        }
        let Some(snapshot) = self.reviewed.clone() else {
            return;
        };
        let remote = self.selected_remote.clone();
        let branch = self.branch.read(cx).buffer.text().to_owned();
        let command = match (key, remote) {
            ("network-fetch", Some(remote)) => NetworkCommand::Fetch(snapshot, remote),
            ("network-push", Some(remote)) => NetworkCommand::Push(
                snapshot,
                PushRequest {
                    remote,
                    branch,
                    set_upstream: self.set_upstream,
                },
            ),
            ("network-upstream", Some(remote)) => {
                NetworkCommand::Upstream(snapshot, remote, branch)
            }
            ("network-pull", _) => NetworkCommand::Pull(snapshot),
            _ => return,
        };
        cx.emit(NetworkEvent(command));
    }
    /// Captures entered destination/URL/initial branch without logging or persisting them in app settings.
    pub fn create(&mut self, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let root = PathBuf::from(self.destination.read(cx).buffer.text());
        let command = match self.page {
            NetworkPage::Init => NetworkCommand::Init {
                root,
                branch: self.initial_branch.read(cx).buffer.text().to_owned(),
                format: self.format,
            },
            NetworkPage::Clone => NetworkCommand::Clone {
                destination: root,
                url: self.url.read(cx).buffer.text().to_owned(),
            },
            NetworkPage::Remotes => return,
        };
        cx.emit(NetworkEvent(command));
    }
    /// Provides explicit shortcuts for visible actions while letting native editors retain ordinary text input.
    fn key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let key = event.keystroke.key.as_str();
        if key == "f6" || key == "tab" && !event.keystroke.modifiers.control {
            let mut handles = vec![self.focus.clone()];
            match self.page {
                NetworkPage::Clone => {
                    handles.push(self.url.read(cx).focus_handle(cx));
                    handles.push(self.destination.read(cx).focus_handle(cx));
                }
                NetworkPage::Init => {
                    handles.push(self.destination.read(cx).focus_handle(cx));
                    handles.push(self.initial_branch.read(cx).focus_handle(cx));
                }
                NetworkPage::Remotes => handles.push(self.branch.read(cx).focus_handle(cx)),
            }
            let current = handles
                .iter()
                .position(|handle| handle.is_focused(window))
                .unwrap_or(0);
            let next = if event.keystroke.modifiers.shift {
                (current + handles.len() - 1) % handles.len()
            } else {
                (current + 1) % handles.len()
            };
            window.focus(&handles[next]);
            window.prevent_default();
            cx.stop_propagation();
        } else if event.keystroke.modifiers.control
            && key == "enter"
            && self.page != NetworkPage::Remotes
        {
            self.create(cx);
            cx.stop_propagation();
        } else if event.keystroke.modifiers.control
            && event.keystroke.modifiers.shift
            && self.page == NetworkPage::Remotes
        {
            let action = match key {
                "f" => Some("network-fetch"),
                "p" => Some("network-push"),
                "l" => Some("network-pull"),
                _ => None,
            };
            if let Some(action) = action {
                self.action(action, cx);
                cx.stop_propagation();
            }
        }
    }
    /// Renders native bounded creation fields; existing folders are accepted only by explicit Init semantics.
    fn creation(&self, cx: &Context<Self>) -> gpui::Div {
        let mut panel = div()
            .flex_1()
            .min_h_0()
            .p_6()
            .flex()
            .flex_col()
            .gap_3()
            .child(
                div()
                    .text_lg()
                    .child(self.t(if self.page == NetworkPage::Clone {
                        "network-clone"
                    } else {
                        "network-init"
                    })),
            )
            .child(div().text_color(rgb(theme::MUTED)).child(self.t(
                if self.page == NetworkPage::Clone {
                    "network-clone-hint"
                } else {
                    "network-init-hint"
                },
            )));
        if self.page == NetworkPage::Clone {
            panel = panel
                .child(div().text_sm().child(self.t("network-url")))
                .child(div().h(px(44.)).child(self.url.clone()));
        }
        panel = panel
            .child(div().text_sm().child(self.t("network-destination")))
            .child(div().h(px(44.)).child(self.destination.clone()));
        if self.page == NetworkPage::Init {
            let mut hashes = div().flex().gap_2();
            for (index, format, label) in [
                (0usize, ObjectFormat::Sha1, "SHA-1"),
                (1, ObjectFormat::Sha256, "SHA-256"),
            ] {
                hashes = hashes.child(
                    button(
                        ("object-format", index),
                        label.into(),
                        self.format == format,
                        !self.busy,
                    )
                    .on_click(cx.listener(move |view, _, _, cx| {
                        if !view.busy {
                            view.format = format;
                            cx.notify();
                        }
                    })),
                );
            }
            panel = panel
                .child(div().text_sm().child(self.t("network-initial-branch")))
                .child(div().h(px(44.)).child(self.initial_branch.clone()))
                .child(hashes);
        }
        panel.child(
            button(
                "create-repository",
                self.t(if self.busy {
                    "write-busy"
                } else {
                    "network-create"
                }),
                false,
                !self.busy,
            )
            .on_click(cx.listener(|view, _, _, cx| view.create(cx))),
        )
    }
    /// Renders virtual remote refs beside explicit branch/upstream actions, without endpoint URLs or blocking reads.
    fn remotes(&self, cx: &Context<Self>) -> gpui::Div {
        let mut sidebar = div()
            .w(px(260.))
            .flex_none()
            .min_h_0()
            .flex()
            .flex_col()
            .border_r_1()
            .border_color(rgb(theme::BORDER))
            .child(
                div()
                    .p_3()
                    .text_sm()
                    .text_color(rgb(theme::MUTED))
                    .child(self.t("network-remotes")),
            );
        let count = self
            .snapshot
            .as_ref()
            .map_or(0, |snapshot| snapshot.remotes.len());
        sidebar = sidebar.child(
            uniform_list(
                "remote-list",
                count,
                cx.processor(|view, range: std::ops::Range<usize>, _, cx| {
                    let Some(snapshot) = &view.snapshot else {
                        return Vec::new();
                    };
                    range
                        .map(|index| {
                            let remote = &snapshot.remotes[index];
                            button(
                                ("remote", index),
                                remote.name.clone(),
                                view.selected_remote.as_ref() == Some(&remote.name),
                                !view.busy,
                            )
                            .h(px(32.))
                            .on_click(
                                cx.listener(move |view, _, _, cx| view.select_remote(index, cx)),
                            )
                        })
                        .collect::<Vec<_>>()
                }),
            )
            .h(px(160.))
            .track_scroll(self.remote_scroll.clone()),
        );
        let count = self
            .snapshot
            .as_ref()
            .map_or(0, |snapshot| snapshot.branches.len());
        sidebar = sidebar
            .child(
                div()
                    .p_3()
                    .text_sm()
                    .text_color(rgb(theme::MUTED))
                    .child(self.t("network-tracking-branches")),
            )
            .child(
                uniform_list(
                    "remote-branches",
                    count,
                    cx.processor(|view, range: std::ops::Range<usize>, _, cx| {
                        let Some(snapshot) = &view.snapshot else {
                            return Vec::new();
                        };
                        range
                            .map(|index| {
                                let branch = &snapshot.branches[index];
                                div()
                                    .id(("remote-branch", index))
                                    .h(px(32.))
                                    .px_3()
                                    .flex()
                                    .items_center()
                                    .overflow_hidden()
                                    .cursor_pointer()
                                    .hover(|style| style.bg(rgb(theme::HOVER)))
                                    .child(branch.name.clone())
                                    .on_click(cx.listener(move |view, _, _, cx| {
                                        view.select_branch(index, cx)
                                    }))
                            })
                            .collect::<Vec<_>>()
                    }),
                )
                .flex_1()
                .min_h_0()
                .track_scroll(self.branch_scroll.clone()),
            );
        let mut panel = div().flex_1().min_w_0().p_6().flex().flex_col().gap_3();
        if let Some(snapshot) = &self.snapshot {
            panel = panel.child(
                div().text_lg().child(
                    snapshot
                        .current_branch
                        .clone()
                        .unwrap_or_else(|| self.t("detached")),
                ),
            );
            let upstream = snapshot
                .upstream
                .as_ref()
                .map(|upstream| format!("{} / {}", upstream.remote, upstream.branch))
                .unwrap_or_else(|| self.t("network-no-upstream"));
            panel = panel.child(
                div()
                    .text_color(rgb(theme::MUTED))
                    .child(format!("{}: {upstream}", self.t("network-upstream-label"))),
            );
        }
        if count == 0 {
            panel = panel.child(
                div()
                    .text_color(rgb(theme::MUTED))
                    .child(self.t("network-no-refs")),
            );
        }
        panel = panel
            .child(div().text_sm().child(self.t("network-target-branch")))
            .child(div().h(px(44.)).child(self.branch.clone()))
            .child(
                button(
                    "push-upstream-toggle",
                    self.t("network-set-upstream"),
                    self.set_upstream,
                    !self.busy,
                )
                .on_click(cx.listener(|view, _, _, cx| {
                    if !view.busy {
                        view.set_upstream = !view.set_upstream;
                        cx.notify();
                    }
                })),
            );
        let mut actions = div().flex().gap_2();
        for (index, key) in [
            "network-fetch",
            "network-upstream",
            "network-push",
            "network-pull",
        ]
        .into_iter()
        .enumerate()
        {
            actions = actions.child(
                button(
                    ("network-action", index),
                    self.t(key),
                    false,
                    !self.busy && !self.loading && self.reviewed.is_some(),
                )
                .on_click(cx.listener(move |view, _, _, cx| view.action(key, cx))),
            );
        }
        panel = panel.child(actions).child(
            div()
                .text_sm()
                .text_color(rgb(theme::MUTED))
                .child(self.t("network-pull-hint")),
        );
        div().flex_1().min_h_0().flex().child(sidebar).child(panel)
    }
}
impl Focusable for NetworkView {
    /// Lets the host focus the visible panel without moving any editor's selection.
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}
impl Render for NetworkView {
    /// Uses only existing snapshots and native entities; the rendered snapshot binds subsequent emitted actions.
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.frames += 1;
        self.reviewed = if self.busy || self.loading {
            None
        } else {
            self.snapshot.clone()
        };
        let mut tabs = div()
            .h(px(36.))
            .flex_none()
            .px_2()
            .flex()
            .items_center()
            .gap_1()
            .bg(rgb(theme::PANEL))
            .border_b_1()
            .border_color(rgb(theme::BORDER));
        for (index, page, key) in [
            (0usize, NetworkPage::Remotes, "network-remotes"),
            (1, NetworkPage::Init, "network-init"),
            (2, NetworkPage::Clone, "network-clone"),
        ] {
            tabs = tabs.child(
                button(
                    ("network-page", index),
                    self.t(key),
                    self.page == page,
                    !self.busy,
                )
                .on_click(cx.listener(move |view, _, _, cx| view.select_page(page, cx))),
            );
        }
        let mut panel = div()
            .size_full()
            .min_h_0()
            .flex()
            .flex_col()
            .text_color(rgb(theme::TEXT))
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::key_down))
            .child(tabs);
        if let Some(key) = self.error.or(self.status) {
            panel = panel.child(
                div()
                    .p_3()
                    .text_sm()
                    .text_color(rgb(theme::MUTED))
                    .child(self.t(key)),
            );
        }
        if self.loading {
            panel = panel.child(div().p_2().text_sm().child(self.t("loading")));
        }
        panel.child(if self.page == NetworkPage::Remotes {
            self.remotes(cx)
        } else {
            self.creation(cx)
        })
    }
}
