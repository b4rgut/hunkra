//! Finite native acceptance handlers; never active without an explicit smoke fixture request.
use super::*;

impl Hunkra {
    /// Returns private fixture inputs solely for explicitly requested smoke windows; values are never serialized in reports.
    fn network_fixture_input(key: &str) -> String {
        std::env::var(key).unwrap_or_default()
    }
    /// Exercises actual single-line input callbacks and retains selections/revisions for locale and refusal assertions.
    pub(super) fn smoke_network_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.smoke.as_ref().is_some_and(|smoke| {
            !smoke.network_input
                && smoke
                    .operation
                    .as_deref()
                    .is_some_and(|action| action.starts_with("network"))
        }) {
            return;
        }
        let Some(network) = self.network.clone() else {
            return;
        };
        if network.read(cx).loading {
            return;
        }
        let editors = {
            let view = network.read(cx);
            [
                view.branch.clone(),
                view.destination.clone(),
                view.url.clone(),
                view.initial_branch.clone(),
            ]
        };
        let inputs = [
            Self::network_fixture_input("HUNKRA_SMOKE_BRANCH"),
            Self::network_fixture_input("HUNKRA_SMOKE_DESTINATION"),
            Self::network_fixture_input("HUNKRA_SMOKE_URL"),
            Self::network_fixture_input("HUNKRA_SMOKE_INITIAL_BRANCH"),
        ];
        let mut preserved = Vec::new();
        for (editor, input) in editors.iter().zip(inputs) {
            editor.update(cx, |editor, cx| {
                editor.buffer.reset(String::new());
                editor.replace_and_mark_text_in_range(None, &input, Some(0..0), window, cx);
                editor.replace_text_in_range(None, &input, window, cx);
                let original = editor.buffer.text().to_owned();
                editor.replace_text_in_range(None, "multiline\nrefusal", window, cx);
                assert_eq!(editor.buffer.text(), original);
                editor.buffer.select(0, false);
                editor.buffer.select(
                    input
                        .char_indices()
                        .nth(3)
                        .map_or(input.len(), |(index, _)| index),
                    true,
                );
                cx.notify();
            });
            let buffer = &editor.read(cx).buffer;
            preserved.push((buffer.text().to_owned(), buffer.selection, buffer.revision));
        }
        let smoke = self.smoke.as_mut().unwrap();
        smoke.network_input = true;
        smoke.network_preserved = preserved;
    }
    /// Proves labels do not trigger reads or reset any visible/hidden network input.
    fn assert_network_inputs(&self, cx: &Context<Self>) {
        let Some(network) = &self.network else {
            return;
        };
        let view = network.read(cx);
        for (editor, preserved) in [
            &view.branch,
            &view.destination,
            &view.url,
            &view.initial_branch,
        ]
        .into_iter()
        .zip(&self.smoke.as_ref().unwrap().network_preserved)
        {
            let buffer = &editor.read(cx).buffer;
            assert_eq!(
                (buffer.text().to_owned(), buffer.selection, buffer.revision),
                *preserved
            );
        }
    }
    /// Drives real rendered remotes/creation handlers through the shared writer and verifies frozen inputs/close waiting.
    pub(super) fn smoke_network_tick(&mut self, cx: &mut Context<Self>) {
        let smoke = self.smoke.as_ref().unwrap();
        assert!(
            smoke.started.elapsed() < Duration::from_secs(30),
            "native network fixture deadline"
        );
        if self.frames <= smoke.last_frame {
            return;
        }
        let phase = smoke.phase;
        let action = smoke.operation.clone().unwrap();
        let draft = action.starts_with("network-draft");
        let creation = action.contains("init") || action.contains("clone");
        let cancelled = action.ends_with("cancel");
        if phase == 0 {
            if self.root.is_some()
                && (self.scanning || self.diff_loading || self.view.repository.is_none())
                || draft && !smoke.result_ready
            {
                return;
            }
            if !creation
                && !action.contains("pull")
                && let Some(diff) = self.view.diff.clone()
            {
                if let Some(line) = diff
                    .rows
                    .iter()
                    .find(|row| row.changed)
                    .and_then(|row| row.right.as_ref())
                {
                    self.toggle_line(false, line.number - 1, false, cx);
                }
                self.smoke.as_mut().unwrap().network_selection =
                    Some((diff, self.view.line_selection.clone()));
            }
            self.select_tab(3, cx);
            let page = if action.contains("init") {
                NetworkPage::Init
            } else if action.contains("clone") {
                NetworkPage::Clone
            } else {
                NetworkPage::Remotes
            };
            self.network.as_ref().unwrap().update(cx, |view, cx| {
                view.select_page(page, cx);
                if Self::network_fixture_input("HUNKRA_SMOKE_FORMAT") == "sha256" {
                    view.format = hunkra_core::network::ObjectFormat::Sha256;
                }
            });
        } else {
            let Some(network) = self.network.clone() else {
                if phase >= 5
                    && self.last_write_backend == Some("gix")
                    && !self.scanning
                    && self.view.repository.is_some()
                {
                    self.finish_network_smoke(cx);
                }
                return;
            };
            if network.read(cx).loading || !self.smoke.as_ref().unwrap().network_input {
                return;
            }
            if phase <= 3 {
                let generation = network.read(cx).generation;
                self.set_locale([Locale::En, Locale::Ru, Locale::RuDev][phase - 1], cx);
                self.assert_network_inputs(cx);
                assert_eq!(generation, network.read(cx).generation);
                let label = self.t("network");
                self.smoke.as_mut().unwrap().locales.push(label);
                if draft {
                    self.assert_network_draft(cx);
                }
            } else if phase == 4 {
                self.assert_network_inputs(cx);
                if creation {
                    network.update(cx, |view, cx| view.create(cx));
                } else {
                    let key = match action.as_str() {
                        "network-fetch" => "network-fetch",
                        "network-upstream" | "network-upstream-refusal" => "network-upstream",
                        "network-push" | "network-push-refusal" => "network-push",
                        "network-pull" | "network-pull-refusal" => "network-pull",
                        "network-draft-fetch" => "network-fetch",
                        "network-draft-pull-cancel" => "network-pull",
                        _ => panic!("unknown native network fixture"),
                    };
                    network.update(cx, |view, cx| view.action(key, cx));
                }
                let host = cx.entity();
                cx.defer(move |cx| {
                    host.update(cx, |app, cx| {
                        if draft && (creation || action.contains("pull")) {
                            assert!(
                                !app.writing
                                    && app.confirmation.as_ref().unwrap().kind
                                        == ConfirmationKind::Dirty
                            );
                            app.assert_network_draft(cx);
                            assert!(network.read(cx).busy);
                            if cancelled {
                                app.cancel_confirmation(cx);
                            } else {
                                app.save_before_intent(cx);
                            }
                            return;
                        }
                        assert!(app.writing);
                        assert!(network.read(cx).busy);
                        let before = network.read(cx).generation;
                        network.update(cx, |view, cx| {
                            view.reload(cx);
                            view.action("network-push", cx);
                            view.create(cx);
                        });
                        assert_eq!(before, network.read(cx).generation);
                        assert!(!network.read(cx).branch.read(cx).enabled);
                        if app.smoke.as_ref().unwrap().close_on_write {
                            assert!(!app.request_close(cx));
                        }
                    })
                });
            } else {
                if self.writing || self.scanning || self.diff_loading {
                    return;
                }
                self.assert_network_inputs(cx);
                assert!(!network.read(cx).busy && network.read(cx).branch.read(cx).enabled);
                if cancelled {
                    assert!(self.confirmation.is_none() && self.last_write_backend.is_none());
                    self.assert_network_draft(cx);
                } else if action.contains("refusal") {
                    assert!(self.write_error_key.is_some());
                    assert!(self.last_write_backend.is_none());
                    assert!(!self.close_requested);
                } else {
                    assert!(self.write_error_key.is_none());
                    assert_eq!(
                        self.last_write_backend,
                        Some(if action == "network-push" || action == "network-pull" {
                            "git-cli"
                        } else {
                            "gix"
                        })
                    );
                }
                if draft && !creation {
                    self.assert_network_draft(cx);
                }
                if let Some((diff, selection)) = &self.smoke.as_ref().unwrap().network_selection {
                    assert!(Arc::ptr_eq(diff, self.view.diff.as_ref().unwrap()));
                    assert_eq!(selection.added, self.view.line_selection.added);
                    assert_eq!(selection.removed, self.view.line_selection.removed);
                }
                assert_eq!(self.message.read(cx).buffer.text(), SMOKE_MESSAGE);
                self.finish_network_smoke(cx);
                return;
            }
        }
        let smoke = self.smoke.as_mut().unwrap();
        smoke.phase += 1;
        smoke.last_frame = self.frames;
        cx.notify();
    }
    /// Checks unsaved Result text/selection after remote reads, locale switches and cancelled project replacement.
    fn assert_network_draft(&self, cx: &Context<Self>) {
        let conflict = self.conflict.as_ref().unwrap().read(cx);
        let smoke = self.smoke.as_ref().unwrap();
        assert!(conflict.dirty(cx));
        assert_eq!(
            conflict.result.read(cx).buffer.text(),
            smoke.result_text.as_deref().unwrap()
        );
        assert_eq!(
            Some(conflict.result.read(cx).buffer.selection),
            smoke.result_selection
        );
    }
    /// Stores only owned result categories/refs and finite native evidence; URL/helper/config bytes are never reported.
    fn finish_network_smoke(&mut self, cx: &mut Context<Self>) {
        let smoke = self.smoke.as_ref().unwrap();
        let network = self.network.as_ref().map(|network| network.read(cx));
        let report = serde_json::json!({"ok":true,"native_display":true,"frames":self.frames,"network_frames":network.as_ref().map(|view| view.frames),
            "scenario":smoke.operation,"locales":smoke.locales,"inputs_retained":true,"writer_backend":self.last_write_backend,
            "error":self.write_error_key,"close_cancelled":!self.close_requested,"upstream":network.as_ref().and_then(|view| view.snapshot.as_ref()).and_then(|snapshot| snapshot.upstream.as_ref()).map(|upstream| (&upstream.remote,&upstream.branch))});
        std::fs::write(&smoke.report, serde_json::to_vec_pretty(&report).unwrap()).unwrap();
        cx.quit();
    }
}
