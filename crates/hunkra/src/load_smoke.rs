//! Finite native load acceptance; external fixture edits run off the UI thread and never activate in ordinary launches.
use super::*;
use std::sync::atomic::{AtomicBool, Ordering};

pub(super) struct LoadState {
    original: Arc<DiffSnapshot>,
    selection: LineSelection,
    initial_files: usize,
    external_done: Arc<AtomicBool>,
}
impl Hunkra {
    /// Checks actual shared alignment/coordinates and independently dirty Result after repeat reads and locale changes.
    fn assert_load_retention(&self, cx: &Context<Self>) {
        let load = self.load.as_ref().unwrap();
        assert!(Arc::ptr_eq(
            &load.original,
            self.view.diff.as_ref().unwrap()
        ));
        assert_eq!(self.view.line_selection.added, load.selection.added);
        assert_eq!(self.view.line_selection.removed, load.selection.removed);
        if self.smoke.as_ref().unwrap().operation.as_deref() == Some("load-draft") {
            let conflict = self.conflict.as_ref().unwrap().read(cx);
            assert!(conflict.dirty(cx));
            assert_eq!(
                conflict.result.read(cx).buffer.text(),
                self.smoke.as_ref().unwrap().result_text.as_deref().unwrap()
            );
            assert_eq!(
                Some(conflict.result.read(cx).buffer.selection),
                self.smoke.as_ref().unwrap().result_selection
            );
        }
    }
    /// Mutates only an explicit disposable smoke fixture on a finite background thread, representing an external editor/event storm.
    fn start_load_changes(&mut self, action: &str) {
        let root = self.root.clone().unwrap();
        let done = self.load.as_ref().unwrap().external_done.clone();
        let mass = action == "load-mass";
        let draft = action == "load-draft";
        std::thread::spawn(move || {
            if mass {
                for number in 0..3000 {
                    let path = root.join(format!("files/{number:04}.txt"));
                    let bytes = std::fs::read(&path).unwrap();
                    std::fs::write(path, bytes).unwrap();
                }
                for number in 0..1000 {
                    std::fs::write(
                        root.join(format!("untracked/extra-{number:04}.txt")),
                        b"new\n",
                    )
                    .unwrap();
                }
            } else {
                let path = root.join("f");
                let modified = std::fs::metadata(&path).unwrap().modified().unwrap();
                std::fs::write(
                    &path,
                    if draft {
                        b"external result\n".as_slice()
                    } else {
                        b"other\n".as_slice()
                    },
                )
                .unwrap();
                std::fs::File::options()
                    .write(true)
                    .open(path)
                    .unwrap()
                    .set_modified(modified)
                    .unwrap();
            }
            done.store(true, Ordering::Release);
        });
    }
    /// Exercises real native virtualization/reconciliation with finite deadlines; metrics clearly separate sources/alignment/visible preparation/callbacks/RSS.
    pub(super) fn smoke_load_tick(&mut self, cx: &mut Context<Self>) {
        let smoke = self.smoke.as_ref().unwrap();
        assert!(
            smoke.started.elapsed() < Duration::from_secs(30),
            "native load fixture deadline"
        );
        if self.frames <= smoke.last_frame
            || self.scanning
            || self.diff_loading
            || self.view.repository.is_none()
            || self.view.diff.is_none()
            || !smoke.input_prepared
        {
            return;
        }
        let phase = smoke.phase;
        let action = smoke.operation.clone().unwrap();
        if phase == 0 {
            if !self.watch.active() || action == "load-draft" && !smoke.result_ready {
                return;
            }
            let files = self.view.repository.as_ref().unwrap().files.len();
            assert_eq!(
                files,
                std::env::var("HUNKRA_SMOKE_EXPECT_FILES")
                    .unwrap()
                    .parse::<usize>()
                    .unwrap()
            );
            assert!(
                files > 4000
                    && self.measurements.file_rows > 0
                    && self.measurements.file_rows < 200
            );
            assert_eq!(
                self.view.selected.as_deref(),
                Some(std::path::Path::new("f"))
            );
            if let Some(line) = self
                .view
                .diff
                .as_ref()
                .unwrap()
                .rows
                .iter()
                .find(|row| row.changed)
                .and_then(|row| row.right.as_ref())
            {
                self.toggle_line(false, line.number - 1, false, cx);
            }
            self.load = Some(LoadState {
                original: self.view.diff.clone().unwrap(),
                selection: self.view.line_selection.clone(),
                initial_files: files,
                external_done: Arc::new(AtomicBool::new(false)),
            });
        } else if phase <= 3 {
            let generation = self.scan_generation;
            self.set_locale([Locale::En, Locale::Ru, Locale::RuDev][phase - 1], cx);
            assert_eq!(generation, self.scan_generation);
            self.assert_load_retention(cx);
            let label = self.t("changes");
            self.smoke.as_mut().unwrap().locales.push(label);
        } else if phase == 4 {
            self.refresh(cx);
        } else if phase == 5 {
            self.assert_load_retention(cx);
            assert!(self.measurements.cache.hits > 0);
        } else if phase == 6 {
            if action == "load-large" {
                self.select("large.txt".into(), cx);
            } else {
                if action == "load-periodic" {
                    self.watch
                        .reset(Some(self.root.as_ref().unwrap().join("missing-watch-root")));
                }
                self.start_load_changes(&action);
            }
        } else if action == "load-large" {
            if phase == 7 {
                assert_eq!(self.view.diff.as_ref().unwrap().rows.len(), 18000);
                self.diff_scroll
                    .scroll_to_item(9000, ScrollStrategy::Center);
                self.select("large.txt".into(), cx);
            } else if phase == 8 {
                assert!(self.measurements.diff_rows > 0 && self.measurements.diff_rows < 200);
                self.select("oversized".into(), cx);
            } else if phase == 9 {
                assert_eq!(
                    self.view.diff.as_ref().unwrap().presentation,
                    Presentation::TooLarge
                );
                assert!(self.view.diff.as_ref().unwrap().rows.is_empty());
                self.select("dense.txt".into(), cx);
            } else if phase == 10 {
                let diff = self.view.diff.as_ref().unwrap();
                assert_eq!(diff.presentation, Presentation::TooLarge);
                assert_eq!(diff.left.bytes.len(), hunkra_core::diff::MAX_BYTES);
                assert!(diff.left.bytes.iter().all(|byte| *byte == b'\n'));
                assert_eq!(diff.right.bytes, b"dense worktree\r\nEOF");
                assert!(diff.rows.is_empty() && diff.hunks.is_empty());
                for path in ["f", "sibling", "large.txt"] {
                    self.select(path.into(), cx);
                }
            } else {
                assert_eq!(
                    self.view.selected.as_deref(),
                    Some(std::path::Path::new("large.txt"))
                );
                assert_eq!(self.view.diff.as_ref().unwrap().rows.len(), 18000);
                self.finish_load_smoke(cx);
                return;
            }
        } else {
            let load = self.load.as_ref().unwrap();
            if !load.external_done.load(Ordering::Acquire) {
                return;
            }
            if action == "load-mass" {
                if self.view.repository.as_ref().unwrap().files.len() != load.initial_files + 1000 {
                    return;
                }
                self.assert_load_retention(cx);
            } else if action == "load-draft" {
                let view = self.conflict.as_ref().unwrap().read(cx);
                if !view.draft.stale {
                    return;
                }
                assert!(view.dirty(cx));
                assert_eq!(
                    view.result.read(cx).buffer.text(),
                    self.smoke.as_ref().unwrap().result_text.as_deref().unwrap()
                );
                assert_eq!(
                    Some(view.result.read(cx).buffer.selection),
                    self.smoke.as_ref().unwrap().result_selection
                );
            } else {
                if self.view.diff.as_ref().unwrap().right.bytes != b"other\n" {
                    return;
                }
                assert!(!Arc::ptr_eq(
                    &load.original,
                    self.view.diff.as_ref().unwrap()
                ));
                assert!(
                    self.view.line_selection.added.is_empty()
                        && self.view.line_selection.removed.is_empty()
                );
            }
            if action == "load-periodic" {
                assert!(self.watch.unavailable() && self.measurements.watch_hints == 0);
            } else {
                assert!(self.measurements.watch_hints > 0);
            }
            self.finish_load_smoke(cx);
            return;
        }
        let smoke = self.smoke.as_mut().unwrap();
        smoke.phase += 1;
        smoke.last_frame = self.frames;
        cx.notify();
    }
    /// Persists only finite evidence/owned metrics; native GPU execution and physical input are never inferred from these counters.
    fn finish_load_smoke(&mut self, cx: &mut Context<Self>) {
        assert!(self.last_write_backend.is_none());
        assert_eq!(self.message.read(cx).buffer.text(), SMOKE_MESSAGE);
        assert!(self.measurements.file_rows < 200 && self.measurements.diff_rows < 200);
        assert!(
            self.measurements.cache.entries <= hunkra_core::diff_reader::CACHE_ENTRIES
                && self.measurements.cache.allocated_bytes <= hunkra_core::diff_reader::CACHE_BYTES
        );
        let smoke = self.smoke.as_ref().unwrap();
        let report = serde_json::json!({"ok":true,"native_display":true,"frames":self.frames,"scenario":smoke.operation,"locales":smoke.locales,"initial_files":self.load.as_ref().unwrap().initial_files,"final_files":self.view.repository.as_ref().unwrap().files.len(),"message_retained":true,"draft_retained":smoke.operation.as_deref()==Some("load-draft"),"dense_limit_verified":smoke.operation.as_deref()==Some("load-large"),"watch_native":self.watch.active(),"measurements":self.measurements.json()});
        std::fs::write(&smoke.report, serde_json::to_vec_pretty(&report).unwrap()).unwrap();
        cx.quit();
    }
}
