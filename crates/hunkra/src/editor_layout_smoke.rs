//! Finite real Wayland text-layout acceptance; only an explicit isolated editor-layout scenario enters this path.
use super::*;

/// Produces the exact byte-bounded document requested by this finite test phase.
fn oversized_text(phase: usize) -> String {
    if phase < 4 {
        "🦀".repeat(8192)
    } else {
        "\n".repeat(2 * 1024 * 1024)
    }
}

impl Hunkra {
    /// Exercises native Result input, preview refusal, locale retention and full-text undo/redo without touching clipboard or Git writes.
    pub(super) fn smoke_editor_layout_input(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(smoke) = self
            .smoke
            .as_ref()
            .filter(|smoke| smoke.operation.as_deref() == Some("editor-layout"))
        else {
            return;
        };
        assert!(
            smoke.started.elapsed() < Duration::from_secs(25),
            "editor layout deadline: phase {}, input {}, scanning {}, diff_loading {}, conflict {}, selected {:?}, error {:?}",
            smoke.phase,
            smoke.input_prepared,
            self.scanning,
            self.diff_loading,
            self.conflict.is_some(),
            self.view.selected,
            self.error_key
        );
        if !smoke.input_prepared || self.scanning || self.diff_loading {
            return;
        }
        let phase = smoke.phase;
        let Some(conflict) = self.conflict.clone() else {
            return;
        };
        let editor = conflict.read(cx).result.clone();
        if phase == 0 || phase == 4 || phase == 6 {
            let text = if phase == 6 {
                "Снова🦀\r\nEOF".into()
            } else {
                oversized_text(phase)
            };
            editor.update(cx, |editor, cx| {
                window.focus(&editor.focus_handle(cx));
                editor.buffer.select(0, false);
                editor.buffer.select(editor.buffer.text().len(), true);
                editor.replace_text_in_range(None, &text, window, cx);
                assert_eq!(editor.buffer.text(), text);
            });
        } else if (1..=3).contains(&phase) {
            let text = editor.read(cx).buffer.text().to_owned();
            let selection = editor.read(cx).buffer.selection;
            let revision = editor.read(cx).buffer.revision;
            let generation = (self.scan_generation, self.view.diff_generation);
            let locale = [Locale::En, Locale::Ru, Locale::RuDev][phase - 1];
            self.set_locale(locale, cx);
            let notice = self.t("editor-preview-limit");
            editor.update(cx, |editor, cx| {
                assert_eq!(editor.buffer.text(), text);
                assert_eq!(editor.buffer.selection, selection);
                assert_eq!(editor.buffer.revision, revision);
                assert_eq!(editor.preview_notice.as_ref(), notice);
                assert!(
                    editor
                        .bounds_for_range(0..0, Bounds::default(), window, cx)
                        .is_none()
                );
            });
            assert_eq!(
                generation,
                (self.scan_generation, self.view.diff_generation)
            );
            let label = self.t("changes");
            self.smoke.as_mut().unwrap().locales.push(label);
        } else if phase == 5 || phase == 8 {
            editor.update(cx, |editor, cx| {
                assert_eq!(editor.buffer.text(), oversized_text(phase));
                assert!(
                    editor
                        .bounds_for_range(0..0, Bounds::default(), window, cx)
                        .is_none()
                );
                if phase == 8 {
                    editor.key_down(
                        &KeyDownEvent {
                            keystroke: Keystroke::parse("ctrl-shift-z").unwrap(),
                            is_held: false,
                        },
                        window,
                        cx,
                    );
                }
            });
        } else if phase == 7 {
            editor.update(cx, |editor, cx| {
                assert_eq!(editor.buffer.text(), "Снова🦀\r\nEOF");
                let end = editor.buffer.selected_utf16().0.end;
                assert!(
                    editor
                        .bounds_for_range(end..end, Bounds::default(), window, cx)
                        .is_some()
                );
                editor.key_down(
                    &KeyDownEvent {
                        keystroke: Keystroke::parse("ctrl-z").unwrap(),
                        is_held: false,
                    },
                    window,
                    cx,
                );
            });
        } else {
            assert_eq!(editor.read(cx).buffer.text(), "Снова🦀\r\nEOF");
            assert!(conflict.read(cx).dirty(cx));
            assert!(!self.writing);
            let smoke = self.smoke.as_ref().unwrap();
            let report = serde_json::json!({"ok":true,"native_display":true,"scenario":"editor-layout","frames":self.frames,"locales":smoke.locales,"giant_line_bytes":32768,"newline_document_bytes":2097152,"text_retained":true,"notice_has_no_source_coordinates":true,"replacement_undo_redo":true,"read_only":true,"scope":"actual native Result input/layout/locale/undo callbacks; owner clipboard and Git unchanged; physical input/pixels/GPU execution unmeasured"});
            std::fs::write(&smoke.report, serde_json::to_vec_pretty(&report).unwrap()).unwrap();
            cx.quit();
            return;
        }
        self.smoke.as_mut().unwrap().phase += 1;
        cx.notify();
    }
}
