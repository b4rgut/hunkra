//! Owned workspace chrome and preferences; presentation actions never request repository work.
use super::*;
use hunkra::{settings::Language, theme};

pub(super) const SIDEBAR_WIDTH: f32 = 320.;

struct WindowHint(String);
impl Render for WindowHint {
    /// Displays a translated purpose for an icon-only window control.
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .p_2()
            .rounded_sm()
            .text_sm()
            .bg(rgb(theme::ELEVATED))
            .text_color(rgb(TEXT))
            .border_1()
            .border_color(rgb(LINE))
            .child(self.0.clone())
    }
}

#[derive(Clone, Copy)]
pub(super) enum WindowCommand {
    Minimize,
    ToggleMaximize,
    Close,
}

/// Builds a compact window control with a translated tooltip and stable test selector.
fn window_control(
    id: &'static str,
    symbol: &'static str,
    label: String,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .text_sm()
        .cursor_pointer()
        .bg(rgb(PANEL))
        .text_color(rgb(TEXT))
        .hover(move |style| {
            style.bg(rgb(if id == "window-close" {
                theme::REMOVED
            } else {
                HOVER
            }))
        })
        .child(symbol)
        .w(px(32.))
        .h_full()
        .px_0()
        .py_0()
        .rounded_none()
        .flex()
        .items_center()
        .justify_center()
        .debug_selector(move || id.into())
        .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .tooltip(move |_, cx| cx.new(|_| WindowHint(label.clone())).into())
}

/// Builds a rectangular workspace tab with a visible active edge.
fn tab(id: impl Into<gpui::ElementId>, label: String, active: bool) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .h_full()
        .px_3()
        .flex()
        .items_center()
        .text_sm()
        .cursor_pointer()
        .border_r_1()
        .border_b_2()
        .border_color(rgb(if active { ACCENT } else { LINE }))
        .bg(rgb(if active { BG } else { PANEL }))
        .text_color(rgb(if active { TEXT } else { MUTED }))
        .hover(|style| style.bg(rgb(HOVER)))
        .child(label)
}

