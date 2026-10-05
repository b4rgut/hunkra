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
    pub message: String,
    pub diff_generation: u64,
}
impl ViewState {
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
            } else if previous.kind == diff.kind && previous.presentation == diff.presentation {
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
        let diff = state.diff.clone().unwrap();
        for locale in [Locale::En, Locale::Ru, Locale::RuDev] {
            state.switch_locale(locale);
            assert!(Arc::ptr_eq(&diff, state.diff.as_ref().unwrap()));
            assert_eq!(state.message, "User commit message");
            assert!(state.selected_rows.contains(&0));
            assert_eq!(state.diff_generation, generation);
        }
    }
    #[test]
    /// Fast file switching cannot install a delayed response for the old file.
    fn stale_reply_is_ignored() {
        let mut state = ViewState::default();
        let old = state.select("old".into(), DiffKind::Unstaged);
        let current = state.select("new".into(), DiffKind::Unstaged);
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
