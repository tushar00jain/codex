use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::widgets::Block;
use ratatui::widgets::Borders;

pub(crate) mod highlight;
pub(crate) mod line_utils;
pub(crate) mod renderable;

pub trait BlockExt {
    fn hairline(self, style: Style) -> Self;
}

impl BlockExt for Block<'_> {
    /// Brackets the block with hairline rules above and below it, leaving the
    /// sides open. `Block::inner` yields the content rows between them.
    fn hairline(self, style: Style) -> Self {
        self.borders(Borders::TOP | Borders::BOTTOM)
            .border_style(style)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Insets {
    left: u16,
    top: u16,
    right: u16,
    bottom: u16,
}

impl Insets {
    pub fn tlbr(top: u16, left: u16, bottom: u16, right: u16) -> Self {
        Self {
            top,
            left,
            bottom,
            right,
        }
    }

    pub fn vh(v: u16, h: u16) -> Self {
        Self {
            top: v,
            left: h,
            bottom: v,
            right: h,
        }
    }
}

pub trait RectExt {
    fn inset(&self, insets: Insets) -> Rect;
}

impl RectExt for Rect {
    fn inset(&self, insets: Insets) -> Rect {
        let horizontal = insets.left.saturating_add(insets.right);
        let vertical = insets.top.saturating_add(insets.bottom);
        Rect {
            x: self.x.saturating_add(insets.left),
            y: self.y.saturating_add(insets.top),
            width: self.width.saturating_sub(horizontal),
            height: self.height.saturating_sub(vertical),
        }
    }
}
