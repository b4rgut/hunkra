//! Owned native multiline text surface using GPUI's platform input contract.
use crate::{text::TextBuffer, theme};
use gpui::{
    App, Bounds, ClipboardItem, ContentMask, Context, CursorStyle, Element, ElementId,
    ElementInputHandler, Entity, EntityInputHandler, EventEmitter, FocusHandle, Focusable,
    GlobalElementId, KeyDownEvent, LayoutId, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, Pixels, Point, ShapedLine, SharedString, Style, TextRun, UTF16Selection, Window,
    div, fill, point, prelude::*, px, relative, rgb, rgba, size,
};
use std::ops::Range;
use unicode_segmentation::UnicodeSegmentation;

const LINE_HEIGHT: f32 = 22.;
pub struct TextChanged;
pub struct TextRejected;
pub struct TextSubmitted;
#[derive(Clone)]
struct Row {
    start: usize,
    end: usize,
    line: ShapedLine,
    origin: Point<Pixels>,
}
pub struct NativeEditor {
    pub buffer: TextBuffer,
    pub placeholder: SharedString,
    pub enabled: bool,
    pub single_line: bool,
    focus: FocusHandle,
    rows: Vec<Row>,
    bounds: Option<Bounds<Pixels>>,
    layout_revision: u64,
    dragging: bool,
    preferred_column: Option<usize>,
    scroll_row: usize,
    horizontal: Pixels,
}
impl EventEmitter<TextChanged> for NativeEditor {}
impl EventEmitter<TextRejected> for NativeEditor {}
impl EventEmitter<TextSubmitted> for NativeEditor {}
impl Focusable for NativeEditor {
    /// Exposes the platform focus handle without moving selection or text.
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}
impl NativeEditor {
    /// Creates an owned input whose placeholder may change independently of the user's text.
    pub fn new(limit: usize, placeholder: String, cx: &mut Context<Self>) -> Self {
        Self {
            buffer: TextBuffer::new(limit),
            placeholder: placeholder.into(),
            enabled: true,
            single_line: false,
            focus: cx.focus_handle(),
            rows: Vec::new(),
            bounds: None,
            layout_revision: u64::MAX,
            dragging: false,
            preferred_column: None,
            scroll_row: 0,
            horizontal: px(0.),
        }
    }
    /// Creates a branch/filter input that rejects newline paste/composition and emits Enter separately.
    pub fn single_line(limit: usize, placeholder: String, cx: &mut Context<Self>) -> Self {
        let mut editor = Self::new(limit, placeholder, cx);
        editor.single_line = true;
        editor
    }
    /// Reveals an existing byte range without changing text or its undo/redo history.
    pub fn reveal_range(&mut self, range: Range<usize>, cx: &mut Context<Self>) {
        self.buffer.select(range.start, false);
        self.buffer.select(range.end, true);
        self.scroll_row = self.buffer.text()[..self.buffer.boundary(range.start)]
            .bytes()
            .filter(|byte| *byte == b'\n')
            .count();
        self.horizontal = px(0.);
        cx.notify();
    }
    /// Keeps single-line values exact by refusing newline input rather than silently rewriting it.
    fn accepts(&self, text: &str) -> bool {
        !self.single_line || !text.contains(['\n', '\r'])
    }
    /// Notifies both the editor and its owning shell after an accepted text mutation.
    fn changed(&mut self, cx: &mut Context<Self>) {
        cx.emit(TextChanged);
        cx.notify();
    }
    /// Finds source line starts and visible ends, retaining CRLF bytes in the model.
    fn lines(&self) -> Vec<Range<usize>> {
        let mut result = Vec::new();
        let mut start = 0;
        for piece in self.buffer.text().split_inclusive('\n') {
            let display = piece
                .strip_suffix('\n')
                .map(|line| line.strip_suffix('\r').unwrap_or(line))
                .unwrap_or(piece);
            result.push(start..start + display.len());
            start += piece.len();
        }
        if result.is_empty() || self.buffer.text().ends_with('\n') {
            result.push(start..start);
        }
        result
    }
    /// Maps a current painted point to a valid source byte; stale frames cannot change selection.
    fn index_at(&self, position: Point<Pixels>) -> Option<usize> {
        if self.layout_revision != self.buffer.revision {
            return None;
        }
        let bounds = self.bounds?;
        if position.y < bounds.top() {
            return self.rows.first().map(|row| row.start);
        }
        let row = self
            .rows
            .iter()
            .find(|row| position.y < row.origin.y + px(LINE_HEIGHT))
            .or_else(|| self.rows.last())?;
        Some(
            row.start
                + row
                    .line
                    .closest_index_for_x(position.x - row.origin.x)
                    .min(row.end - row.start),
        )
    }
    /// Focuses and starts a selection only against the frame matching the current text revision.
    fn mouse_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus);
        if let Some(index) = self.index_at(event.position) {
            self.buffer.unmark();
            self.buffer.select(index, event.modifiers.shift);
            self.dragging = true;
            self.preferred_column = None;
            cx.notify();
        }
    }
    /// Finishes dragging even when the pointer is released outside the field.
    fn mouse_up(&mut self, _: &MouseUpEvent, _: &mut Window, _: &mut Context<Self>) {
        self.dragging = false;
    }
    /// Extends a drag selection using actual shaped text positions, not approximate character widths.
    fn mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.dragging
            && let Some(index) = self.index_at(event.position)
        {
            self.buffer.select(index, true);
            cx.notify();
        }
    }
    /// Moves vertically by a retained grapheme column, clamping only in the target line.
    fn vertical(&mut self, down: bool, extend: bool) {
        let lines = self.lines();
        let current = lines
            .iter()
            .rposition(|line| line.start <= self.buffer.selection.cursor)
            .unwrap_or(0);
        let column = *self.preferred_column.get_or_insert_with(|| {
            self.buffer.text()
                [lines[current].start..self.buffer.selection.cursor.min(lines[current].end)]
                .graphemes(true)
                .count()
        });
        let target = if down {
            (current + 1).min(lines.len() - 1)
        } else {
            current.saturating_sub(1)
        };
        let line = &lines[target];
        let index = self.buffer.text()[line.clone()]
            .grapheme_indices(true)
            .nth(column)
            .map_or(line.end, |(offset, _)| line.start + offset);
        self.buffer.select(index, extend);
    }
    /// Handles editing keys locally while ordinary characters remain platform/IME input.
    pub fn key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let key = event.keystroke.key.as_str();
        if self.single_line && matches!(key, "up" | "down") {
            return;
        }
        let modifiers = event.keystroke.modifiers;
        let command = modifiers.control || modifiers.platform;
        let extend = modifiers.shift;
        let mut changed = false;
        if !matches!(key, "up" | "down") {
            self.preferred_column = None;
        }
        match (command, key) {
            (true, "a") => {
                self.buffer.select(0, false);
                self.buffer.select(self.buffer.text().len(), true);
            }
            (true, "c") => {
                let range = self.buffer.selection.range();
                if !range.is_empty() {
                    cx.write_to_clipboard(ClipboardItem::new_string(
                        self.buffer.text()[range].into(),
                    ));
                }
            }
            (true, "x") if self.enabled => {
                let range = self.buffer.selection.range();
                if !range.is_empty() {
                    cx.write_to_clipboard(ClipboardItem::new_string(
                        self.buffer.text()[range].into(),
                    ));
                    changed = self.buffer.replace(None, "");
                }
            }
            (true, "v") if self.enabled => {
                if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
                    changed = self.accepts(&text) && self.buffer.replace(None, &text);
                    if !changed {
                        cx.emit(TextRejected);
                    }
                }
            }
            (true, "z") if self.enabled => {
                changed = if extend {
                    self.buffer.redo()
                } else {
                    self.buffer.undo()
                }
            }
            (true, "y") if self.enabled => changed = self.buffer.redo(),
            (false, "backspace" | "delete") if self.enabled => {
                self.buffer.unmark();
                if self.buffer.selection.range().is_empty() {
                    let cursor = self.buffer.selection.cursor;
                    self.buffer.select(
                        if key == "backspace" {
                            self.buffer.previous(cursor)
                        } else {
                            self.buffer.next(cursor)
                        },
                        true,
                    );
                }
                changed = self.buffer.replace(None, "");
            }
            (false, "enter") if self.enabled => {
                if self.single_line {
                    cx.emit(TextSubmitted);
                } else {
                    let newline = if self.buffer.text().contains("\r\n") {
                        "\r\n"
                    } else {
                        "\n"
                    };
                    changed = self.buffer.replace(None, newline);
                }
            }
            (false, "left" | "right") => {
                self.buffer.unmark();
                let range = self.buffer.selection.range();
                let index = if !extend && !range.is_empty() {
                    if key == "left" {
                        range.start
                    } else {
                        range.end
                    }
                } else if key == "left" {
                    self.buffer.previous(self.buffer.selection.cursor)
                } else {
                    self.buffer.next(self.buffer.selection.cursor)
                };
                self.buffer.select(index, extend);
            }
            (false, "up" | "down") => {
                self.buffer.unmark();
                self.vertical(key == "down", extend);
            }
            (_, "home" | "end") => {
                self.buffer.unmark();
                let lines = self.lines();
                let line = lines
                    .iter()
                    .rfind(|line| line.start <= self.buffer.selection.cursor)
                    .unwrap();
                let index = if command {
                    if key == "home" {
                        0
                    } else {
                        self.buffer.text().len()
                    }
                } else if key == "home" {
                    line.start
                } else {
                    line.end
                };
                self.buffer.select(index, extend);
            }
            _ => return,
        }
        window.prevent_default();
        cx.stop_propagation();
        if changed {
            self.changed(cx);
        } else {
            cx.notify();
        }
    }
}
impl EntityInputHandler for NativeEditor {
    /// Returns scalar-safe text and the adjusted native UTF-16 range requested by the input method.
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        adjusted: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        let bytes = self.buffer.from_utf16(range);
        *adjusted =
            Some(self.buffer.utf16_offset(bytes.start)..self.buffer.utf16_offset(bytes.end));
        Some(self.buffer.text()[bytes].to_owned())
    }
    /// Preserves the selected range and direction; disabled input remains queryable when requested.
    fn selected_text_range(
        &mut self,
        ignore_disabled: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        if !self.enabled && !ignore_disabled {
            return None;
        }
        let (range, reversed) = self.buffer.selected_utf16();
        Some(UTF16Selection { range, reversed })
    }
    /// Reports the active composition in platform coordinates without altering its content.
    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        self.buffer
            .marked
            .as_ref()
            .map(|range| self.buffer.utf16_offset(range.start)..self.buffer.utf16_offset(range.end))
    }
    /// Commits the current composition as a single undo revision.
    fn unmark_text(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.buffer.unmark();
        cx.notify();
    }
    /// Accepts native text only while enabled, preserving exact UTF-8/newline bytes and history.
    fn replace_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.enabled {
            if self.accepts(text) && self.buffer.replace(range, text) {
                self.changed(cx);
            } else {
                cx.emit(TextRejected);
            }
        }
    }
    /// Updates marked native text and its insertion-relative selection without splitting Unicode.
    fn replace_and_mark_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        selected: Option<Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.enabled {
            if self.accepts(text) && self.buffer.compose(range, text, selected) {
                self.changed(cx);
            } else {
                cx.emit(TextRejected);
            }
        }
    }
    /// Gives the input method the current caret/range rectangle from actual shaped text.
    fn bounds_for_range(
        &mut self,
        range: Range<usize>,
        _: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        if self.layout_revision != self.buffer.revision {
            return None;
        }
        let range = self.buffer.from_utf16(range);
        let row = self
            .rows
            .iter()
            .find(|row| range.start >= row.start && range.start <= row.end)?;
        let start = row.line.x_for_index(range.start - row.start);
        let end = row.line.x_for_index(range.end.min(row.end) - row.start);
        Some(Bounds::new(
            point(row.origin.x + start, row.origin.y),
            size((end - start).max(px(1.)), px(LINE_HEIGHT)),
        ))
    }
    /// Maps an input method point back into UTF-16 coordinates using the same painted layout.
    fn character_index_for_point(
        &mut self,
        position: Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        self.index_at(position)
            .map(|byte| self.buffer.utf16_offset(byte))
    }
}
impl Render for NativeEditor {
    /// Renders one focusable text viewport; its owned placeholder never replaces the user's text.
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .track_focus(&self.focus)
            .key_context("HunkraEditor")
            .cursor(CursorStyle::IBeam)
            .font_family("monospace")
            .text_size(px(14.))
            .line_height(px(LINE_HEIGHT))
            .text_color(rgb(theme::TEXT))
            .bg(rgb(theme::BACKGROUND))
            .rounded_sm()
            .border_1()
            .border_color(rgb(theme::BORDER))
            .p_2()
            .on_key_down(cx.listener(Self::key_down))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::mouse_down))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::mouse_up))
            .on_mouse_move(cx.listener(Self::mouse_move))
            .child(TextSurface {
                editor: cx.entity(),
            })
    }
}