impl Hunkra {
    /// Exercises the real Wayland maximize/restore acknowledgements and dispatches minimize in an isolated finite scenario.
    pub(super) fn smoke_window_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(smoke) = self
            .smoke
            .as_ref()
            .filter(|smoke| smoke.operation.as_deref() == Some("window-controls"))
        else {
            return;
        };
        assert!(
            smoke.started.elapsed() < Duration::from_secs(15),
            "window control deadline at phase {}",
            smoke.phase
        );
        if self.scanning || self.diff_loading || self.view.diff.is_none() {
            return;
        }
        match smoke.phase {
            0 => {
                assert!(!window.is_maximized());
                self.window_command(WindowCommand::ToggleMaximize, window, cx);
                self.smoke.as_mut().unwrap().phase = 1;
            }
            1 if window.is_maximized() => {
                self.window_command(WindowCommand::ToggleMaximize, window, cx);
                self.smoke.as_mut().unwrap().phase = 2;
            }
            2 if !window.is_maximized() => {
                self.window_command(WindowCommand::Minimize, window, cx);
                self.smoke.as_mut().unwrap().phase = 3;
            }
            _ => {}
        }
    }

    /// Sends native window commands through the same dirty-result and active-writer close guards as the OS button.
    pub(super) fn window_command(
        &mut self,
        command: WindowCommand,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match command {
            WindowCommand::Minimize => window.minimize_window(),
            WindowCommand::ToggleMaximize => window.zoom_window(),
            WindowCommand::Close => {
                if self.settings_open {
                    self.toggle_settings(window, cx);
                }
                self.menu_open = false;
                if self.request_close(cx) {
                    window.remove_window();
                }
            }
        }
        cx.notify();
    }

    /// Persists presentation preferences through the existing worker, except in disposable smoke runs.
    fn save_preferences(&self) {
        if self.smoke.is_none() {
            self.settings_worker.submit(0, self.settings.clone());
        }
    }

    /// Applies a language preference while keeping system mode distinct from its resolved catalog.
    pub(super) fn set_language(&mut self, language: Language, cx: &mut Context<Self>) {
        self.apply_locale(language.resolve(), cx);
        self.settings.locale = language;
        self.save_preferences();
    }

    /// Toggles optional diff explanations without changing documents, selections or workers.
    pub(super) fn toggle_diff_details(&mut self, cx: &mut Context<Self>) {
        self.settings.diff_details = !self.settings.diff_details;
        self.save_preferences();
        cx.notify();
    }

    /// Captures focus while preferences are open and restores the previous field when closing.
    pub(super) fn toggle_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.settings_open = !self.settings_open;
        self.menu_open = false;
        self.recent_open = false;
        if self.settings_open {
            self.settings_focus = window.focused(cx);
            window.focus(&self.focus);
        } else if let Some(focus) = self.settings_focus.take() {
            window.focus(&focus);
        }
        cx.notify();
    }

    /// Shows the repository and branch in a compact title row, with secondary actions in a menu.
    pub(super) fn workspace_titlebar(&self, window: &Window, cx: &Context<Self>) -> gpui::Div {
        let mut bar = div()
            .h(px(32.))
            .flex_none()
            .px_2()
            .flex()
            .items_center()
            .gap_2()
            .bg(rgb(PANEL))
            .border_b_1()
            .border_color(rgb(LINE))
            .child(
                button("application-menu", "☰".into(), self.menu_open)
                    .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(|app, _, window, cx| {
                        app.menu_open = !app.menu_open;
                        if app.settings_open {
                            app.toggle_settings(window, cx);
                            app.menu_open = true;
                        }
                        window.focus(&app.focus);
                        cx.notify();
                    })),
            );
        let project = self.root.as_ref().and_then(|path| path.file_name());
        bar = bar.child(
            div().text_sm().child(
                project
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "Hunkra".into()),
            ),
        );
        if let Some(repository) = &self.view.repository {
            bar = bar.child(div().text_color(rgb(MUTED)).child("/")).child(
                button(
                    "title-branch",
                    format!(
                        "⑂ {}",
                        repository
                            .branch
                            .clone()
                            .unwrap_or_else(|| self.t("detached"))
                    ),
                    self.branch_tab,
                )
                .on_click(cx.listener(|app, _, window, cx| {
                    app.select_tab(1, cx);
                    app.focus_tab(window, cx);
                })),
            );
        }
        bar.child(div().flex_1())
            .child(
                button(
                    "refresh",
                    format!(
                        "↻ {}",
                        self.t(if self.scanning { "loading" } else { "refresh" })
                    ),
                    false,
                )
                .on_click(cx.listener(|app, _, _, cx| app.refresh_visible(cx))),
            )
            .child(
                button(
                    "settings-menu",
                    format!("⚙ {}", self.t("settings")),
                    self.settings_open,
                )
                .debug_selector(|| "settings-menu".into())
                .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(cx.listener(|app, _, window, cx| app.toggle_settings(window, cx))),
            )
            .child(
                window_control("window-minimize", "−", self.t("window-minimize")).on_click(
                    cx.listener(|app, _, window, cx| {
                        app.window_command(WindowCommand::Minimize, window, cx)
                    }),
                ),
            )
            .child(
                window_control(
                    "window-maximize",
                    if window.is_maximized() { "❐" } else { "□" },
                    self.t(if window.is_maximized() {
                        "window-restore"
                    } else {
                        "window-maximize"
                    }),
                )
                .on_click(cx.listener(|app, _, window, cx| {
                    app.window_command(WindowCommand::ToggleMaximize, window, cx)
                })),
            )
            .child(
                window_control("window-close", "×", self.t("close")).on_click(cx.listener(
                    |app, _, window, cx| app.window_command(WindowCommand::Close, window, cx),
                )),
            )
    }

    /// Aligns navigation with the sidebar and places the current document above the comparison panes.
    pub(super) fn workspace_tabs(&self, cx: &Context<Self>) -> gpui::Div {
        let mut navigation = div().w(px(SIDEBAR_WIDTH)).flex_none().h_full().flex();
        for (index, key) in ["changes", "branches", "history", "network"]
            .into_iter()
            .enumerate()
        {
            let active = match index {
                0 => !self.branch_tab && !self.history_tab && !self.network_tab,
                1 => self.branch_tab && !self.history_tab && !self.network_tab,
                2 => self.history_tab,
                _ => self.network_tab,
            };
            navigation = navigation.child(
                tab(("workspace-tab", index), self.t(key), active)
                    .flex_1()
                    .min_w_0()
                    .px_2()
                    .overflow_hidden()
                    .on_click(cx.listener(move |app, _, window, cx| {
                        app.select_tab(index, cx);
                        app.focus_tab(window, cx);
                    })),
            );
        }
        let mut tabs = div()
            .h(px(34.))
            .flex_none()
            .flex()
            .bg(rgb(PANEL))
            .border_b_1()
            .border_color(rgb(LINE))
            .child(navigation);
        if !self.history_tab && !self.network_tab {
            let label = self
                .view
                .selected
                .as_ref()
                .and_then(|path| path.file_name())
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| self.t("select-file"));
            tabs = tabs
                .child(
                    tab("document-tab", label, true)
                        .max_w(px(300.))
                        .overflow_hidden(),
                )
                .child(
                    div()
                        .px_3()
                        .h_full()
                        .flex()
                        .items_center()
                        .text_sm()
                        .text_color(rgb(MUTED))
                        .child(self.t(if self.mode == DiffKind::Staged {
                            "staged"
                        } else {
                            "unstaged"
                        })),
                );
        }
        tabs
    }

    /// Keeps the existing commit editor and exact-index action at the bottom of the changes sidebar.
    pub(super) fn commit_panel(&self, cx: &Context<Self>) -> gpui::Div {
        div()
            .debug_selector(|| "commit-panel".into())
            .h(px(188.))
            .flex_none()
            .p_2()
            .flex()
            .flex_col()
            .gap_2()
            .bg(rgb(BG))
            .border_t_1()
            .border_color(rgb(LINE))
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(MUTED))
                    .child(self.t("commit-message-label")),
            )
            .child(div().flex_1().min_h_0().child(self.message.clone()))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(div().text_xs().text_color(rgb(MUTED)).child("Ctrl+Enter"))
                    .child(div().flex_1())
                    .child(
                        button(
                            "commit",
                            self.t(if self.writing { "write-busy" } else { "commit" }),
                            false,
                        )
                        .border_1()
                        .border_color(rgb(LINE))
                        .opacity(
                            if self.writing
                                || self.scanning
                                || self.diff_loading
                                || self.reviewed_commit.is_none()
                                || self.confirmation.is_some()
                            {
                                0.5
                            } else {
                                1.
                            },
                        )
                        .on_click(cx.listener(|app, _, _, cx| app.commit(cx))),
                    ),
            )
    }

    /// Renders the separate preferences menu over the workspace, retaining all underlying view entities.
    pub(super) fn preferences_overlay(&self, cx: &Context<Self>) -> gpui::Stateful<gpui::Div> {
        let mut panel = div()
            .id("preferences-panel")
            .debug_selector(|| "preferences-panel".into())
            .absolute()
            .top(px(6.))
            .right(px(12.))
            .w(px(360.))
            .p_4()
            .flex()
            .flex_col()
            .gap_3()
            .rounded_sm()
            .bg(rgb(theme::ELEVATED))
            .border_1()
            .border_color(rgb(LINE))
            .shadow_lg()
            .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(|_, _, cx| cx.stop_propagation())
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(self.t("settings"))
                    .child(button("close-settings", self.t("close"), false).on_click(
                        cx.listener(|app, _, window, cx| app.toggle_settings(window, cx)),
                    )),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(rgb(MUTED))
                    .child(self.t("language")),
            );
        for (index, language) in [
            Language::System,
            Language::RuDev,
            Language::Ru,
            Language::En,
        ]
        .into_iter()
        .enumerate()
        {
            let label = if language == Language::System {
                self.t("language-system")
            } else {
                language.resolve().label().into()
            };
            panel = panel.child(
                button(
                    ("language-setting", index),
                    format!(
                        "{} {label}",
                        if self.settings.locale == language {
                            "●"
                        } else {
                            "○"
                        }
                    ),
                    self.settings.locale == language,
                )
                .debug_selector(move || format!("language-setting-{index}"))
                .on_click(cx.listener(move |app, _, _, cx| app.set_language(language, cx))),
            );
        }
        panel = panel
            .child(div().text_xs().text_color(rgb(MUTED)).child(format!(
                "{}: {}",
                self.t("language-system-detected"),
                settings::system_locale().label()
            )))
            .child(
                div()
                    .pt_2()
                    .border_t_1()
                    .border_color(rgb(LINE))
                    .text_sm()
                    .text_color(rgb(MUTED))
                    .child(self.t("appearance")),
            )
            .child(
                div()
                    .flex()
                    .justify_between()
                    .text_sm()
                    .child(self.t("theme"))
                    .child("GitHub Dark"),
            )
            .child(
                button(
                    "diff-details-setting",
                    format!(
                        "{} {}",
                        if self.settings.diff_details {
                            "☑"
                        } else {
                            "☐"
                        },
                        self.t("diff-details")
                    ),
                    self.settings.diff_details,
                )
                .debug_selector(|| "diff-details-setting".into())
                .on_click(cx.listener(|app, _, _, cx| app.toggle_diff_details(cx))),
            );
        div()
            .id("preferences-overlay")
            .absolute()
            .inset_0()
            .top(px(32.))
            .occlude()
            .on_click(cx.listener(|app, _, window, cx| app.toggle_settings(window, cx)))
            .child(panel)
    }

    /// Groups project opening and creation in the application menu instead of the primary toolbar.
    pub(super) fn application_overlay(
        &self,
        window: &Window,
        cx: &Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let panel = div()
            .id("application-panel")
            .absolute()
            .top(px(6.))
            .left(px(8.))
            .w(px(248.))
            .p_2()
            .flex()
            .flex_col()
            .gap_1()
            .bg(rgb(theme::ELEVATED))
            .border_1()
            .border_color(rgb(LINE))
            .rounded_sm()
            .shadow_lg()
            .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(|_, _, cx| cx.stop_propagation())
            .child(
                button("open", self.t("open"), false).on_click(cx.listener(|app, _, _, cx| {
                    app.menu_open = false;
                    app.choose_repository(cx);
                })),
            )
            .child(
                button("recent-menu", self.t("recent"), false).on_click(cx.listener(
                    |app, _, _, cx| {
                        app.menu_open = false;
                        app.recent_open = true;
                        cx.notify();
                    },
                )),
            )
            .child(
                button("new-repository", self.t("network-init"), false).on_click(cx.listener(
                    |app, _, window, cx| {
                        app.menu_open = false;
                        app.show_creation(NetworkPage::Init, cx);
                        app.focus_tab(window, cx);
                    },
                )),
            )
            .child(
                button("clone-repository", self.t("network-clone"), false).on_click(cx.listener(
                    |app, _, window, cx| {
                        app.menu_open = false;
                        app.show_creation(NetworkPage::Clone, cx);
                        app.focus_tab(window, cx);
                    },
                )),
            )
            .child(
                button(
                    "open-settings",
                    format!("{}  Ctrl+,", self.t("settings")),
                    false,
                )
                .on_click(cx.listener(|app, _, window, cx| app.toggle_settings(window, cx))),
            )
            .child(div().my_1().border_t_1().border_color(rgb(LINE)))
            .child(
                button("menu-minimize", self.t("window-minimize"), false).on_click(cx.listener(
                    |app, _, window, cx| {
                        app.menu_open = false;
                        app.window_command(WindowCommand::Minimize, window, cx);
                    },
                )),
            )
            .child(
                button(
                    "menu-maximize",
                    self.t(if window.is_maximized() {
                        "window-restore"
                    } else {
                        "window-maximize"
                    }),
                    false,
                )
                .on_click(cx.listener(|app, _, window, cx| {
                    app.menu_open = false;
                    app.window_command(WindowCommand::ToggleMaximize, window, cx);
                })),
            )
            .child(
                button("menu-close", self.t("close"), false).on_click(cx.listener(
                    |app, _, window, cx| app.window_command(WindowCommand::Close, window, cx),
                )),
            );
        div()
            .id("application-overlay")
            .absolute()
            .inset_0()
            .top(px(32.))
            .occlude()
            .on_click(cx.listener(|app, _, _, cx| {
                app.menu_open = false;
                cx.notify();
            }))
            .child(panel)
    }
}
