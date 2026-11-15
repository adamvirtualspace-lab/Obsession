//! Where the Source editor is scrolled to.
//!
//! iced keeps the editor's scroll position private, and Split mode needs it
//! to keep the read-only view beside the editor in step. So this follows the
//! same rules the editor's text engine (cosmic-text) uses: the scroll is a
//! line plus a pixel offset into it, moved by wheel actions, pulled to the
//! caret when it moves, and clamped so the last line doesn't leave the
//! bottom. Line heights come from laying each line out with the editor's
//! font, size and width, so wrapped lines count exactly as the editor
//! counts them. Heights are measured only for lines that are needed and
//! kept until the line changes.

use iced::Size;
use iced::advanced::text::Paragraph as _;
use iced::widget::text::{LineHeight, Shaping, Wrapping};
use iced::widget::text_editor::Content;

use crate::live::LINE_PX;
use crate::theme;

type P = <iced::Renderer as iced::advanced::text::Renderer>::Paragraph;

#[derive(Debug, Default)]
pub struct SourceScroll {
    /// First line at the top of the view, and how far into it.
    pub line: usize,
    pub vertical: f32,
    /// The editor's text area (the widget minus its padding).
    size: Size,
    /// Each line's laid-out height; 0 = not measured yet.
    heights: Vec<f32>,
}

impl SourceScroll {
    /// A new editor: scrolled to the top.
    pub fn reset(&mut self) {
        self.line = 0;
        self.vertical = 0.0;
        self.heights.clear();
    }

    /// The text area's size. Returns false while it isn't known yet.
    pub fn resize(&mut self, size: Size, content: &Content) -> bool {
        if size.width <= 0.0 || size.height <= 0.0 {
            return false;
        }
        if size != self.size {
            if size.width != self.size.width {
                self.heights.clear();
            }
            self.size = size;
            self.settle(content);
        }
        true
    }

    pub fn height(&self) -> f32 {
        self.size.height
    }

    /// Forgets the heights of lines an edit touched: `from..=to`, or every
    /// line from `from` on when lines were added or removed.
    pub fn edited(&mut self, from: usize, to: usize, shifted: bool) {
        if shifted {
            self.heights.truncate(from);
        } else {
            for height in self.heights.iter_mut().take(to + 1).skip(from) {
                *height = 0.0;
            }
        }
    }

    pub fn line_height(&mut self, content: &Content, line: usize) -> f32 {
        let count = content.line_count();
        if self.heights.len() != count {
            self.heights.resize(count, 0.0);
        }
        let Some(&known) = self.heights.get(line) else {
            return LINE_PX;
        };
        if known > 0.0 {
            return known;
        }
        let text = content.line(line).map(|l| l.text.into_owned()).unwrap_or_default();
        let height = measure(&text, self.size.width);
        self.heights[line] = height;
        height
    }

    /// A wheel action: `pixels` down (negative is up).
    pub fn scroll(&mut self, pixels: f32, content: &Content) {
        self.vertical += pixels;
        self.settle(content);
    }

    /// Keeps the scroll inside the text, as the editor does after a scroll
    /// or a resize.
    fn settle(&mut self, content: &Content) {
        let count = content.line_count();
        if self.line >= count {
            self.line = count.saturating_sub(1);
            self.vertical = 0.0;
        }
        loop {
            while self.vertical < 0.0 {
                if self.line > 0 {
                    self.line -= 1;
                    self.vertical += self.line_height(content, self.line);
                } else {
                    self.vertical = 0.0;
                    break;
                }
            }
            let end = self.vertical + self.size.height;
            let mut total = 0.0;
            for i in self.line..count {
                if total > end {
                    break;
                }
                let height = self.line_height(content, i);
                total += height;
                if i == self.line && height <= self.vertical {
                    self.line += 1;
                    self.vertical -= height;
                }
            }
            if total < end && self.line > 0 {
                self.vertical -= end - total;
            } else {
                break;
            }
        }
    }

    /// The caret moved: scroll just enough to show it.
    pub fn reveal(&mut self, content: &Content, line: usize, column: usize) {
        let line = line.min(content.line_count().saturating_sub(1));
        let row_y = self.row_top(content, line, column);
        let mut total = row_y + LINE_PX;
        if self.line > line || (self.line == line && self.vertical > row_y) {
            self.line = line;
            self.vertical = row_y;
        } else if line <= self.line {
            if total > self.size.height + self.vertical {
                self.vertical = total - self.size.height;
            }
        } else {
            let mut i = line;
            while i > self.line {
                i -= 1;
                total += self.line_height(content, i);
                if total > self.size.height + self.vertical {
                    self.line = i;
                    self.vertical = total - self.size.height;
                }
            }
        }
    }

    /// How far below its line's top the caret's row is.
    pub fn row_top(&mut self, content: &Content, line: usize, column: usize) -> f32 {
        if self.line_height(content, line) <= LINE_PX {
            return 0.0;
        }
        let text = content.line(line).map(|l| l.text.into_owned()).unwrap_or_default();
        let column = floor_char_boundary(&text, column.min(text.len()));
        (measure(&text[..column], self.size.width) - LINE_PX).max(0.0)
    }

    /// The y of a point `frac` of the way down `line`, from the top of the
    /// text area (negative when above it).
    pub fn y_of(&mut self, content: &Content, line: usize, frac: f32) -> f32 {
        let mut y = -self.vertical;
        if line >= self.line {
            for i in self.line..line {
                y += self.line_height(content, i);
            }
        } else {
            for i in line..self.line {
                y -= self.line_height(content, i);
            }
        }
        y + frac * self.line_height(content, line)
    }

    /// The line at `y` from the top of the text area, and how far into it.
    pub fn at(&mut self, content: &Content, y: f32) -> (usize, f32) {
        let count = content.line_count();
        let mut top = -self.vertical;
        for i in self.line..count {
            let height = self.line_height(content, i);
            if y < top + height {
                return (i, ((y - top) / height).clamp(0.0, 1.0));
            }
            top += height;
        }
        (count.saturating_sub(1), 1.0)
    }
}

/// The height of one editor line laid out at `width`.
fn measure(text: &str, width: f32) -> f32 {
    if text.is_empty() {
        return LINE_PX;
    }
    P::with_text(iced::advanced::Text {
        content: text,
        bounds: Size::new(width, f32::INFINITY),
        size: iced::Pixels(theme::TEXT_SIZE),
        line_height: LineHeight::Absolute(iced::Pixels(LINE_PX)),
        font: theme::MONO_FONT,
        align_x: iced::widget::text::Alignment::Default,
        align_y: iced::alignment::Vertical::Top,
        shaping: Shaping::Advanced,
        wrapping: Wrapping::Word,
    })
    .min_bounds()
    .height
    .max(LINE_PX)
}

fn floor_char_boundary(s: &str, mut i: usize) -> usize {
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}
