//! Owned Unicode text, selection, IME composition and bounded undo, independent of GPUI.
use std::ops::Range;
use unicode_segmentation::GraphemeCursor;

const MAX_UNDO_BYTES: usize = 8 * 1024 * 1024;
const MAX_UNDO_EDITS: usize = 256;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Selection {
    pub anchor: usize,
    pub cursor: usize,
}
impl Selection {
    /// Returns ascending UTF-8 byte bounds while retaining the active cursor separately.
    pub fn range(self) -> Range<usize> {
        self.anchor.min(self.cursor)..self.anchor.max(self.cursor)
    }
}

#[derive(Debug)]
struct Edit {
    start: usize,
    removed: String,
    inserted: String,
    before: Selection,
    after: Selection,
}
#[derive(Clone, Debug)]
struct Composition {
    text: String,
    selection: Selection,
}

#[derive(Debug)]
pub struct TextBuffer {
    text: String,
    pub selection: Selection,
    pub marked: Option<Range<usize>>,
    composition: Option<Composition>,
    undo: Vec<Edit>,
    redo: Vec<Edit>,
    limit: usize,
    pub revision: u64,
}
impl TextBuffer {
    /// Creates an empty editable document with an explicit byte limit.
    pub fn new(limit: usize) -> Self {
        Self {
            text: String::new(),
            selection: Selection::default(),
            marked: None,
            composition: None,
            undo: Vec::new(),
            redo: Vec::new(),
            limit,
            revision: 0,
        }
    }
    /// Exposes original UTF-8 bytes without normalizing line endings.
    pub fn text(&self) -> &str {
        &self.text
    }
    /// Loads or clears a document, invalidating the previous document's undo coordinates.
    pub fn reset(&mut self, text: String) -> bool {
        if text.len() > self.limit || text.contains('\0') {
            return false;
        }
        self.text = text;
        self.selection = Selection {
            anchor: self.text.len(),
            cursor: self.text.len(),
        };
        self.marked = None;
        self.composition = None;
        self.undo.clear();
        self.redo.clear();
        self.revision += 1;
        true
    }
    /// Clamps a requested UTF-8 offset to the preceding complete Unicode scalar.
    pub fn boundary(&self, byte: usize) -> usize {
        let mut byte = byte.min(self.text.len());
        while !self.text.is_char_boundary(byte) {
            byte -= 1;
        }
        byte
    }
    /// Converts a valid byte offset to the native input system's UTF-16 coordinate.
    pub fn utf16_offset(&self, byte: usize) -> usize {
        self.text[..self.boundary(byte)].encode_utf16().count()
    }
    /// Converts UTF-16 offsets, rounding an interior surrogate pair toward the requested edge.
    fn byte_offset(&self, utf16: usize, end: bool) -> usize {
        let mut units = 0;
        for (byte, ch) in self.text.char_indices() {
            if units >= utf16 {
                return byte;
            }
            if units + ch.len_utf16() > utf16 {
                return byte + if end { ch.len_utf8() } else { 0 };
            }
            units += ch.len_utf16();
        }
        self.text.len()
    }
    /// Adjusts native ranges to complete UTF-8 scalars, never splitting an emoji surrogate pair.
    pub fn from_utf16(&self, range: Range<usize>) -> Range<usize> {
        let start = self.byte_offset(range.start, false);
        if range.is_empty() {
            return start..start;
        }
        start..self.byte_offset(range.end.max(range.start), true)
    }
    /// Reports the selected native range with its original direction.
    pub fn selected_utf16(&self) -> (Range<usize>, bool) {
        let range = self.selection.range();
        (
            self.utf16_offset(range.start)..self.utf16_offset(range.end),
            self.selection.cursor < self.selection.anchor,
        )
    }
    /// Moves or extends a selection to an existing scalar boundary.
    pub fn select(&mut self, byte: usize, extend: bool) {
        let byte = self.boundary(byte);
        if !extend {
            self.selection.anchor = byte;
        }
        self.selection.cursor = byte;
    }
    /// Seeks the next extended-grapheme boundary from the cursor, supplying complete Unicode context without allocating a prefix index.
    pub fn next(&self, byte: usize) -> usize {
        GraphemeCursor::new(self.boundary(byte), self.text.len(), true)
            .next_boundary(&self.text, 0)
            .expect("complete UTF-8 text supplies grapheme context")
            .unwrap_or(self.text.len())
    }
    /// Seeks the previous extended boundary with global context; an interior scalar offset rounds forward to preserve existing navigation semantics.
    pub fn previous(&self, byte: usize) -> usize {
        let mut byte = byte.min(self.text.len());
        while !self.text.is_char_boundary(byte) {
            byte += 1;
        }
        GraphemeCursor::new(byte, self.text.len(), true)
            .prev_boundary(&self.text, 0)
            .expect("complete UTF-8 text supplies grapheme context")
            .unwrap_or(0)
    }
    /// Bounds history by retained edit bytes and count; an oversized edit stays undoable alone.
    fn remember(&mut self, edit: Edit) {
        if edit.removed == edit.inserted {
            return;
        }
        self.redo.clear();
        self.undo.push(edit);
        while self.undo.len() > 1
            && (self.undo.len() > MAX_UNDO_EDITS
                || self
                    .undo
                    .iter()
                    .map(|edit| edit.removed.len() + edit.inserted.len())
                    .sum::<usize>()
                    > MAX_UNDO_BYTES)
        {
            self.undo.remove(0);
        }
    }
    /// Replaces a byte range exactly, rejecting oversized or invalid input before any change.
    fn replace_bytes(&mut self, range: Range<usize>, text: &str, remember: bool) -> bool {
        let range = self.boundary(range.start)..self.boundary(range.end.max(range.start));
        if text.contains('\0') || self.text.len() - range.len() + text.len() > self.limit {
            return false;
        }
        let before = self.selection;
        let removed = self.text[range.clone()].to_owned();
        self.text.replace_range(range.clone(), text);
        self.select(range.start + text.len(), false);
        if remember {
            self.remember(Edit {
                start: range.start,
                removed,
                inserted: text.to_owned(),
                before,
                after: self.selection,
            });
        }
        self.revision += 1;
        true
    }
    /// Inserts ordinary text or finalizes a native composition as one undoable change.
    pub fn replace(&mut self, utf16: Option<Range<usize>>, text: &str) -> bool {
        let range = utf16
            .map(|range| self.from_utf16(range))
            .or_else(|| self.marked.clone())
            .unwrap_or_else(|| self.selection.range());
        let composing = self.composition.is_some();
        if !self.replace_bytes(range, text, !composing) {
            return false;
        }
        self.unmark();
        true
    }
    /// Replaces a full document with one minimal scalar-safe undo edit, retaining unchanged bytes.
    pub fn replace_document(&mut self, text: &str) -> bool {
        if text == self.text || text.len() > self.limit || text.contains('\0') {
            return false;
        }
        self.unmark();
        let prefix = self.boundary(
            self.text
                .bytes()
                .zip(text.bytes())
                .take_while(|(a, b)| a == b)
                .count(),
        );
        let mut suffix = self.text[prefix..]
            .bytes()
            .rev()
            .zip(text[prefix..].bytes().rev())
            .take_while(|(a, b)| a == b)
            .count();
        while !self.text.is_char_boundary(self.text.len() - suffix)
            || !text.is_char_boundary(text.len() - suffix)
        {
            suffix -= 1;
        }
        self.replace_bytes(
            prefix..self.text.len() - suffix,
            &text[prefix..text.len() - suffix],
            true,
        )
    }
    /// Updates marked text; the native selected range is relative to the inserted text alone.
    pub fn compose(
        &mut self,
        utf16: Option<Range<usize>>,
        text: &str,
        selected: Option<Range<usize>>,
    ) -> bool {
        let range = utf16
            .map(|range| self.from_utf16(range))
            .or_else(|| self.marked.clone())
            .unwrap_or_else(|| self.selection.range());
        let initial = self.composition.is_none();
        if initial {
            self.composition = Some(Composition {
                text: self.text.clone(),
                selection: self.selection,
            });
        }
        if !self.replace_bytes(range.clone(), text, false) {
            if initial {
                self.composition = None;
            }
            return false;
        }
        self.marked = Some(range.start..range.start + text.len());
        if let Some(selected) = selected {
            let mut inserted = Self::new(text.len());
            inserted.text = text.to_owned();
            let selected = inserted.from_utf16(selected);
            self.selection = Selection {
                anchor: range.start + selected.start,
                cursor: range.start + selected.end,
            };
        }
        true
    }
    /// Commits one composition revision using a minimal scalar-safe edit, preserving undo selection.
    pub fn unmark(&mut self) {
        self.marked = None;
        let Some(before) = self.composition.take() else {
            return;
        };
        let start = before
            .text
            .chars()
            .zip(self.text.chars())
            .take_while(|(a, b)| a == b)
            .map(|(ch, _)| ch.len_utf8())
            .sum::<usize>();
        let suffix = before.text[start..]
            .chars()
            .rev()
            .zip(self.text[start..].chars().rev())
            .take_while(|(a, b)| a == b)
            .map(|(ch, _)| ch.len_utf8())
            .sum::<usize>();
        self.remember(Edit {
            start,
            removed: before.text[start..before.text.len() - suffix].to_owned(),
            inserted: self.text[start..self.text.len() - suffix].to_owned(),
            before: before.selection,
            after: self.selection,
        });
    }
    /// Restores exact previous bytes and selection; no encoding or newline conversion occurs.
    pub fn undo(&mut self) -> bool {
        self.unmark();
        let Some(edit) = self.undo.pop() else {
            return false;
        };
        self.text
            .replace_range(edit.start..edit.start + edit.inserted.len(), &edit.removed);
        self.selection = edit.before;
        self.redo.push(edit);
        self.revision += 1;
        true
    }
    /// Reapplies an undone edit, retaining its original post-edit selection.
    pub fn redo(&mut self) -> bool {
        self.unmark();
        let Some(edit) = self.redo.pop() else {
            return false;
        };
        self.text
            .replace_range(edit.start..edit.start + edit.removed.len(), &edit.inserted);
        self.selection = edit.after;
        self.undo.push(edit);
        self.revision += 1;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use unicode_segmentation::UnicodeSegmentation;
    #[test]
    /// Minimal document edits stay scalar-safe, bounded and reversible with the original selection.
    fn document_choice_preserves_unicode_context_and_undo() {
        for (original, desired) in [
            ("prefix é\r\nконец 🦀", "prefix ê\r\nконец 🦀"),
            ("🙂common👩‍💻", "🙃common👩‍💻"),
            ("\u{feff}A\r\nБ", "\u{feff}A\r\n"),
            ("", "кириллица"),
            ("é", ""),
        ] {
            let mut buffer = TextBuffer::new(1024);
            buffer.reset(original.into());
            buffer.select(original.len(), false);
            buffer.select(0, true);
            let selection = buffer.selection;
            assert!(buffer.replace_document(desired));
            assert_eq!(buffer.text(), desired);
            assert!(buffer.undo());
            assert_eq!(buffer.text(), original);
            assert_eq!(buffer.selection, selection);
            assert!(buffer.redo());
            assert_eq!(buffer.text(), desired);
            let revision = buffer.revision;
            assert!(!buffer.replace_document(desired));
            assert!(!buffer.replace_document("\0"));
            assert!(!buffer.replace_document(&"x".repeat(1025)));
            assert_eq!(buffer.revision, revision);
        }
        let context = "контекст\r\n".repeat(10_000);
        let mut buffer = TextBuffer::new(2 * context.len() + 1);
        buffer.reset(format!("{context}A{context}"));
        assert!(buffer.replace_document(&format!("{context}B{context}")));
        let edit = buffer.undo.last().unwrap();
        assert_eq!((&*edit.removed, &*edit.inserted), ("A", "B"));
    }
    #[test]
    /// Every byte position retains global extended-grapheme boundaries, including scalar interiors, Unicode context and offsets beyond EOF.
    fn cursor_navigation_matches_extended_grapheme_boundaries() {
        for text in [
            "",
            "AБ\r\nе\u{301}👩\u{200d}💻🧑🏽\u{200d}🚀Z",
            "🇦🇧🇨🇩🇪🇫🇬",
            "क्\u{200d}षक्ष\u{600}A각\r\n",
        ] {
            let mut buffer = TextBuffer::new(65536);
            assert!(buffer.reset(text.into()));
            let starts = text
                .grapheme_indices(true)
                .map(|(offset, _)| offset)
                .collect::<Vec<_>>();
            let selection = buffer.selection;
            let revision = buffer.revision;
            for byte in (0..=text.len() + 4).chain(std::iter::once(usize::MAX)) {
                let next = starts
                    .iter()
                    .copied()
                    .find(|offset| *offset > byte)
                    .unwrap_or(text.len());
                let previous = starts
                    .iter()
                    .copied()
                    .take_while(|offset| *offset < byte)
                    .last()
                    .unwrap_or(0);
                assert_eq!(buffer.next(byte), next, "next at{byte} in{text:?}");
                assert_eq!(
                    buffer.previous(byte),
                    previous,
                    "previous at{byte} in{text:?}"
                );
            }
            assert_eq!(buffer.text(), text);
            assert_eq!(buffer.selection, selection);
            assert_eq!(buffer.revision, revision);
        }
    }
    #[test]
    /// Cyrillic, combining marks, joined emoji and CRLF survive grapheme edits plus undo/redo.
    fn unicode_edit_roundtrip() {
        let original = "строка\r\nе\u{301} 👩‍💻";
        let mut buffer = TextBuffer::new(65536);
        assert!(buffer.reset(original.into()));
        let start = buffer.previous(buffer.text.len());
        assert_eq!(&buffer.text[start..], "👩‍💻");
        buffer.select(start, true);
        assert!(buffer.replace(None, "🦀"));
        assert!(buffer.undo());
        assert_eq!(buffer.text(), original);
        assert!(buffer.redo());
        assert!(buffer.text().ends_with('🦀'));
        assert!(buffer.undo());
        buffer.select("строка\r\nе\u{301}".len(), false);
        assert_eq!(
            &buffer.text[buffer.previous(buffer.selection.cursor)..buffer.selection.cursor],
            "е\u{301}"
        );
    }
    #[test]
    /// UTF-16 ranges round surrogate interiors safely and marked selections stay insertion-relative.
    fn composition_has_one_undo_revision() {
        let mut buffer = TextBuffer::new(65536);
        buffer.reset("A🙂B\r\nконец".into());
        assert_eq!(buffer.from_utf16(2..3), 1..5);
        assert_eq!(buffer.from_utf16(2..2), 1..1);
        buffer.select(1, false);
        buffer.select(5, true);
        let initial = buffer.selection;
        assert!(buffer.compose(None, "е", Some(1..1)));
        assert!(buffer.compose(None, "е\u{301}", Some(0..2)));
        assert_eq!(buffer.selection.range(), 1..5);
        assert_eq!(buffer.marked, Some(1..5));
        assert!(buffer.replace(None, "ё🙂"));
        assert_eq!(buffer.text(), "Aё🙂B\r\nконец");
        assert!(buffer.undo());
        assert_eq!(buffer.text(), "A🙂B\r\nконец");
        assert_eq!(buffer.selection, initial);
        assert!(!buffer.undo());
        assert!(buffer.redo());
        assert_eq!(buffer.text(), "Aё🙂B\r\nконец");
    }
    #[test]
    /// Refused paste leaves text/selection/history unchanged; reset severs old undo coordinates.
    fn limits_and_document_reset() {
        let mut buffer = TextBuffer::new(8);
        buffer.reset("нач".into());
        let before = (buffer.text.clone(), buffer.selection, buffer.revision);
        assert!(!buffer.replace(None, "слишком длинно"));
        assert!(!buffer.replace(None, "\0"));
        assert_eq!(
            (buffer.text.clone(), buffer.selection, buffer.revision),
            before
        );
        assert!(!buffer.undo());
        assert!(buffer.replace(None, "a"));
        buffer.reset("new".into());
        assert!(!buffer.undo());
        assert_eq!(buffer.text(), "new");
    }
}
