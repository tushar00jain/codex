//! Session banner layout: a pixel logo in a left gutter with the header text
//! beside it, replacing the bordered card.
//!
//! Kept out of `session.rs` so merges from upstream stay cheap -- the upstream
//! file changes by a single call. The logo is confined to `robot.rs`, which
//! holds only a grid and its colors, so swapping logos means swapping that file.

mod robot;

use crate::line_truncation::truncate_line_with_ellipsis_if_overflow;
use ratatui::style::Color;
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::text::Span;
use unicode_width::UnicodeWidthStr;

/// Blank columns between the logo and the text block.
const GUTTER: usize = 2;

/// Grid key meaning "leave the terminal background showing through".
const TRANSPARENT: char = '.';

/// Renders a pixel grid as terminal rows, mapping each key to a color.
///
/// Two pixel rows collapse into one text row drawn with half blocks, so a cell
/// carries two pixels as its foreground and background. That keeps pixels square
/// despite a cell being roughly twice as tall as it is wide.
fn render_grid(rows: &[&str], tone: impl Fn(char) -> Option<Color>) -> Vec<Line<'static>> {
    let width = rows.iter().map(|row| row.chars().count()).max().unwrap_or(0);
    let color_at = |row: &str, x: usize| match row.chars().nth(x) {
        Some(TRANSPARENT) | None => None,
        Some(key) => tone(key),
    };

    rows.chunks(2)
        .map(|pair| {
            let (top, bottom) = (pair[0], pair.get(1).copied().unwrap_or(""));
            let cells: Vec<Span<'static>> = (0..width)
                .map(|x| match (color_at(top, x), color_at(bottom, x)) {
                    (None, None) => Span::from(" "),
                    (Some(top), None) => Span::styled("▀", Style::default().fg(top)),
                    (None, Some(bottom)) => Span::styled("▄", Style::default().fg(bottom)),
                    (Some(top), Some(bottom)) if top == bottom => {
                        Span::styled("█", Style::default().fg(top))
                    }
                    (Some(top), Some(bottom)) => {
                        Span::styled("▀", Style::default().fg(top).bg(bottom))
                    }
                })
                .collect();
            Line::from(cells)
        })
        .collect()
}

fn line_width(line: &Line<'static>) -> usize {
    line.spans
        .iter()
        .map(|span| UnicodeWidthStr::width(span.content.as_ref()))
        .sum()
}

/// Lays `lines` out to the right of the logo, vertically centered against it.
///
/// Returns `lines` untouched when the terminal is too narrow to seat the logo
/// plus a usable text column.
pub(crate) fn with_logo(lines: Vec<Line<'static>>, width: u16) -> Vec<Line<'static>> {
    let logo = robot::logo();
    let logo_width = logo.first().map(line_width).unwrap_or(0);
    // Keep a trailing column so text never runs into the terminal edge.
    let text_width = (width as usize).saturating_sub(logo_width + GUTTER + 1);
    if text_width == 0 {
        return lines;
    }

    let text: Vec<Line<'static>> = lines
        .into_iter()
        .map(|line| truncate_line_with_ellipsis_if_overflow(line, text_width))
        .collect();

    let top_pad = logo.len().saturating_sub(text.len()) / 2;
    let rows = logo.len().max(text.len() + top_pad);
    let blank_logo = " ".repeat(logo_width);
    let gutter = " ".repeat(GUTTER);

    (0..rows)
        .map(|row| {
            let text_line = row.checked_sub(top_pad).and_then(|i| text.get(i));
            let mut spans: Vec<Span<'static>> = match logo.get(row) {
                Some(logo_line) => logo_line.spans.clone(),
                // Text outrunning the logo still has to clear the logo column.
                None if text_line.is_some() => vec![Span::from(blank_logo.clone())],
                None => Vec::new(),
            };
            if let Some(text_line) = text_line {
                spans.push(Span::from(gutter.clone()));
                spans.extend(text_line.spans.iter().cloned());
            }
            Line::from(spans)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    fn plain(text: &str) -> Line<'static> {
        Line::from(text.to_string())
    }

    fn rendered(line: &Line<'static>) -> String {
        line.spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect()
    }

    #[test]
    fn each_text_line_lands_beside_the_logo() {
        let lines = ["Codex", "model", "directory"];

        let out = with_logo(lines.iter().map(|text| plain(text)).collect(), /*width*/ 80);

        // Deliberately derived from the logo rather than hardcoded, so swapping
        // the sprite for one of a different height doesn't break this.
        let logo = robot::logo();
        let logo_width = logo.first().map(line_width).unwrap_or(0);
        assert_eq!(
            out.len(),
            logo.len(),
            "with fewer text lines than logo rows, the logo sets the height"
        );

        for expected in lines {
            let row = out
                .iter()
                .map(rendered)
                .find(|text| text.ends_with(expected))
                .unwrap_or_else(|| panic!("{expected:?} is missing from the banner"));
            assert_eq!(
                row.chars().count(),
                logo_width + GUTTER + expected.chars().count(),
                "{expected:?} should start past the logo and its gutter"
            );
        }
    }

    #[test]
    fn narrow_terminals_fall_back_to_bare_text() {
        let out = with_logo(vec![plain("Codex")], /*width*/ 8);

        assert_eq!(out.len(), 1);
        assert_eq!(rendered(&out[0]), "Codex", "no room for a logo gutter");
    }
}
