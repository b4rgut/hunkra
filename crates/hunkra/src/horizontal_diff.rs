//! One exact shaped-text horizontal offset is shared by both diff panes; gutters and vertical virtualization remain fixed.
use gpui::{Pixels, ScrollWheelEvent, TextRun, Window, canvas, div, prelude::*, px};
use std::{cell::RefCell, rc::Rc};

#[derive(Default)]
struct State {
    offset: Pixels,
    width: Pixels,
    viewport: Pixels,
}
#[derive(Default, Clone)]
pub struct HorizontalDiff(Rc<RefCell<State>>);
impl HorizontalDiff {
    /// Resets presentation only when selecting another document; unchanged refresh and locale changes preserve the shared offset.
    pub fn reset(&self) {
        *self.0.borrow_mut() = State::default();
    }
    /// Exposes the shared offset for native acceptance and presentation controls, without reading Git.
    pub fn offset(&self) -> Pixels {
        self.0.borrow().offset
    }
    /// Uses exact widths of lazily shaped visible lines; newly exposed longer lines extend the reachable range.
    pub fn shift(&self, delta: Pixels) -> bool {
        let mut state = self.0.borrow_mut();
        let maximum = (state.width - state.viewport).max(px(0.));
        let next = (state.offset + delta).clamp(px(0.), maximum);
        let changed = next != state.offset;
        state.offset = next;
        changed
    }
    /// Handles trackpad horizontal deltas and Shift+wheel before the list; ordinary vertical wheel remains available to virtualization.
    pub fn wheel(&self, event: &ScrollWheelEvent) -> bool {
        let delta = event.delta.pixel_delta(px(20.));
        let horizontal = if event.modifiers.shift {
            if delta.x == px(0.) { delta.y } else { delta.x }
        } else {
            delta.x
        };
        if horizontal == px(0.) {
            return false;
        }
        self.shift(-horizontal);
        true
    }
    /// Shapes only the visible display string and clips its translated text within the actual pane viewport; source bytes remain unchanged.
    pub fn text(&self, text: String, window: &mut Window) -> gpui::Div {
        let font = gpui::font("DejaVu Sans Mono");
        let run = TextRun {
            len: text.len(),
            font,
            color: window.text_style().color,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let width = window
            .text_system()
            .shape_line(text.clone().into(), px(12.), &[run], None)
            .width;
        {
            let mut state = self.0.borrow_mut();
            state.width = state.width.max(width);
        }
        let state = self.0.clone();
        div()
            .flex_1()
            .min_w_0()
            .h_full()
            .relative()
            .flex()
            .items_center()
            .overflow_hidden()
            .font_family("DejaVu Sans Mono")
            .text_size(px(12.))
            .child(
                canvas(
                    move |bounds, window, _| {
                        if bounds.size.width > px(0.) {
                            let mut state = state.borrow_mut();
                            state.viewport = bounds.size.width;
                            let previous = state.offset;
                            state.offset =
                                state.offset.min((state.width - state.viewport).max(px(0.)));
                            if previous != state.offset {
                                window.refresh();
                            }
                        }
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .size_full(),
            )
            .child(
                div()
                    .relative()
                    .left(-self.offset())
                    .flex_none()
                    .whitespace_nowrap()
                    .child(text),
            )
    }
}
