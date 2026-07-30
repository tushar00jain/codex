//! The robot banner logo: an eight-pixel-wide grid and its amber tones.
//!
//! Data only -- replacing the banner logo means replacing this file. The
//! renderer and layout in `mod.rs` are logo-agnostic.

use super::render_grid;
use crate::color::is_light;
use crate::terminal_palette::StdoutColorLevel;
use crate::terminal_palette::best_color_for_level;
use crate::terminal_palette::default_bg;
use crate::terminal_palette::rgb_color;
use crate::terminal_palette::stdout_color_level;
use ratatui::style::Color;
use ratatui::text::Line;

const BODY: char = 'D';
const EYE: char = 'W';
const VISOR: char = 'N';

/// An antenna breaking the top silhouette, a square head, a dark visor band with
/// two lit eyes, and stubby legs.
///
/// The visor rows are duplicated so those pixels land inside single cells and
/// draw as solid blocks; splitting them across cells would thin the eyes into
/// slivers. Eight pixel rows give four text rows.
const ROWS: &[&str] = &[
    "...DD...",
    ".DDDDDD.",
    "DDDDDDDD",
    "DDDDDDDD",
    "DNWDDWND",
    "DNWDDWND",
    ".DDDDDD.",
    ".D....D.",
];

#[derive(Clone, Copy)]
struct Tones {
    body: (u8, u8, u8),
    eye: (u8, u8, u8),
    visor: (u8, u8, u8),
}

/// Amber body, near-white eyes, and the deepest shade of the hue for the visor
/// they sit in -- so the face reads without spending a fourth tone on shading.
const AMBER: Tones = Tones {
    body: (0xD8, 0x86, 0x3C),
    eye: (0xFB, 0xF6, 0xEE),
    visor: (0x3D, 0x1B, 0x05),
};

/// The body is the only tone that meets the terminal background: the grid encloses
/// the visor and eyes within it. So a light background needs just the body
/// deepened for contrast, not a second palette.
const BODY_ON_LIGHT: (u8, u8, u8) = (0xB0, 0x63, 0x1E);

/// Renders the logo for the current terminal background and color support.
pub(crate) fn logo() -> Vec<Line<'static>> {
    logo_for(default_bg(), stdout_color_level())
}

fn logo_for(
    terminal_bg: Option<(u8, u8, u8)>,
    color_level: StdoutColorLevel,
) -> Vec<Line<'static>> {
    let tones = if terminal_bg.is_some_and(is_light) {
        Tones {
            body: BODY_ON_LIGHT,
            ..AMBER
        }
    } else {
        AMBER
    };

    render_grid(ROWS, |key| {
        let rgb = match key {
            BODY => tones.body,
            EYE => tones.eye,
            VISOR => tones.visor,
            _ => return None,
        };
        Some(match color_level {
            StdoutColorLevel::TrueColor => rgb_color(rgb),
            StdoutColorLevel::Ansi256 => best_color_for_level(rgb, color_level),
            // Below 256 colors `best_color_for_level` resolves to the default
            // color, which would flatten every tone into one and erase the
            // drawing -- so name the ANSI colors explicitly instead.
            StdoutColorLevel::Ansi16 | StdoutColorLevel::Unknown => match key {
                EYE => Color::White,
                VISOR => Color::Black,
                // ANSI has no orange, and `styles.md` rules out yellow, so light
                // red is the closest read available for amber.
                _ => Color::LightRed,
            },
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn grid_is_rectangular() {
        let widths: Vec<usize> = ROWS.iter().map(|row| row.chars().count()).collect();
        assert!(
            widths.iter().all(|width| *width == widths[0]),
            "ragged rows render misaligned: {widths:?}"
        );
    }

    #[test]
    fn low_color_terminals_keep_the_tones_distinguishable() {
        for color_level in [StdoutColorLevel::Ansi16, StdoutColorLevel::Unknown] {
            let colors: HashSet<Option<Color>> = logo_for(/*terminal_bg*/ None, color_level)
                .iter()
                .flat_map(|line| line.spans.iter())
                .flat_map(|span| [span.style.fg, span.style.bg])
                .filter(|color| color.is_some())
                .collect();

            assert!(
                colors.len() >= 3,
                "{color_level:?} collapsed the drawing to {} tone(s)",
                colors.len()
            );
        }
    }
}
