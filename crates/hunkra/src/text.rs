//! Owned Unicode text, selection, IME composition and bounded undo, independent of GPUI.
use std::ops::Range;
use std::sync::OnceLock;
use unicode_segmentation::GraphemeCursor;

const MAX_UNDO_BYTES: usize = 8 * 1024 * 1024;
const MAX_UNDO_EDITS: usize = 256;
const UTF16_STRIDE: usize = 512;

#[derive(Clone, Copy, PartialEq, Eq)]
enum WordClass {
    Space,
    Word,
    Punctuation,
}
impl WordClass {
    /// Classifies whole graphemes: whitespace, letters/numbers/identifier underscores, or punctuation/emoji.
    fn for_grapheme(text: &str) -> Self {
        if text.chars().all(char::is_whitespace) {
            Self::Space
        } else if text.chars().any(|ch| ch.is_alphanumeric() || ch == '_') {
            Self::Word
        } else {
            Self::Punctuation
        }
    }
}

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
    coordinates: OnceLock<Vec<(usize, usize)>>,
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
            coordinates: OnceLock::new(),
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
        self.text_changed();
        true
    }
    /// Retires native coordinate checkpoints whenever actual text changes, including document loads and undo/redo.
    fn text_changed(&mut self) {
        self.coordinates.take();
        self.revision += 1;
    }
    /// Lazily counts UTF-16 in scalar-safe byte chunks, retaining only at most len/512+2 coordinate pairs for the current text.
    fn coordinates(&self) -> &[(usize, usize)] {
        self.coordinates.get_or_init(|| {
            let mut points = Vec::with_capacity(self.text.len() / UTF16_STRIDE + 2);
            points.push((0, 0));
            let (mut byte, mut units) = (0, 0);
            while byte < self.text.len() {
                let mut end = byte.saturating_add(UTF16_STRIDE).min(self.text.len());
                while !self.text.is_char_boundary(end) {
                    end += 1;
                }
                units += self.text[byte..end].encode_utf16().count();
                byte = end;
                points.push((byte, units));
            }
            points
        })
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
        let byte = self.boundary(byte);
        if byte == 0 {
            return 0;
        }
        let points = self.coordinates();
        let (start, units) = points[points.partition_point(|(start, _)| *start <= byte) - 1];
        units + self.text[start..byte].encode_utf16().count()
    }
    /// Converts UTF-16 offsets, rounding an interior surrogate pair toward the requested edge.
    fn byte_offset(&self, utf16: usize, end: bool) -> usize {
        if utf16 == 0 {
            return 0;
        }
        let points = self.coordinates();
        let (start, mut units) = points[points.partition_point(|(_, units)| *units <= utf16) - 1];
        for (relative, ch) in self.text[start..].char_indices() {
            let byte = start + relative;
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
    /// Moves forward over initial whitespace and then one word or punctuation run, retaining complete graphemes.
    pub fn next_word(&self, byte: usize) -> usize {
        self.word_boundary(byte, true)
    }
    /// Moves backward over initial whitespace and then one word or punctuation run without splitting CRLF or joined emoji.
    pub fn previous_word(&self, byte: usize) -> usize {
        self.word_boundary(byte, false)
    }
    /// Walks only neighboring graphemes needed by the requested token; no document-wide word index or text copy is retained.
    fn word_boundary(&self, byte: usize, forward: bool) -> usize {
        let mut cursor = self.boundary(byte);
        let mut kind = None;
        while if forward {
            cursor < self.text.len()
        } else {
            cursor > 0
        } {
            let next = if forward {
                self.next(cursor)
            } else {
                self.previous(cursor)
            };
            let range = cursor.min(next)..cursor.max(next);
            let current = WordClass::for_grapheme(&self.text[range]);
            if let Some(kind) = kind {
                if current != kind {
                    break;
                }
            } else if current != WordClass::Space {
                kind = Some(current);
            }
            cursor = next;
        }
        cursor
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
        self.text_changed();
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
    /// Deletes the existing selection or bytes toward one boundary, remembering the original caret rather than a temporary deletion selection.
    pub fn delete_to(&mut self, byte: usize) -> bool {
        self.unmark();
        let mut range = self.selection.range();
        if range.is_empty() {
            let target = self.boundary(byte);
            range = self.selection.cursor.min(target)..self.selection.cursor.max(target);
        }
        !range.is_empty() && self.replace_bytes(range, "", true)
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
        self.text_changed();
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
        self.text_changed();
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use unicode_segmentation::UnicodeSegmentation;
    #[test]
    /// Sparse coordinate lookup preserves every scalar/surrogate edge, reversed/native ranges and EOF around multibyte checkpoint boundaries.
    fn sparse_utf16_coordinates_match_full_prefixes() {
        /// Computes the previous contract directly from Unicode scalars, independently of sparse checkpoints.
        fn native_byte(text: &str, target: usize, end: bool) -> usize {
            let mut units = 0;
            for (byte, ch) in text.char_indices() {
                if units >= target {
                    return byte;
                }
                if units + ch.len_utf16() > target {
                    return byte + if end { ch.len_utf8() } else { 0 };
                }
                units += ch.len_utf16();
            }
            text.len()
        }
        for text in [
            String::new(),
            "A🙂B\r\nе\u{301}👩🏽\u{200d}💻".repeat(35),
            format!("{}🙂{}Ж🦀\r\nEOF", "a".repeat(511), "б".repeat(257)),
        ] {
            let mut buffer = TextBuffer::new(65536);
            assert!(buffer.reset(text.clone()));
            let before = (buffer.selection, buffer.revision);
            for byte in (0..=text.len() + 4).chain(std::iter::once(usize::MAX)) {
                assert_eq!(
                    buffer.utf16_offset(byte),
                    text[..buffer.boundary(byte)].encode_utf16().count()
                );
            }
            let units = text.encode_utf16().count();
            for start in (0..=units + 4).chain(std::iter::once(usize::MAX)) {
                for end in [
                    start,
                    start.saturating_add(1),
                    start.saturating_sub(1),
                    units,
                    usize::MAX,
                ] {
                    let first = native_byte(&text, start, false);
                    let last = if start >= end {
                        first
                    } else {
                        native_byte(&text, end.max(start), true)
                    };
                    assert_eq!(buffer.from_utf16(start..end), first..last);
                }
            }
            assert_eq!((buffer.selection, buffer.revision), before);
            assert_eq!(buffer.text(), text);
        }
        let mut maximum = TextBuffer::new(2 * 1024 * 1024);
        assert!(maximum.reset("🦀".repeat(512 * 1024)));
        assert_eq!(maximum.utf16_offset(usize::MAX), 1024 * 1024);
        let points = maximum.coordinates.get().unwrap();
        assert!(points.capacity() <= maximum.text.len() / UTF16_STRIDE + 2);
        assert!(points.capacity() * std::mem::size_of::<(usize, usize)>() <= 65568);
    }

    #[test]
    /// Native coordinate caches retire after accepted edits/compositions/undo/redo/reset while selection, unmark and rejected writes reuse current text coordinates.
    fn utf16_coordinates_follow_text_and_history() {
        let original = "A🙂Б\r\nконец";
        let mut buffer = TextBuffer::new(1024);
        assert!(buffer.reset(original.into()));
        assert_eq!(
            buffer.utf16_offset(usize::MAX),
            original.encode_utf16().count()
        );
        assert!(buffer.coordinates.get().is_some());
        buffer.select(1, false);
        buffer.unmark();
        let revision = buffer.revision;
        assert!(!buffer.replace(None, "\0"));
        assert!(!buffer.reset("x".repeat(1025)));
        assert!(buffer.coordinates.get().is_some());
        assert_eq!(buffer.revision, revision);
        assert!(buffer.replace(Some(1..3), "е\u{301}🦀"));
        assert!(buffer.coordinates.get().is_none());
        assert_eq!(
            buffer.utf16_offset(usize::MAX),
            buffer.text().encode_utf16().count()
        );
        assert!(buffer.undo());
        assert!(buffer.coordinates.get().is_none());
        assert_eq!(buffer.text(), original);
        assert_eq!(
            buffer.utf16_offset(usize::MAX),
            original.encode_utf16().count()
        );
        assert!(buffer.redo());
        assert!(buffer.coordinates.get().is_none());
        let composed_from = buffer.text().to_owned();
        buffer.select(buffer.text().len(), false);
        assert!(buffer.compose(None, "ё🦀", Some(1..3)));
        assert!(buffer.coordinates.get().is_none());
        let marked = buffer.marked.clone().unwrap();
        let native = buffer.utf16_offset(marked.start)..buffer.utf16_offset(marked.end);
        assert_eq!(buffer.from_utf16(native), marked);
        assert!(buffer.compose(None, "界🙂", Some(0..3)));
        assert!(buffer.coordinates.get().is_none());
        assert_eq!(
            buffer.utf16_offset(usize::MAX),
            buffer.text().encode_utf16().count()
        );
        buffer.unmark();
        assert!(buffer.coordinates.get().is_some());
        assert!(buffer.undo());
        assert!(buffer.coordinates.get().is_none());
        assert_eq!(buffer.text(), composed_from);
        assert!(buffer.reset("Другой\r\n🦀".into()));
        assert!(buffer.coordinates.get().is_none());
        assert_eq!(
            buffer.utf16_offset(usize::MAX),
            "Другой\r\n🦀".encode_utf16().count()
        );
    }

    #[test]
    /// Word jumps group identifiers and punctuation, skip initial Unicode whitespace and land only on complete global graphemes.
    fn word_navigation_retains_graphemes_and_token_groups() {
        let segments = [
            "  foo_bar42",
            "::",
            "кириллица",
            " e\u{301}",
            " 👩🏽\u{200d}💻",
            "\r\n\t尾",
        ];
        let text = segments.concat();
        let mut buffer = TextBuffer::new(65536);
        assert!(buffer.reset(text.clone()));
        let mut cursor = 0;
        for segment in segments {
            let expected = cursor + segment.len();
            assert_eq!(buffer.next_word(cursor), expected);
            cursor = expected;
        }
        for segment in [
            "尾",
            "\r\n\t👩🏽\u{200d}💻",
            " e\u{301}",
            " кириллица",
            "::",
            "foo_bar42",
            "  ",
        ] {
            let expected = cursor - segment.len();
            assert_eq!(buffer.previous_word(cursor), expected);
            cursor = expected;
        }
        assert_eq!(cursor, 0);
        let mut boundaries = text
            .grapheme_indices(true)
            .map(|(byte, _)| byte)
            .collect::<Vec<_>>();
        boundaries.push(text.len());
        let before = (buffer.selection, buffer.revision);
        for byte in (0..=text.len() + 4).chain(std::iter::once(usize::MAX)) {
            assert!(boundaries.contains(&buffer.next_word(byte)));
            assert!(boundaries.contains(&buffer.previous_word(byte)));
        }
        assert_eq!((buffer.selection, buffer.revision), before);
        assert_eq!(buffer.text(), text);
        assert_eq!(buffer.next_word(usize::MAX), text.len());
        assert_eq!(buffer.previous_word(0), 0);
    }
    #[test]
    /// Deleting toward a word/grapheme boundary restores the original collapsed caret on undo, while an explicit selection retains its direction.
    fn deletion_undo_restores_the_original_caret() {
        let text = "начало e\u{301}👩🏽\u{200d}💻\r\nконец";
        let cursor = "начало e\u{301}".len();
        let mut buffer = TextBuffer::new(65536);
        for target in [
            "начало ".len(),
            "начало e\u{301}👩🏽\u{200d}💻".len(),
            text.len(),
        ] {
            assert!(buffer.reset(text.into()));
            buffer.select(cursor, false);
            let before = buffer.selection;
            assert!(buffer.delete_to(target));
            assert!(buffer.undo());
            assert_eq!(buffer.text(), text);
            assert_eq!(buffer.selection, before);
            assert!(buffer.redo());
            assert_eq!(buffer.selection.range().len(), 0);
        }
        buffer.reset(text.into());
        buffer.select(text.len(), false);
        buffer.select(cursor, true);
        let before = buffer.selection;
        assert!(buffer.delete_to(0));
        assert_eq!(buffer.text(), &text[..cursor]);
        assert!(buffer.undo());
        assert_eq!(buffer.selection, before);
        buffer.select(0, false);
        let revision = buffer.revision;
        assert!(!buffer.delete_to(0));
        assert_eq!(buffer.revision, revision);
    }

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
