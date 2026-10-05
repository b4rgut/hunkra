//! View state survives label changes and rejects stale service results.
use crate::i18n::Locale;
use hunkra_core::{DiffKind, DiffSnapshot, RepositorySnapshot};
use std::{collections::BTreeSet, path::PathBuf, sync::Arc};

#[derive(Default)]
pub struct ViewState {
    pub locale: Locale,
    pub repository: Option<RepositorySnapshot>,
    pub selected: Option<PathBuf>,
    pub diff: Option<Arc<DiffSnapshot>>,
    pub selected_rows: BTreeSet<usize>,
    pub line_selection: hunkra_core::LineSelection,
    pub message: String,
    pub diff_generation: u64,
}
impl ViewState {
    /// Gates callbacks from a rendered frame so old coordinates cannot target a newer document.
    pub fn is_current_diff(&self, rendered: &Arc<DiffSnapshot>) -> bool {
        self.diff
            .as_ref()
            .is_some_and(|current| Arc::ptr_eq(current, rendered))
    }
    /// Keeps retryable choices after a refusal; successful or stale writes invalidate coordinates.
    pub fn finish_index_write(
        &mut self,
        result: &Result<hunkra_core::IndexWrite, hunkra_core::CoreError>,
    ) -> bool {
        let invalidated = result.is_ok() || matches!(result, Err(hunkra_core::CoreError::Stale));
        if invalidated {
            self.line_selection = Default::default();
            self.selected_rows.clear();
            self.diff = None;
        }
        self.diff_generation += 1;
        invalidated
    }
    /// Changes only the label profile; requests and all document/selection state stay intact.
    pub fn switch_locale(&mut self, locale: Locale) {
        self.locale = locale;
    }
    /// Applies only the current selected-file request, ignoring delayed older replies.
    pub fn apply_diff(&mut self, generation: u64, diff: DiffSnapshot) -> bool {
        if generation != self.diff_generation || self.selected.as_ref() != Some(&diff.path) {
            return false;
        }
        if let Some(previous) = &self.diff {
            if previous.left != diff.left || previous.right != diff.right {
                self.selected_rows.clear();
                self.line_selection = Default::default();
            } else if previous.same_version(&diff) {
                return true;
            }
        }
        self.diff = Some(Arc::new(diff));
        true
    }
    /// Resets selection only for a genuinely different document or comparison.
    pub fn select(&mut self, path: PathBuf, kind: DiffKind) -> u64 {
        if self.selected.as_ref() != Some(&path)
            || self.diff.as_ref().is_some_and(|diff| diff.kind != kind)
        {
            self.diff = None;
            self.selected_rows.clear();
            self.line_selection = Default::default();
        }
        self.selected = Some(path);
        self.diff_generation += 1;
        self.diff_generation
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use hunkra_core::Document;
    #[test]
    /// Retryable refusal keeps immutable coordinates; stale/success invalidates them before refresh.
    fn write_refusal_preserves_selection_until_sources_change() {
        use hunkra_core::{CoreError, IndexWrite};
        let mut state = ViewState::default();
        let generation = state.select("f".into(), DiffKind::Unstaged);
        state.apply_diff(
            generation,
            DiffSnapshot::new(
                "f".into(),
                DiffKind::Unstaged,
                Document::present(b"old\n".to_vec(), None),
                Document::present(b"new\n".to_vec(), None),
            ),
        );
        state.line_selection.added.insert(0);
        let rendered = state.diff.clone().unwrap();
        for error in [
            CoreError::IndexLock,
            CoreError::SelectionBoundary,
            CoreError::UnsupportedWrite,
        ] {
            assert!(!state.finish_index_write(&Err(error)));
            assert!(state.is_current_diff(&rendered));
            assert!(state.line_selection.added.contains(&0));
        }
        let generation = state.diff_generation;
        let unchanged = rendered.as_ref().clone();
        assert!(state.apply_diff(generation, unchanged));
        assert!(state.is_current_diff(&rendered));
        assert!(state.line_selection.added.contains(&0));
        assert!(state.finish_index_write(&Err(CoreError::Stale)));
        assert!(state.diff.is_none() && state.line_selection.added.is_empty());
        state.line_selection.added.insert(0);
        assert!(state.finish_index_write(&Ok(IndexWrite { backend: "gix" })));
        assert!(state.line_selection.added.is_empty());
    }
    #[test]
    /// Runtime locale changes preserve file, selection, diff identity and message with no new job.
    fn locale_preserves_state() {
        let mut state = ViewState::default();
        let path = PathBuf::from("f");
        let generation = state.select(path.clone(), DiffKind::Unstaged);
        state.apply_diff(
            generation,
            DiffSnapshot::new(
                path,
                DiffKind::Unstaged,
                Document::absent(),
                Document::present(b"a\n".to_vec(), None),
            ),
        );
        state.message = "User commit message".into();
        state.selected_rows.insert(0);
        state.line_selection.added.insert(0);
        let diff = state.diff.clone().unwrap();
        for locale in [Locale::En, Locale::Ru, Locale::RuDev] {
            state.switch_locale(locale);
            assert!(Arc::ptr_eq(&diff, state.diff.as_ref().unwrap()));
            assert!(state.is_current_diff(&diff));
            assert_eq!(state.message, "User commit message");
            assert!(state.selected_rows.contains(&0));
            assert!(state.line_selection.added.contains(&0));
            assert_eq!(state.diff_generation, generation);
        }
    }
    #[test]
    /// Fast file switching cannot install a delayed response for the old file.
    fn stale_reply_is_ignored() {
        let mut state = ViewState::default();
        let old = state.select("old".into(), DiffKind::Unstaged);
        state.apply_diff(
            old,
            DiffSnapshot::new(
                "old".into(),
                DiffKind::Unstaged,
                Document::absent(),
                Document::present(b"old\n".to_vec(), None),
            ),
        );
        let rendered = state.diff.clone().unwrap();
        let current = state.select("new".into(), DiffKind::Unstaged);
        assert!(!state.is_current_diff(&rendered));
        let diff = DiffSnapshot::new(
            "old".into(),
            DiffKind::Unstaged,
            Document::absent(),
            Document::absent(),
        );
        assert!(!state.apply_diff(old, diff));
        assert!(state.diff.is_none());
        assert_eq!(state.diff_generation, current);
    }
}
