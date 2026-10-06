//! Finite native UX acceptance activated only by explicit disposable smoke scenarios.
use super::*;

pub(super) struct UxState {
    original: Arc<DiffSnapshot>,
    root: PathBuf,
    filter: (String, hunkra::text::Selection, u64),
}
impl Hunkra {
    /// Sends a shortcut through the host handler using GPUI's current key representation and an actual native window.
    fn ux_key(&mut self, shortcut: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.key_down(
            &KeyDownEvent {
                keystroke: gpui::Keystroke::parse(shortcut).unwrap(),
                is_held: false,
            },
            window,
            cx,
        );
    }
    /// Uses the platform text input contract and event dispatch, preserving exact Unicode selection/revision evidence.
    fn ux_filter(&mut self, query: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.filter_input.update(cx, |editor, cx| {
            editor.buffer.select(0, false);
            editor.buffer.select(editor.buffer.text().len(), true);
            editor.replace_text_in_range(None, query, window, cx);
        });
    }
    /// Verifies hidden dirty Result and user message survive a query, locale change or cancelled project navigation.
    fn ux_assert_draft(&self, cx: &Context<Self>) {
        assert_eq!(self.message.read(cx).buffer.text(), SMOKE_MESSAGE);
        if self.smoke.as_ref().unwrap().operation.as_deref() == Some("ux-draft") {
            let view = self.conflict.as_ref().unwrap().read(cx);
            assert!(view.dirty(cx));
            assert_eq!(
                view.result.read(cx).buffer.text(),
                self.smoke.as_ref().unwrap().result_text.as_deref().unwrap()
            );
            assert_eq!(
                Some(view.result.read(cx).buffer.selection),
                self.smoke.as_ref().unwrap().result_selection
            );
        }
    }
    /// Drives actual native field/focus/filter/list/long-line/history/recent controls and records only owned finite evidence.
    pub(super) fn smoke_ux_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(smoke) = &self.smoke else {
            return;
        };
        let Some(action) = smoke
            .operation
            .as_deref()
            .filter(|action| action.starts_with("ux-"))
        else {
            return;
        };
        assert!(
            smoke.started.elapsed() < Duration::from_secs(30),
            "native UX deadline: phase {}, scanning {}, diff_loading {}, filter_loading {}, filter rows {}, selected {:?}, error {:?}",
            smoke.phase,
            self.scanning,
            self.diff_loading,
            self.path_filter.loading,
            self.path_filter.rows.len(),
            self.view.selected,
            self.error_key
        );
        if !smoke.input_prepared
            || self.scanning
            || self.diff_loading
            || self.view.diff.is_none()
            || self.path_filter.loading
            || action == "ux-draft" && !smoke.result_ready
        {
            return;
        }
        let action = action.to_owned();
        let phase = smoke.phase;
        if phase == 0 {
            assert_eq!(self.settings.recent.len(), 2);
            assert_eq!(
                self.view.selected.as_deref(),
                Some(std::path::Path::new("f"))
            );
            self.ux = Some(UxState {
                original: self.view.diff.clone().unwrap(),
                root: self.root.clone().unwrap(),
                filter: (String::new(), Default::default(), 0),
            });
            self.ux_key("ctrl-f", window, cx);
            assert!(
                self.filter_input
                    .read(cx)
                    .focus_handle(cx)
                    .is_focused(window)
            );
            self.ux_filter("SRC ЯДРО", window, cx);
        } else if phase == 1 {
            if self.path_filter.rows.len() != 2 {
                return;
            }
            let editor = self.filter_input.read(cx);
            self.ux.as_mut().unwrap().filter = (
                editor.buffer.text().to_owned(),
                editor.buffer.selection,
                editor.buffer.revision,
            );
        } else if phase <= 4 {
            let generation = self.scan_generation;
            let diff_generation = self.view.diff_generation;
            self.set_locale([Locale::En, Locale::Ru, Locale::RuDev][phase - 2], cx);
            assert_eq!(
                (generation, diff_generation),
                (self.scan_generation, self.view.diff_generation)
            );
            assert!(Arc::ptr_eq(
                &self.ux.as_ref().unwrap().original,
                self.view.diff.as_ref().unwrap()
            ));
            let editor = self.filter_input.read(cx);
            assert_eq!(
                (
                    editor.buffer.text().to_owned(),
                    editor.buffer.selection,
                    editor.buffer.revision
                ),
                self.ux.as_ref().unwrap().filter
            );
            self.ux_assert_draft(cx);
            let label = self.t("changes");
            self.smoke.as_mut().unwrap().locales.push(label);
        } else if action == "ux-draft" {
            if phase == 5 {
                self.ux_key("ctrl-r", window, cx);
                assert!(self.recent_open);
                self.ux_key("down", window, cx);
                self.ux_key("enter", window, cx);
                assert!(self.confirmation.is_some());
                assert_eq!(self.root.as_ref(), Some(&self.ux.as_ref().unwrap().root));
            } else if phase == 6 {
                self.ux_key("escape", window, cx);
                assert!(self.confirmation.is_none());
                self.ux_assert_draft(cx);
                assert!(Arc::ptr_eq(
                    &self.ux.as_ref().unwrap().original,
                    self.view.diff.as_ref().unwrap()
                ));
                self.recent_open = false;
            } else {
                self.finish_ux(cx);
                return;
            }
        } else if phase == 5 {
            self.ux_key("down", window, cx);
        } else if phase == 6 {
            let path = &self.view.repository.as_ref().unwrap().files[self.path_filter.rows[0]].path;
            assert_eq!(self.view.selected.as_ref(), Some(path));
            self.ux_key("tab", window, cx);
            assert!(self.message.read(cx).focus_handle(cx).is_focused(window));
            self.ux_key("shift-tab", window, cx);
            assert!(
                self.filter_input
                    .read(cx)
                    .focus_handle(cx)
                    .is_focused(window)
            );
            self.ux_key("down", window, cx);
        } else if phase == 7 {
            let path = &self.view.repository.as_ref().unwrap().files[self.path_filter.rows[1]].path;
            assert_eq!(self.view.selected.as_ref(), Some(path));
            self.filter_input.update(cx, |editor, cx| {
                editor.key_down(
                    &KeyDownEvent {
                        keystroke: gpui::Keystroke::parse("enter").unwrap(),
                        is_held: false,
                    },
                    window,
                    cx,
                )
            });
        } else if phase == 8 {
            let path = &self.view.repository.as_ref().unwrap().files[self.path_filter.rows[1]].path;
            if self.view.selected.as_ref() != Some(path) || self.filter_submitted {
                return;
            }
            assert!(self.focus.is_focused(window));
            for query in ["src", "ЯД", "missing", "src .bsl", "no matching file"] {
                self.ux_filter(query, window, cx);
            }
        } else if phase == 9 {
            if !self.path_filter.rows.is_empty() {
                return;
            }
            assert_eq!(self.filter_input.read(cx).buffer.text(), "no matching file");
            assert!(self.view.selected.is_some() && self.view.diff.is_some());
            self.clear_filter(cx);
        } else if phase == 10 {
            assert_eq!(
                self.path_filter.rows.len(),
                self.view.repository.as_ref().unwrap().files.len()
            );
            self.select("f".into(), cx);
        } else if phase == 11 {
            let generation = self.scan_generation;
            self.ux_key("ctrl-r", window, cx);
            self.ux_key("enter", window, cx);
            assert!(!self.recent_open);
            assert_eq!(generation, self.scan_generation);
            assert_eq!(self.message.read(cx).buffer.text(), SMOKE_MESSAGE);
            if !self.horizontal.shift(px(320.)) {
                cx.notify();
                return;
            }
            assert_eq!(self.horizontal.offset(), px(320.));
        } else if phase == 12 {
            let generation = (self.scan_generation, self.view.diff_generation);
            for locale in [Locale::En, Locale::Ru, Locale::RuDev] {
                self.set_locale(locale, cx);
            }
            assert_eq!(
                generation,
                (self.scan_generation, self.view.diff_generation)
            );
            assert_eq!(self.horizontal.offset(), px(320.));
            if action == "ux-history" {
                self.select_tab(2, cx);
            } else {
                self.ux_key("ctrl-r", window, cx);
                self.ux_key("down", window, cx);
                self.ux_key("enter", window, cx);
            }
        } else if action == "ux-history" {
            let Some(history) = &self.history else {
                return;
            };
            let view = history.read(cx);
            if view.loading || view.diff.is_none() {
                return;
            }
            if phase == 13 {
                if !view.horizontal.shift(px(320.)) {
                    cx.notify();
                    return;
                }
                assert_eq!(view.horizontal.offset(), px(320.));
            } else {
                let diff = view.diff.clone().unwrap();
                let generation = view.generation;
                history.update(cx, |view, cx| {
                    for locale in [Locale::En, Locale::Ru, Locale::RuDev] {
                        view.set_locale(locale, cx);
                    }
                    assert_eq!(view.generation, generation);
                    assert!(Arc::ptr_eq(&diff, view.diff.as_ref().unwrap()));
                    assert_eq!(view.horizontal.offset(), px(320.));
                });
                self.finish_ux(cx);
                return;
            }
        } else {
            let destination = PathBuf::from(std::env::var_os("HUNKRA_UX_RECENT").unwrap());
            if self.root.as_ref() != Some(&destination) {
                return;
            }
            assert_ne!(self.root.as_ref(), Some(&self.ux.as_ref().unwrap().root));
            assert!(!self.recent_open);
            self.finish_ux(cx);
            return;
        }
        self.smoke.as_mut().unwrap().phase += 1;
        cx.notify();
    }
    /// Records native handler/render acceptance, while keeping source text, URLs, clipboard and settings private.
    fn finish_ux(&mut self, cx: &mut Context<Self>) {
        assert!(self.last_write_backend.is_none());
        let smoke = self.smoke.as_ref().unwrap();
        let report = serde_json::json!({"ok":true,"native_display":true,"scenario":smoke.operation,"frames":self.frames,"locales":smoke.locales,"read_only":true,"filter_native":true,"draft_retained":smoke.operation.as_deref()==Some("ux-draft"),"shared_horizontal_native":smoke.operation.as_deref()!=Some("ux-draft"),"measurements":self.measurements.json()});
        std::fs::write(&smoke.report, serde_json::to_vec_pretty(&report).unwrap()).unwrap();
        cx.quit();
    }
}
