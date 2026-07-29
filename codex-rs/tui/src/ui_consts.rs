//! Shared UI constants for layout and alignment within the TUI.

/// Width (in terminal columns) reserved for the left gutter/prefix used by
/// live cells and aligned widgets.
///
/// Semantics:
/// - Chat composer reserves this many columns for the left border + padding.
/// - Status indicator lines begin with this many spaces for alignment.
/// - User history lines account for this many columns (e.g., "▌ ") when wrapping.
pub(crate) const LIVE_PREFIX_COLS: u16 = 2;
pub(crate) const FOOTER_INDENT_COLS: usize = LIVE_PREFIX_COLS as usize;
pub(crate) const TRANSCRIPT_HINT: &str = "ctrl + t to view transcript";

/// The chevron pointer: the composer's input prompt, the prefix on sent
/// messages in history, and the cursor marking the highlighted row in
/// selection lists and pickers. One glyph so they all read as the same mark.
pub(crate) const CHEVRON: &str = "❯";
