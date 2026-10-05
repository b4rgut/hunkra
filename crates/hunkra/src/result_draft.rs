//! Result persistence state is independent of native text selection, undo and label changes.
use hunkra_core::Document;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DraftRefresh {
    Keep,
    Replace,
    Stale,
}

pub struct ResultDraft {
    saved: Document,
    pub present: bool,
    pub mode: Option<u32>,
    pub stale: bool,
    pub awaiting_refresh: bool,
}

impl ResultDraft {
    /// Initializes exact file presence independently of empty text and the eventual index action.
    pub fn new(saved: Document, fallback_mode: Option<u32>) -> Self {
        Self {
            present: saved.present,
            mode: saved.mode.or(fallback_mode).or(Some(0o100644)),
            saved,
            stale: false,
            awaiting_refresh: false,
        }
    }
    /// Returns the user's explicit keep/delete choice without normalizing text or line endings.
    pub fn document(&self, text: &str) -> Document {
        if self.present {
            Document::present(text.as_bytes().to_vec(), self.mode)
        } else {
            Document::absent()
        }
    }
    /// Detects an unsaved Result without treating an absent file as an empty file.
    pub fn dirty(&self, text: &str) -> bool {
        self.present != self.saved.present
            || (self.present
                && (text.as_bytes() != self.saved.bytes || self.mode != self.saved.mode))
    }
    /// Records confirmed filesystem persistence while retaining the editor's selection and undo.
    pub fn saved(&mut self, document: Document) {
        self.saved = document;
        self.stale = false;
        self.awaiting_refresh = true;
    }
    /// A refresh never overwrites a dirty document; changed sources invalidate its write precondition.
    pub fn refresh(
        &mut self,
        fresh: &Document,
        same_sources: bool,
        same_operation: bool,
        text: &str,
    ) -> DraftRefresh {
        if self.dirty(text) && (!same_sources || !same_operation || fresh != &self.saved) {
            self.stale = true;
            return DraftRefresh::Stale;
        }
        self.awaiting_refresh = false;
        self.stale = false;
        if same_sources && same_operation && (self.dirty(text) || fresh == &self.saved) {
            return DraftRefresh::Keep;
        }
        self.saved = fresh.clone();
        self.present = fresh.present;
        self.mode = fresh.mode.or(self.mode);
        DraftRefresh::Replace
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::text::TextBuffer;

    #[test]
    /// Status/locale refresh retain exact unsaved Unicode bytes, selection and undo coordinates.
    fn unsaved_refresh_keeps_text_history_and_refuses_changed_disk() {
        let saved = Document::present(b"initial\r\n".to_vec(), Some(0o100644));
        let mut draft = ResultDraft::new(saved.clone(), None);
        let mut buffer = TextBuffer::new(4096);
        buffer.reset("initial\r\n".into());
        buffer.replace(None, "текст 🦀");
        let selection = buffer.selection;
        assert_eq!(
            draft.refresh(&saved, true, true, buffer.text()),
            DraftRefresh::Keep
        );
        assert_eq!(buffer.selection, selection);
        let external = Document::present(b"external\n".to_vec(), Some(0o100644));
        assert_eq!(
            draft.refresh(&external, true, true, buffer.text()),
            DraftRefresh::Stale
        );
        assert!(draft.stale);
        assert!(buffer.text().ends_with("текст 🦀"));
        assert!(buffer.undo());
        assert_eq!(buffer.text(), "initial\r\n");
        assert!(buffer.redo());
        assert!(buffer.text().ends_with("текст 🦀"));
    }

    #[test]
    /// Saved Result remains undoable and empty-file/delete choices remain different.
    fn saved_refresh_and_presence_are_separate() {
        let saved = Document::present(b"before".to_vec(), Some(0o100644));
        let mut draft = ResultDraft::new(saved, None);
        let mut buffer = TextBuffer::new(4096);
        buffer.reset("before".into());
        buffer.replace(Some(0..6), "after");
        let persisted = draft.document(buffer.text());
        draft.saved(persisted.clone());
        assert!(!draft.dirty(buffer.text()));
        assert!(draft.awaiting_refresh);
        assert_eq!(
            draft.refresh(&persisted, true, true, buffer.text()),
            DraftRefresh::Keep
        );
        assert!(!draft.awaiting_refresh);
        assert!(buffer.undo());
        assert!(draft.dirty(buffer.text()));
        draft.present = false;
        assert!(!draft.document(buffer.text()).present);
        draft.present = true;
        assert!(draft.document("").present);
        assert_eq!(
            draft.refresh(&persisted, false, true, buffer.text()),
            DraftRefresh::Stale
        );
    }
}
