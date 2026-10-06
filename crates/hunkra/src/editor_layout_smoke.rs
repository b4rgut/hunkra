//! Finite real Wayland text-layout acceptance; only an explicit isolated editor-layout scenario enters this path.
use super::*;
use hunkra_core::diff::MAX_BYTES;

/// Produces the exact byte-bounded document requested by this finite test phase.
fn oversized_text(phase: usize) -> String {
    if phase < 4 {
        "🦀".repeat(8192)
    } else {
        "\n".repeat(2 * 1024 * 1024)
    }
}

/// Fits the supported native/core row and byte budgets while requiring a substantial unchanged Result scan.
fn large_marked_text() -> String {
    let mut text = "unchanged text Кириллица 🦀\r\n".repeat(39_994);
    text.push_str("<<<<<<< A\r\nТекущая🦀\r\n=======\r\nВходящая🦀\r\n>>>>>>> B\r\n");
    assert!(text.len() < MAX_BYTES);
    text
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
        } else if phase == 9 {
            assert_eq!(editor.read(cx).buffer.text(), "Снова🦀\r\nEOF");
            self.smoke.as_mut().unwrap().marker_summary_before =
                Some(conflict.read(cx).chunk_measurements());
            let measurements = editor.update(cx, |editor, cx| {
                editor.buffer.replace_document(&large_marked_text());
                let end = editor.buffer.text().len();
                editor.buffer.select(end, false);
                let started = std::time::Instant::now();
                let native_end = editor.selected_text_range(true, window, cx).unwrap().range.end;
                let first = started.elapsed();
                let started = std::time::Instant::now();
                for _ in 0..32 {
                    assert_eq!(editor.selected_text_range(true, window, cx).unwrap().range, native_end..native_end);
                    let mut adjusted = None;
                    assert_eq!(editor.text_for_range(native_end-3..native_end, &mut adjusted, window, cx).as_deref(), Some("B\r\n"));
                    assert_eq!(adjusted, Some(native_end-3..native_end));
                }
                let repeated = started.elapsed();
                editor.buffer.select(0, false);
                cx.notify();
                serde_json::json!({"text_bytes":end,"first_selection_query_ms":first.as_secs_f64()*1000.,"repeated_pairs":32,"repeated_selection_and_text_ms":repeated.as_secs_f64()*1000.,"scope":"actual native UTF-16 input-query callback CPU-code wall time at EOF; first request after text revision and unchanged repeated requests separated; text/selection restored; painting/scheduling/GPU excluded"})
            });
            self.smoke.as_mut().unwrap().utf16_queries = Some(measurements);
        } else if (10..=25).contains(&phase) {
            let selection = editor.read(cx).buffer.selection;
            let revision = editor.read(cx).buffer.revision;
            if phase <= 12 {
                self.set_locale([Locale::En, Locale::Ru, Locale::RuDev][phase - 10], cx);
            }
            if phase == 11 {
                let bounds = editor.update(cx, |editor, cx| {
                    editor
                        .bounds_for_range(0..0, Bounds::default(), window, cx)
                        .unwrap()
                });
                editor.update(cx, |editor, cx| {
                    editor.scroll_wheel(
                        &gpui::ScrollWheelEvent {
                            position: bounds.origin + gpui::point(px(1.), px(1.)),
                            delta: gpui::ScrollDelta::Lines(gpui::point(0., -5.)),
                            ..Default::default()
                        },
                        window,
                        cx,
                    );
                });
            } else if phase == 12 {
                editor.update(cx, |editor, cx| {
                    assert!(
                        editor
                            .bounds_for_range(0..0, Bounds::default(), window, cx)
                            .is_none(),
                        "wheel left the initial Result row visible"
                    );
                });
            } else if phase == 13 {
                editor.update(cx, |editor, cx| {
                    editor.key_down(
                        &KeyDownEvent {
                            keystroke: Keystroke::parse("ctrl-home").unwrap(),
                            is_held: false,
                        },
                        window,
                        cx,
                    );
                });
            } else if phase == 14 {
                editor.update(cx, |editor, cx| {
                    assert!(
                        editor
                            .bounds_for_range(0..0, Bounds::default(), window, cx)
                            .is_some(),
                        "explicit Home did not restore Result source coordinates"
                    );
                });
            } else if phase == 15 || phase == 16 {
                editor.update(cx, |editor, cx| {
                    if phase == 16 {
                        let cursor = editor.buffer.selected_utf16().0.end;
                        assert!(
                            editor
                                .bounds_for_range(cursor..cursor, Bounds::default(), window, cx)
                                .is_some()
                        );
                        assert!(
                            editor
                                .bounds_for_range(0..0, Bounds::default(), window, cx)
                                .is_none()
                        );
                    }
                    editor.key_down(
                        &KeyDownEvent {
                            keystroke: Keystroke::parse(if phase == 15 {
                                "pagedown"
                            } else {
                                "pageup"
                            })
                            .unwrap(),
                            is_held: false,
                        },
                        window,
                        cx,
                    );
                    assert!(editor.buffer.selection.range().is_empty());
                    assert_eq!(editor.buffer.selection.cursor > 0, phase == 15);
                });
            } else if phase == 17 {
                editor.update(cx, |editor, cx| {
                    assert!(
                        editor
                            .bounds_for_range(0..0, Bounds::default(), window, cx)
                            .is_some()
                    );
                    for key in ["shift-pagedown", "shift-pageup"] {
                        editor.key_down(
                            &KeyDownEvent {
                                keystroke: Keystroke::parse(key).unwrap(),
                                is_held: false,
                            },
                            window,
                            cx,
                        );
                        assert_eq!(
                            !editor.buffer.selection.range().is_empty(),
                            key == "shift-pagedown"
                        );
                        assert_eq!(editor.buffer.selection.anchor, 0);
                    }
                });
            } else if phase == 18 {
                let measurements = editor.update(cx, |editor, cx| {
                    let end = editor.buffer.text().len();
                    editor.key_down(&KeyDownEvent { keystroke: Keystroke::parse("ctrl-end").unwrap(), is_held: false }, window, cx);
                    let started = std::time::Instant::now();
                    for _ in 0..16 {
                        for key in ["left", "right"] {
                            editor.key_down(&KeyDownEvent { keystroke: Keystroke::parse(key).unwrap(), is_held: false }, window, cx);
                        }
                        assert_eq!(editor.buffer.selection.cursor, end);
                    }
                    let elapsed = started.elapsed();
                    editor.key_down(&KeyDownEvent { keystroke: Keystroke::parse("ctrl-home").unwrap(), is_held: false }, window, cx);
                    serde_json::json!({"text_bytes":end,"handler_calls":32,"total_ms":elapsed.as_secs_f64()*1000.,"scope":"actual native left/right handler CPU-code wall time at end of unchanged Result; painting/scheduling/GPU excluded"})
                });
                self.smoke.as_mut().unwrap().grapheme_navigation = Some(measurements);
            } else if phase == 19 {
                editor.update(cx, |editor, cx| {
                    let end = editor.buffer.text().len();
                    for key in ["ctrl-end", "ctrl-left", "ctrl-shift-right"] {
                        editor.key_down(
                            &KeyDownEvent {
                                keystroke: Keystroke::parse(key).unwrap(),
                                is_held: false,
                            },
                            window,
                            cx,
                        );
                        if key == "ctrl-left" {
                            assert_eq!(editor.buffer.selection.cursor, end - 3);
                        } else if key == "ctrl-shift-right" {
                            assert_eq!(editor.buffer.selection.range(), end - 3..end - 2);
                            assert_eq!(&editor.buffer.text()[editor.buffer.selection.range()], "B");
                        }
                    }
                    editor.key_down(
                        &KeyDownEvent {
                            keystroke: Keystroke::parse("ctrl-home").unwrap(),
                            is_held: false,
                        },
                        window,
                        cx,
                    );
                });
            }
            if phase != 15 && phase != 16 {
                assert_eq!(editor.read(cx).buffer.selection, selection);
            }
            assert_eq!(editor.read(cx).buffer.revision, revision);
            assert!(editor.read(cx).buffer.text().ends_with(">>>>>>> B\r\n"));
        } else {
            assert_eq!(editor.read(cx).buffer.text(), large_marked_text());
            assert!(conflict.read(cx).dirty(cx));
            assert!(!self.writing);
            let smoke = self.smoke.as_ref().unwrap();
            let report = serde_json::json!({"ok":true,"native_display":true,"scenario":"editor-layout","frames":self.frames,"locales":smoke.locales,"giant_line_bytes":32768,"newline_document_bytes":2097152,"text_retained":true,"notice_has_no_source_coordinates":true,"replacement_undo_redo":true,"large_result_bytes":large_marked_text().len(),"stable_result_phases":16,"marker_summary_before":smoke.marker_summary_before,"marker_summary":conflict.read(cx).chunk_measurements(),"grapheme_navigation":smoke.grapheme_navigation,"utf16_queries":smoke.utf16_queries,"word_navigation_preserves_graphemes_and_revision":true,"wheel_preserves_text_cursor_and_revision":true,"explicit_home_restores_source_coordinates":true,"page_navigation_preserves_text_revision":true,"page_selection_and_source_coordinates":true,"read_only":true,"scope":"actual native Result input/layout/locale/undo/scroll/page/word handler callbacks; registered key/wheel event dispatch is checked separately on isolated GPUI; owner clipboard and Git unchanged; physical input/pixels/GPU execution unmeasured"});
            std::fs::write(&smoke.report, serde_json::to_vec_pretty(&report).unwrap()).unwrap();
            cx.quit();
            return;
        }
        self.smoke.as_mut().unwrap().phase += 1;
        cx.notify();
    }
}