struct TextSurface {
    editor: Entity<NativeEditor>,
}
struct PaintState {
    rows: Vec<Row>,
    selections: Vec<Bounds<Pixels>>,
    caret: Option<Bounds<Pixels>>,
    marked: Vec<Bounds<Pixels>>,
    revision: u64,
}
impl IntoElement for TextSurface {
    type Element = Self;
    /// Keeps the native text surface's custom layout and input handling intact.
    fn into_element(self) -> Self {
        self
    }
}
impl Element for TextSurface {
    type RequestLayoutState = ();
    type PrepaintState = PaintState;
    /// Leaves identity on the owning focusable field.
    fn id(&self) -> Option<ElementId> {
        None
    }
    /// This owned surface does not attach a runtime inspector source location.
    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }
    /// Occupies the remaining editor viewport without sizing to the entire document.
    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = relative(1.).into();
        (window.request_layout(style, [], cx), ())
    }
    /// Shapes only visible rows and derives selection, composition and caret from their source bytes.
    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) -> PaintState {
        self.editor.update(cx, |editor, _| {
            let lines = editor.lines();
            let cursor = editor.buffer.selection.cursor;
            let cursor_row = lines
                .iter()
                .rposition(|line| line.start <= cursor)
                .unwrap_or(0);
            let visible = ((bounds.size.height / px(LINE_HEIGHT)).floor() as usize).max(1);
            if cursor_row < editor.scroll_row {
                editor.scroll_row = cursor_row;
            }
            if cursor_row >= editor.scroll_row + visible {
                editor.scroll_row = cursor_row + 1 - visible;
            }
            editor.scroll_row = editor.scroll_row.min(lines.len().saturating_sub(visible));
            let style = window.text_style();
            let font_size = style.font_size.to_pixels(window.rem_size());
            let mut rows: Vec<Row> = lines
                .iter()
                .skip(editor.scroll_row)
                .take(visible)
                .enumerate()
                .map(|(row, range)| {
                    let placeholder = editor.buffer.text().is_empty();
                    let text: SharedString = if placeholder {
                        editor.placeholder.clone()
                    } else {
                        editor.buffer.text()[range.clone()].to_owned().into()
                    };
                    let run = TextRun {
                        len: text.len(),
                        font: style.font(),
                        color: if placeholder {
                            rgb(theme::MUTED).into()
                        } else {
                            style.color
                        },
                        background_color: None,
                        underline: None,
                        strikethrough: None,
                    };
                    let line = window
                        .text_system()
                        .shape_line(text, font_size, &[run], None);
                    Row {
                        start: range.start,
                        end: range.end,
                        line,
                        origin: point(bounds.left(), bounds.top() + px(row as f32 * LINE_HEIGHT)),
                    }
                })
                .collect();
            if let Some(row) = rows
                .iter()
                .find(|row| cursor >= row.start && cursor <= row.end)
            {
                let x = row.line.x_for_index(cursor - row.start);
                let width = bounds.size.width.max(px(1.));
                if x < editor.horizontal {
                    editor.horizontal = x;
                }
                if x >= editor.horizontal + width - px(2.) {
                    editor.horizontal = (x - width + px(4.)).max(px(0.));
                }
            }
            let selection = editor.buffer.selection.range();
            let mut state = PaintState {
                rows: Vec::new(),
                selections: Vec::new(),
                caret: None,
                marked: Vec::new(),
                revision: editor.buffer.revision,
            };
            for row in &mut rows {
                row.origin.x -= editor.horizontal;
                if selection.start <= row.end && selection.end > row.start && !selection.is_empty()
                {
                    let start = selection.start.max(row.start).min(row.end) - row.start;
                    let end = selection.end.min(row.end) - row.start;
                    let x = row.line.x_for_index(start);
                    let width = row.line.x_for_index(end) - x
                        + if selection.end > row.end {
                            px(6.)
                        } else {
                            px(0.)
                        };
                    state.selections.push(Bounds::new(
                        point(row.origin.x + x, row.origin.y),
                        size(width, px(LINE_HEIGHT)),
                    ));
                }
                if cursor >= row.start && cursor <= row.end {
                    state.caret = Some(Bounds::new(
                        point(
                            row.origin.x + row.line.x_for_index(cursor - row.start),
                            row.origin.y,
                        ),
                        size(px(2.), px(LINE_HEIGHT)),
                    ));
                }
                if let Some(marked) = &editor.buffer.marked
                    && marked.start < row.end
                    && marked.end > row.start
                {
                    let x = row
                        .line
                        .x_for_index(marked.start.max(row.start) - row.start);
                    let end = row.line.x_for_index(marked.end.min(row.end) - row.start);
                    state.marked.push(Bounds::new(
                        point(row.origin.x + x, row.origin.y + px(LINE_HEIGHT - 2.)),
                        size(end - x, px(1.)),
                    ));
                }
            }
            state.rows = rows;
            state
        })
    }
    /// Paints inside the viewport and publishes that exact frame for native input and pointer queries.
    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        state: &mut PaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let focus = self.editor.read(cx).focus.clone();
        window.handle_input(
            &focus,
            ElementInputHandler::new(bounds, self.editor.clone()),
            cx,
        );
        window.with_content_mask(Some(ContentMask { bounds }), |window| {
            for selected in &state.selections {
                window.paint_quad(fill(*selected, rgba(0x547a9a88)));
            }
            for row in &state.rows {
                let _ = row.line.paint(row.origin, px(LINE_HEIGHT), window, cx);
            }
            for marked in &state.marked {
                window.paint_quad(fill(*marked, rgb(theme::ACCENT)));
            }
            if focus.is_focused(window)
                && let Some(caret) = state.caret
            {
                window.paint_quad(fill(caret, rgb(theme::TEXT)));
            }
        });
        self.editor.update(cx, |editor, _| {
            editor.rows = state.rows.clone();
            editor.bounds = Some(bounds);
            editor.layout_revision = state.revision;
        });
    }
}

#[cfg(all(test, feature = "gui-tests"))]
mod tests {
    use super::*;
    use gpui::TestAppContext;
    struct NavigationHost {
        input: Entity<NativeEditor>,
        arrows: usize,
    }
    impl Render for NavigationHost {
        /// Embeds the real field in a parent that owns list navigation, exercising GPUI bubbling rather than direct handler calls.
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            div()
                .size_full()
                .child(self.input.clone())
                .on_key_down(cx.listener(|host, event: &KeyDownEvent, _, cx| {
                    if matches!(event.keystroke.key.as_str(), "up" | "down") {
                        host.arrows += 1;
                        cx.stop_propagation();
                    }
                }))
        }
    }
    #[gpui::test]
    /// Single-line arrows bubble to the visible list without changing native field text, selection or revision.
    fn single_line_arrows_reach_parent(cx: &mut TestAppContext) {
        let (host, cx) = cx.add_window_view(|window, cx| {
            let input = cx.new(|cx| NativeEditor::single_line(4096, "filter".into(), cx));
            window.focus(&input.read(cx).focus_handle(cx));
            NavigationHost { input, arrows: 0 }
        });
        cx.simulate_input("src Ядро");
        let input = host.read_with(cx, |host, _| host.input.clone());
        let before = input.read_with(cx, |editor, _| {
            (
                editor.buffer.text().to_owned(),
                editor.buffer.selection,
                editor.buffer.revision,
            )
        });
        cx.simulate_keystrokes("up down");
        assert_eq!(host.read_with(cx, |host, _| host.arrows), 2);
        assert_eq!(
            before,
            input.read_with(cx, |editor, _| (
                editor.buffer.text().to_owned(),
                editor.buffer.selection,
                editor.buffer.revision
            ))
        );
    }
    #[gpui::test]
    /// Single-line fields retain exact Unicode and selection when clipboard/native composition includes newlines.
    fn single_line_refuses_multiline_input(cx: &mut TestAppContext) {
        let (editor, cx) = cx.add_window_view(|window, cx| {
            let editor = NativeEditor::single_line(4096, "branch".into(), cx);
            window.focus(&editor.focus);
            editor
        });
        cx.simulate_input("задача/ветка🦀");
        let before = editor.read_with(cx, |editor, _| {
            (
                editor.buffer.text().to_owned(),
                editor.buffer.selection,
                editor.buffer.revision,
            )
        });
        cx.write_to_clipboard(ClipboardItem::new_string("line one\nline two".into()));
        cx.simulate_keystrokes("ctrl-v enter");
        cx.simulate_input("\r\n");
        let after = editor.read_with(cx, |editor, _| {
            (
                editor.buffer.text().to_owned(),
                editor.buffer.selection,
                editor.buffer.revision,
            )
        });
        assert_eq!(before, after);
    }
    #[gpui::test]
    /// Exercises registered native keyboard, Unicode insertion and clipboard handlers on GPUI's isolated platform.
    fn keyboard_clipboard_and_disabled_input(cx: &mut TestAppContext) {
        let (editor, cx) = cx.add_window_view(|window, cx| {
            let editor = NativeEditor::new(64 * 1024, "message".into(), cx);
            window.focus(&editor.focus);
            editor
        });
        cx.simulate_input("Привет🦀");
        assert_eq!(
            editor.read_with(cx, |editor, _| editor.buffer.text().to_owned()),
            "Привет🦀"
        );
        cx.simulate_keystrokes("ctrl-a ctrl-c end enter ctrl-v");
        assert_eq!(
            editor.read_with(cx, |editor, _| editor.buffer.text().to_owned()),
            "Привет🦀\nПривет🦀"
        );
        assert_eq!(
            cx.read_from_clipboard()
                .and_then(|item| item.text())
                .as_deref(),
            Some("Привет🦀")
        );
        cx.simulate_keystrokes("ctrl-a ctrl-x");
        assert!(editor.read_with(cx, |editor, _| editor.buffer.text().is_empty()));
        cx.simulate_keystrokes("ctrl-z");
        assert_eq!(
            editor.read_with(cx, |editor, _| editor.buffer.text().to_owned()),
            "Привет🦀\nПривет🦀"
        );
        cx.simulate_keystrokes("ctrl-shift-z");
        assert!(editor.read_with(cx, |editor, _| editor.buffer.text().is_empty()));
        editor.update(cx, |editor, cx| {
            editor.enabled = false;
            cx.notify();
        });
        cx.simulate_keystrokes("ctrl-v");
        cx.simulate_input("нельзя");
        assert!(editor.read_with(cx, |editor, _| editor.buffer.text().is_empty()));
        editor.update(cx, |editor, cx| {
            editor.enabled = true;
            cx.notify();
        });
        cx.simulate_keystrokes("ctrl-v");
        assert_eq!(
            editor.read_with(cx, |editor, _| editor.buffer.text().to_owned()),
            "Привет🦀\nПривет🦀"
        );
    }
}
