// This is derived from `ratatui::Terminal`, which is licensed under the following terms:
//
// The MIT License (MIT)
// Copyright (c) 2016-2022 Florian Dehau
// Copyright (c) 2023-2025 The Ratatui Developers
//
// Permission is hereby granted, free of charge, to any person obtaining a copy
// of this software and associated documentation files (the "Software"), to deal
// in the Software without restriction, including without limitation the rights
// to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
// copies of the Software, and to permit persons to whom the Software is
// furnished to do so, subject to the following conditions:
//
// The above copyright notice and this permission notice shall be included in all
// copies or substantial portions of the Software.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
// IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
// OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
// SOFTWARE.
use std::io;
use std::io::Write;

use crossterm::cursor::MoveTo;
use crossterm::cursor::SetCursorStyle;
use crossterm::queue;
use crossterm::style::Colors;
use crossterm::style::Print;
use crossterm::style::SetAttribute;
use crossterm::style::SetBackgroundColor;
use crossterm::style::SetColors;
use crossterm::style::SetForegroundColor;
use crossterm::terminal::Clear;
use derive_more::IsVariant;
use ratatui::backend::Backend;
use ratatui::backend::ClearType;
use ratatui::buffer::Buffer;
use ratatui::layout::Position;
use ratatui::layout::Rect;
use ratatui::layout::Size;
use ratatui::style::Color;
use ratatui::style::Modifier;
use ratatui::widgets::WidgetRef;
use unicode_width::UnicodeWidthStr;

/// Returns the display width of a cell symbol, ignoring OSC escape sequences.
///
/// OSC sequences (e.g. OSC 8 hyperlinks: `\x1B]8;;URL\x07`) are terminal
/// control sequences that don't consume display columns.  The standard
/// `UnicodeWidthStr::width()` method incorrectly counts the printable
/// characters inside OSC payloads (like `]`, `8`, `;`, and URL characters).
/// This function strips them first so that only visible characters contribute
/// to the width.
fn display_width(s: &str) -> usize {
    // Fast path: no escape sequences present.
    if !s.contains('\x1B') {
        return s.width();
    }

    // Strip OSC sequences: ESC ] ... BEL
    let mut visible = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(ch) = chars.next() {
        if ch == '\x1B' && chars.clone().next() == Some(']') {
            // Consume the ']' and everything up to and including BEL.
            chars.next(); // skip ']'
            for c in chars.by_ref() {
                if c == '\x07' {
                    break;
                }
            }
            continue;
        }
        visible.push(ch);
    }
    visible.width()
}

fn osc8_hyperlink_parts(symbol: &str) -> Option<(&str, &str)> {
    let content = symbol.strip_prefix("\x1b]8;;")?;
    let destination_end = content.find('\x07')?;
    let destination = &content[..destination_end];
    if destination.is_empty() {
        return None;
    }
    let visible = content[destination_end + 1..].strip_suffix("\x1b]8;;\x07")?;
    Some((destination, visible))
}

pub struct Frame<'a> {
    /// Where should the cursor be after drawing this frame?
    ///
    /// If `None`, the cursor is hidden and its position is controlled by the backend. If `Some((x,
    /// y))`, the cursor is shown and placed at `(x, y)` after the call to `Terminal::draw()`.
    pub(crate) cursor_position: Option<Position>,

    /// Visible cursor shape to apply after drawing this frame.
    cursor_style: SetCursorStyle,

    /// The area of the viewport
    pub(crate) viewport_area: Rect,

    /// The buffer that is used to draw the current frame
    pub(crate) buffer: &'a mut Buffer,
}

impl Frame<'_> {
    /// The area of the current frame
    ///
    /// This is guaranteed not to change during rendering, so may be called multiple times.
    ///
    /// If your app listens for a resize event from the backend, it should ignore the values from
    /// the event for any calculations that are used to render the current frame and use this value
    /// instead as this is the area of the buffer that is used to render the current frame.
    pub const fn area(&self) -> Rect {
        self.viewport_area
    }

    /// Render a [`WidgetRef`] to the current buffer using [`WidgetRef::render_ref`].
    ///
    /// Usually the area argument is the size of the current frame or a sub-area of the current
    /// frame (which can be obtained using [`Layout`] to split the total area).
    #[allow(clippy::needless_pass_by_value)]
    pub fn render_widget_ref<W: WidgetRef>(&mut self, widget: W, area: Rect) {
        widget.render_ref(area, self.buffer);
    }

    /// After drawing this frame, make the cursor visible and put it at the specified (x, y)
    /// coordinates. If this method is not called, the cursor will be hidden.
    ///
    /// Note that this will interfere with calls to [`Terminal::hide_cursor`],
    /// [`Terminal::show_cursor`], and [`Terminal::set_cursor_position`]. Pick one of the APIs and
    /// stick with it.
    ///
    /// [`Terminal::hide_cursor`]: crate::Terminal::hide_cursor
    /// [`Terminal::show_cursor`]: crate::Terminal::show_cursor
    /// [`Terminal::set_cursor_position`]: crate::Terminal::set_cursor_position
    pub fn set_cursor_position<P: Into<Position>>(&mut self, position: P) {
        self.cursor_position = Some(position.into());
    }

    /// After drawing this frame, set the terminal's visible cursor style.
    pub fn set_cursor_style(&mut self, style: SetCursorStyle) {
        self.cursor_style = style;
    }

    /// Gets the buffer that this `Frame` draws into as a mutable reference.
    pub fn buffer_mut(&mut self) -> &mut Buffer {
        self.buffer
    }
}

#[derive(Debug, Default, Clone, Eq, PartialEq, Hash)]
pub struct Terminal<B>
where
    B: Backend + Write,
{
    /// The backend used to interface with the terminal
    backend: B,
    /// Holds the results of the current and previous draw calls. The two are compared at the end
    /// of each draw pass to output the necessary updates to the terminal
    buffers: [Buffer; 2],
    /// Index of the current buffer in the previous array
    current: usize,
    /// Whether the cursor is currently hidden
    pub hidden_cursor: bool,
    /// Area of the viewport
    pub viewport_area: Rect,
    /// Last known size of the terminal. Used to detect if the internal buffers have to be resized.
    pub last_known_screen_size: Size,
    /// Last known position of the cursor. Used to find the new area when the viewport is inlined
    /// and the terminal resized.
    pub last_known_cursor_pos: Position,
    /// Row where this session's history starts: the cursor row at launch.
    ///
    /// Rows above it belong to whatever was on screen before Codex started, so
    /// history must never be written there.
    history_origin_row: u16,
    /// First screen row below this session's history -- where the next history row
    /// goes. An absolute row rather than a count, because a count cannot survive the
    /// viewport growing upward: clamping it to the new viewport top silently forgets
    /// that rows below the top still hold history, and the viewport then paints over
    /// them instead of scrolling them away.
    ///
    /// Invariant: `history_origin_row <= history_end_row <= viewport_area.top()`.
    history_end_row: u16,
    /// Whether the inline viewport has been dropped to the bottom of the screen.
    bottom_anchored: bool,
    #[cfg(test)]
    screen_size_override: Option<Size>,
}

impl<B> Drop for Terminal<B>
where
    B: Backend,
    B: Write,
{
    #[allow(clippy::print_stderr)]
    fn drop(&mut self) {
        // Attempt to restore the cursor state
        if let Err(err) = self.reset_cursor_style() {
            eprintln!("Failed to reset the cursor style: {err}");
        }

        if self.hidden_cursor
            && let Err(err) = self.show_cursor()
        {
            eprintln!("Failed to show the cursor: {err}");
        }
    }
}

impl<B> Terminal<B>
where
    B: Backend,
    B: Write,
{
    /// Creates a new [`Terminal`] with the given [`Backend`] and [`TerminalOptions`].
    pub fn with_options(mut backend: B) -> io::Result<Self> {
        let screen_size = backend.size()?;
        let cursor_pos = backend.get_cursor_position().unwrap_or_else(|err| {
            // Some PTYs do not answer CPR (`ESC[6n`); continue with a safe default instead
            // of failing TUI startup.
            tracing::warn!("failed to read initial cursor position; defaulting to origin: {err}");
            Position { x: 0, y: 0 }
        });
        Ok(Self::with_screen_size_and_cursor_position(
            backend,
            screen_size,
            cursor_pos,
        ))
    }

    /// Creates a new [`Terminal`] from a caller-provided initial cursor position.
    ///
    /// Startup code uses this when cursor probing has already happened outside the backend, for
    /// example through a bounded terminal probe. Supplying a stale or synthetic position changes
    /// the inline viewport anchor, so callers should only use this after they have chosen the same
    /// fallback they want the first render to honor.
    pub fn with_options_and_cursor_position(backend: B, cursor_pos: Position) -> io::Result<Self> {
        let screen_size = backend.size()?;
        Ok(Self::with_screen_size_and_cursor_position(
            backend,
            screen_size,
            cursor_pos,
        ))
    }

    fn with_screen_size_and_cursor_position(
        backend: B,
        screen_size: Size,
        cursor_pos: Position,
    ) -> Self {
        Self {
            backend,
            buffers: [Buffer::empty(Rect::ZERO), Buffer::empty(Rect::ZERO)],
            current: 0,
            hidden_cursor: false,
            viewport_area: Rect::new(
                /*x*/ 0,
                cursor_pos.y,
                /*width*/ 0,
                /*height*/ 0,
            ),
            last_known_screen_size: screen_size,
            last_known_cursor_pos: cursor_pos,
            history_origin_row: cursor_pos.y,
            history_end_row: cursor_pos.y,
            bottom_anchored: false,
            #[cfg(test)]
            screen_size_override: None,
        }
    }

    #[cfg(test)]
    pub(crate) fn with_screen_size_and_cursor_position_for_test(
        backend: B,
        screen_size: Size,
        cursor_pos: Position,
    ) -> Self {
        let mut terminal =
            Self::with_screen_size_and_cursor_position(backend, screen_size, cursor_pos);
        terminal.screen_size_override = Some(screen_size);
        terminal
    }

    /// Get a Frame object which provides a consistent view into the terminal state for rendering.
    pub fn get_frame(&mut self) -> Frame<'_> {
        Frame {
            cursor_position: None,
            cursor_style: SetCursorStyle::DefaultUserShape,
            viewport_area: self.viewport_area,
            buffer: self.current_buffer_mut(),
        }
    }

    /// Gets the current buffer as a reference.
    fn current_buffer(&self) -> &Buffer {
        &self.buffers[self.current]
    }

    /// Gets the current buffer as a mutable reference.
    fn current_buffer_mut(&mut self) -> &mut Buffer {
        &mut self.buffers[self.current]
    }

    /// Gets the previous buffer as a reference.
    fn previous_buffer(&self) -> &Buffer {
        &self.buffers[1 - self.current]
    }

    /// Gets the previous buffer as a mutable reference.
    fn previous_buffer_mut(&mut self) -> &mut Buffer {
        &mut self.buffers[1 - self.current]
    }

    /// Gets the backend
    pub const fn backend(&self) -> &B {
        &self.backend
    }

    /// Gets the backend as a mutable reference
    pub fn backend_mut(&mut self) -> &mut B {
        &mut self.backend
    }

    /// Obtains a difference between the previous and the current buffer and passes it to the
    /// current backend for drawing.
    pub fn flush(&mut self) -> io::Result<()> {
        let updates = diff_buffers(self.previous_buffer(), self.current_buffer());
        let last_put_command = updates.iter().rfind(|command| command.is_put());
        if let Some(&DrawCommand::Put { x, y, .. }) = last_put_command {
            self.last_known_cursor_pos = Position { x, y };
        }
        draw(&mut self.backend, updates.into_iter())
    }

    /// Updates the Terminal so that internal buffers match the requested area.
    ///
    /// Requested area will be saved to remain consistent when rendering. This leads to a full clear
    /// of the screen.
    pub fn resize(&mut self, screen_size: Size) -> io::Result<()> {
        self.last_known_screen_size = screen_size;
        Ok(())
    }

    /// Sets the viewport area.
    pub fn set_viewport_area(&mut self, area: Rect) {
        self.current_buffer_mut().resize(area);
        self.previous_buffer_mut().resize(area);
        self.viewport_area = area;
        // Callers must scroll any overlap away before moving the viewport up over
        // history (see `history_end_row`), so this only enforces the invariant.
        self.history_end_row = self.history_end_row.min(area.top());
    }

    /// Queries the backend for size and resizes if it doesn't match the previous size.
    pub fn autoresize(&mut self) -> io::Result<()> {
        let screen_size = self.size()?;
        if screen_size != self.last_known_screen_size {
            self.resize(screen_size)?;
        }
        Ok(())
    }

    /// Draws a single frame to the terminal.
    ///
    /// Returns a [`CompletedFrame`] if successful, otherwise a [`std::io::Error`].
    ///
    /// If the render callback passed to this method can fail, use [`try_draw`] instead.
    ///
    /// Applications should call `draw` or [`try_draw`] in a loop to continuously render the
    /// terminal. These methods are the main entry points for drawing to the terminal.
    ///
    /// [`try_draw`]: Terminal::try_draw
    ///
    /// This method will:
    ///
    /// - autoresize the terminal if necessary
    /// - call the render callback, passing it a [`Frame`] reference to render to
    /// - flush the current internal state by copying the current buffer to the backend
    /// - move the cursor to the last known position if it was set during the rendering closure
    ///
    /// The render callback should fully render the entire frame when called, including areas that
    /// are unchanged from the previous frame. This is because each frame is compared to the
    /// previous frame to determine what has changed, and only the changes are written to the
    /// terminal. If the render callback does not fully render the frame, the terminal will not be
    /// in a consistent state.
    pub fn draw<F>(&mut self, render_callback: F) -> io::Result<()>
    where
        F: FnOnce(&mut Frame),
    {
        self.try_draw(|frame| {
            render_callback(frame);
            io::Result::Ok(())
        })
    }

    /// Tries to draw a single frame to the terminal.
    ///
    /// Returns [`Result::Ok`] containing a [`CompletedFrame`] if successful, otherwise
    /// [`Result::Err`] containing the [`std::io::Error`] that caused the failure.
    ///
    /// This is the equivalent of [`Terminal::draw`] but the render callback is a function or
    /// closure that returns a `Result` instead of nothing.
    ///
    /// Applications should call `try_draw` or [`draw`] in a loop to continuously render the
    /// terminal. These methods are the main entry points for drawing to the terminal.
    ///
    /// [`draw`]: Terminal::draw
    ///
    /// This method will:
    ///
    /// - autoresize the terminal if necessary
    /// - call the render callback, passing it a [`Frame`] reference to render to
    /// - flush the current internal state by copying the current buffer to the backend
    /// - move the cursor to the last known position if it was set during the rendering closure
    /// - return a [`CompletedFrame`] with the current buffer and the area of the terminal
    ///
    /// The render callback passed to `try_draw` can return any [`Result`] with an error type that
    /// can be converted into an [`std::io::Error`] using the [`Into`] trait. This makes it possible
    /// to use the `?` operator to propagate errors that occur during rendering. If the render
    /// callback returns an error, the error will be returned from `try_draw` as an
    /// [`std::io::Error`] and the terminal will not be updated.
    ///
    /// The [`CompletedFrame`] returned by this method can be useful for debugging or testing
    /// purposes, but it is often not used in regular applicationss.
    ///
    /// The render callback should fully render the entire frame when called, including areas that
    /// are unchanged from the previous frame. This is because each frame is compared to the
    /// previous frame to determine what has changed, and only the changes are written to the
    /// terminal. If the render function does not fully render the frame, the terminal will not be
    /// in a consistent state.
    pub fn try_draw<F, E>(&mut self, render_callback: F) -> io::Result<()>
    where
        F: FnOnce(&mut Frame) -> Result<(), E>,
        E: Into<io::Error>,
    {
        // Autoresize - otherwise we get glitches if shrinking or potential desync between widgets
        // and the terminal (if growing), which may OOB.
        self.autoresize()?;

        let mut frame = self.get_frame();

        render_callback(&mut frame).map_err(Into::into)?;

        // We can't change the cursor position right away because we have to flush the frame to
        // stdout first. But we also can't keep the frame around, since it holds a &mut to
        // Buffer. Thus, we're taking the important data out of the Frame and dropping it.
        let cursor_position = frame.cursor_position;
        let cursor_style = frame.cursor_style;

        // Draw to stdout
        self.flush()?;

        match cursor_position {
            None => self.hide_cursor()?,
            Some(position) => {
                self.set_cursor_style(cursor_style)?;
                self.show_cursor()?;
                self.set_cursor_position(position)?;
            }
        }

        self.swap_buffers();

        Backend::flush(&mut self.backend)?;

        Ok(())
    }

    /// Hides the cursor.
    pub fn hide_cursor(&mut self) -> io::Result<()> {
        self.backend.hide_cursor()?;
        self.hidden_cursor = true;
        Ok(())
    }

    /// Shows the cursor.
    pub fn show_cursor(&mut self) -> io::Result<()> {
        self.backend.show_cursor()?;
        self.hidden_cursor = false;
        Ok(())
    }

    /// Sets the visible terminal cursor style.
    pub fn set_cursor_style(&mut self, style: SetCursorStyle) -> io::Result<()> {
        queue!(self.backend, style)
    }

    /// Restores the user-configured terminal cursor style.
    pub fn reset_cursor_style(&mut self) -> io::Result<()> {
        self.set_cursor_style(SetCursorStyle::DefaultUserShape)
    }

    /// Gets the current cursor position.
    ///
    /// This is the position of the cursor after the last draw call.
    #[allow(dead_code)]
    pub fn get_cursor_position(&mut self) -> io::Result<Position> {
        self.backend.get_cursor_position()
    }

    /// Sets the cursor position.
    pub fn set_cursor_position<P: Into<Position>>(&mut self, position: P) -> io::Result<()> {
        let position = position.into();
        self.backend.set_cursor_position(position)?;
        self.last_known_cursor_pos = position;
        Ok(())
    }

    /// Clear the terminal and force a full redraw on the next draw call.
    pub fn clear(&mut self) -> io::Result<()> {
        if self.viewport_area.is_empty() {
            return Ok(());
        }
        self.clear_after_position(self.viewport_area.as_position())
    }

    /// Clear from `position` through the end of the visible screen and force a full redraw.
    pub(crate) fn clear_after_position(&mut self, position: Position) -> io::Result<()> {
        self.backend.set_cursor_position(position)?;
        self.backend.clear_region(ClearType::AfterCursor)?;
        // Reset the back buffer to make sure the next update will redraw everything.
        self.previous_buffer_mut().reset();
        Ok(())
    }

    /// Force the next draw pass to repaint the entire viewport by resetting the
    /// diff buffer. Call this after raw terminal operations that move screen
    /// content outside ratatui's knowledge.
    pub fn invalidate_viewport(&mut self) {
        self.previous_buffer_mut().reset();
    }

    /// Clear the entire visible screen (not just the viewport) and force a full redraw.
    pub fn clear_visible_screen(&mut self) -> io::Result<()> {
        let home = Position { x: 0, y: 0 };
        // Some terminals (notably Terminal.app) behave more reliably if we pair ED2
        // with an explicit cursor-home before/after, matching the common `clear`
        // sequence (`CSI 2J` + `CSI H`).
        self.set_cursor_position(home)?;
        self.backend.clear_region(ClearType::All)?;
        self.set_cursor_position(home)?;
        std::io::Write::flush(&mut self.backend)?;
        // The screen is blank now, so history may start from the very top again.
        self.history_origin_row = 0;
        self.history_end_row = 0;
        self.previous_buffer_mut().reset();
        Ok(())
    }

    /// Hard-reset scrollback + visible screen using an explicit ANSI sequence.
    ///
    /// Some terminals behave more reliably when purge + clear are emitted as a
    /// single ANSI sequence instead of separate backend commands.
    pub fn clear_scrollback_and_visible_screen_ansi(&mut self) -> io::Result<()> {
        if self.viewport_area.is_empty() {
            return Ok(());
        }

        // Reset scroll region + style state, home cursor, clear screen, purge scrollback.
        // The order matches the common shell `clear && printf '\\e[3J'` behavior.
        write!(self.backend, "\x1b[r\x1b[0m\x1b[H\x1b[2J\x1b[3J\x1b[H")?;
        std::io::Write::flush(&mut self.backend)?;
        self.last_known_cursor_pos = Position { x: 0, y: 0 };
        // The screen is blank now, so history may start from the very top again.
        self.history_origin_row = 0;
        self.history_end_row = 0;
        self.previous_buffer_mut().reset();
        Ok(())
    }

    pub(crate) fn note_history_rows_inserted(&mut self, inserted_rows: u16) {
        self.history_end_row = self
            .history_end_row
            .saturating_add(inserted_rows)
            .min(self.viewport_area.top());
    }

    /// Records that the region above the viewport scrolled up by `rows`, moving this
    /// session's history up with it.
    pub(crate) fn note_history_scrolled(&mut self, rows: u16) {
        self.history_end_row = self.history_end_row.saturating_sub(rows);
        self.history_origin_row = self.history_origin_row.min(self.history_end_row);
    }

    /// First screen row below this session's history.
    pub(crate) fn history_end_row(&self) -> u16 {
        self.history_end_row
    }

    /// Row the next history line should be written to while blank rows remain
    /// between the last history row and the viewport.
    ///
    /// `None` once history reaches the viewport, meaning callers must scroll the
    /// region above instead.
    pub(crate) fn next_history_row(&self) -> Option<u16> {
        (self.history_end_row < self.viewport_area.top()).then_some(self.history_end_row)
    }

    /// Frees the screen by scrolling what was already on it into scrollback, so the
    /// banner can sit near the top and the composer at the bottom.
    ///
    /// Scrolls by exactly the launch cursor row -- the number of occupied rows --
    /// using newlines on the last row, which is what makes a terminal commit the
    /// departing row to scrollback. (`SU`/`ESC[nS` scrolls but discards those rows,
    /// which silently ate the user's shell prompt.) Contrast walking to the bottom
    /// with a screenful of newlines: on a nearly empty screen that pushes one real
    /// line plus a screenful of blanks into scrollback and makes it useless.
    ///
    /// History then starts at `TOP_MARGIN_ROWS`, not row 0: some terminals pin the
    /// last shell prompt to the top of the viewport (tscode does), and anything drawn
    /// on row 0 renders underneath it. One row of margin costs nothing and keeps the
    /// banner visible everywhere.
    pub(crate) fn claim_screen(&mut self) -> io::Result<()> {
        /// Rows left untouched at the top of the screen.
        const TOP_MARGIN_ROWS: u16 = 1;

        let occupied_rows = self.history_origin_row;
        if occupied_rows > 0 {
            let last_row = self.last_known_screen_size.height.saturating_sub(1);
            queue!(self.backend, MoveTo(/*x*/ 0, last_row))?;
            for _ in 0..occupied_rows {
                queue!(self.backend, Print("\n"))?;
            }
            std::io::Write::flush(&mut self.backend)?;
        }
        // The screen is blank now; everything shifted up by `occupied_rows`.
        self.viewport_area.y = self
            .viewport_area
            .y
            .saturating_sub(occupied_rows)
            .max(TOP_MARGIN_ROWS);
        self.history_origin_row = TOP_MARGIN_ROWS;
        // The screen is blank now, so history has not reached any row yet. Leaving
        // this at the launch cursor row would make the bottom anchor believe history
        // already exists and fire on the first draw, dragging the not-yet-committed
        // session header down with the composer -- and it would report a gap over
        // rows that were just blanked.
        self.history_end_row = TOP_MARGIN_ROWS;
        self.last_known_cursor_pos = Position {
            x: 0,
            y: TOP_MARGIN_ROWS,
        };
        Ok(())
    }

    /// Row the viewport should sit at to be flush with the bottom of the screen, or
    /// `None` while it should stay where it is.
    ///
    /// Seats the composer at the bottom once history has reached scrollback, then
    /// keeps it there as its height changes. Letting a shrinking composer float
    /// upward leaves dead rows beneath it, and it also stops history insertion from
    /// recognizing the gap: `insert_history` decides the viewport is bottom-anchored
    /// by comparing `area.bottom()` against the screen height, so a floating viewport
    /// takes the push-the-viewport-down branch and writes just above the composer
    /// instead of filling the gap.
    pub(crate) fn bottom_aligned_y(&mut self, height: u16, screen_height: u16) -> Option<u16> {
        if let Some(bottom_y) = self.take_bottom_anchor(height, screen_height) {
            return Some(bottom_y);
        }
        self.bottom_anchored
            .then(|| screen_height.saturating_sub(height))
    }

    /// Drops the viewport to the bottom of the screen, once, at the first draw.
    ///
    /// Returns the new `y`, or `None` if it has already been claimed or the
    /// viewport is at or below that row. Rows between the launch cursor and the
    /// bottom are blank, so this claims them without scrolling -- anything on
    /// screen above the launch point stays put. It never moves the viewport up,
    /// leaving a launch near the bottom to the caller's overflow handling.
    pub(crate) fn take_bottom_anchor(&mut self, height: u16, screen_height: u16) -> Option<u16> {
        if self.bottom_anchored {
            return None;
        }
        // Wait for the first history to reach scrollback. Before that the session
        // header is still inside the viewport, so anchoring would drag it to the
        // bottom along with the composer and open the gap above it instead of
        // between them. Deliberately does not consume the flag.
        if self.history_end_row == self.history_origin_row {
            return None;
        }
        self.bottom_anchored = true;
        let bottom_y = screen_height.saturating_sub(height);
        (bottom_y > self.viewport_area.y).then_some(bottom_y)
    }

    /// Clears the inactive buffer and swaps it with the current buffer
    pub fn swap_buffers(&mut self) {
        self.previous_buffer_mut().reset();
        self.current = 1 - self.current;
    }

    /// Queries the real size of the backend.
    pub fn size(&self) -> io::Result<Size> {
        #[cfg(test)]
        if let Some(size) = self.screen_size_override {
            return Ok(size);
        }
        self.backend.size()
    }
}

use ratatui::buffer::Cell;

#[derive(Debug, IsVariant)]
enum DrawCommand {
    Put { x: u16, y: u16, cell: Cell },
    ClearToEnd { x: u16, y: u16, bg: Color },
}

fn diff_buffers(a: &Buffer, b: &Buffer) -> Vec<DrawCommand> {
    let previous_buffer = &a.content;
    let next_buffer = &b.content;

    let mut updates = vec![];
    let mut last_nonblank_columns = vec![0; a.area.height as usize];
    for y in 0..a.area.height {
        let row_start = y as usize * a.area.width as usize;
        let row_end = row_start + a.area.width as usize;
        let row = &next_buffer[row_start..row_end];
        let bg = row.last().map(|cell| cell.bg).unwrap_or(Color::Reset);

        // Scan the row to find the rightmost column that still matters: any non-space glyph,
        // any cell whose bg differs from the row’s trailing bg, or any cell with modifiers.
        // Multi-width glyphs extend that region through their full displayed width.
        // After that point the rest of the row can be cleared with a single ClearToEnd, a perf win
        // versus emitting multiple space Put commands.
        let mut last_nonblank_column = 0usize;
        let mut column = 0usize;
        while column < row.len() {
            let cell = &row[column];
            let width = display_width(cell.symbol());
            if cell.symbol() != " " || cell.bg != bg || cell.modifier != Modifier::empty() {
                last_nonblank_column = column + (width.saturating_sub(1));
            }
            column += width.max(1); // treat zero-width symbols as width 1
        }

        if last_nonblank_column + 1 < row.len() {
            let (x, y) = a.pos_of(row_start + last_nonblank_column + 1);
            updates.push(DrawCommand::ClearToEnd { x, y, bg });
        }

        last_nonblank_columns[y as usize] = last_nonblank_column as u16;
    }

    // Cells invalidated by drawing/replacing preceding multi-width characters:
    let mut invalidated: usize = 0;
    // Cells from the current buffer to skip due to preceding multi-width characters taking
    // their place (the skipped cells should be blank anyway), or due to per-cell-skipping:
    let mut to_skip: usize = 0;
    for (i, (current, previous)) in next_buffer.iter().zip(previous_buffer.iter()).enumerate() {
        if !current.skip && (current != previous || invalidated > 0) && to_skip == 0 {
            let (x, y) = a.pos_of(i);
            let row = i / a.area.width as usize;
            if x <= last_nonblank_columns[row] {
                updates.push(DrawCommand::Put {
                    x,
                    y,
                    cell: next_buffer[i].clone(),
                });
            }
        }

        to_skip = display_width(current.symbol()).saturating_sub(1);

        let affected_width = std::cmp::max(
            display_width(current.symbol()),
            display_width(previous.symbol()),
        );
        invalidated = std::cmp::max(affected_width, invalidated).saturating_sub(1);
    }
    updates
}

fn draw<I>(writer: &mut impl Write, commands: I) -> io::Result<()>
where
    I: Iterator<Item = DrawCommand>,
{
    let mut fg = Color::Reset;
    let mut bg = Color::Reset;
    let mut modifier = Modifier::empty();
    let mut last_pos: Option<Position> = None;
    let mut active_hyperlink: Option<String> = None;
    for command in commands {
        let (x, y) = match &command {
            DrawCommand::Put { x, y, .. } => (x, y),
            DrawCommand::ClearToEnd { x, y, .. } => (x, y),
        };
        let hyperlink = match &command {
            DrawCommand::Put { cell, .. } => osc8_hyperlink_parts(cell.symbol()),
            DrawCommand::ClearToEnd { .. } => None,
        };
        let destination = hyperlink.map(|(destination, _)| destination);
        let hyperlink_changed = active_hyperlink.as_deref() != destination;
        if hyperlink_changed && active_hyperlink.is_some() {
            queue!(writer, Print("\x1b]8;;\x07"))?;
        }
        // Move the cursor if the previous location was not (x - 1, y)
        if !matches!(last_pos, Some(p) if *x == p.x + 1 && *y == p.y) {
            queue!(writer, MoveTo(*x, *y))?;
        }
        last_pos = Some(Position { x: *x, y: *y });
        match &command {
            DrawCommand::Put { cell, .. } => {
                if cell.modifier != modifier {
                    let diff = ModifierDiff {
                        from: modifier,
                        to: cell.modifier,
                    };
                    diff.queue(writer)?;
                    modifier = cell.modifier;
                }
                if cell.fg != fg || cell.bg != bg {
                    queue!(
                        writer,
                        SetColors(Colors::new(cell.fg.into(), cell.bg.into()))
                    )?;
                    fg = cell.fg;
                    bg = cell.bg;
                }

                if hyperlink_changed && let Some(destination) = destination {
                    queue!(writer, Print(format!("\x1b]8;;{destination}\x07")))?;
                }
                let symbol = hyperlink.map_or_else(|| cell.symbol(), |(_, visible)| visible);
                queue!(writer, Print(symbol))?;
            }
            DrawCommand::ClearToEnd { bg: clear_bg, .. } => {
                queue!(writer, SetAttribute(crossterm::style::Attribute::Reset))?;
                modifier = Modifier::empty();
                queue!(writer, SetBackgroundColor((*clear_bg).into()))?;
                bg = *clear_bg;
                queue!(writer, Clear(crossterm::terminal::ClearType::UntilNewLine))?;
            }
        }
        if hyperlink_changed {
            active_hyperlink = destination.map(str::to_owned);
        }
    }
    if active_hyperlink.is_some() {
        queue!(writer, Print("\x1b]8;;\x07"))?;
    }

    queue!(
        writer,
        SetForegroundColor(crossterm::style::Color::Reset),
        SetBackgroundColor(crossterm::style::Color::Reset),
        SetAttribute(crossterm::style::Attribute::Reset),
    )?;

    Ok(())
}

/// The `ModifierDiff` struct is used to calculate the difference between two `Modifier`
/// values. This is useful when updating the terminal display, as it allows for more
/// efficient updates by only sending the necessary changes.
struct ModifierDiff {
    pub from: Modifier,
    pub to: Modifier,
}

impl ModifierDiff {
    fn queue<W: io::Write>(self, w: &mut W) -> io::Result<()> {
        use crossterm::style::Attribute as CAttribute;
        let removed = self.from - self.to;
        if removed.contains(Modifier::REVERSED) {
            queue!(w, SetAttribute(CAttribute::NoReverse))?;
        }
        if removed.contains(Modifier::BOLD) {
            queue!(w, SetAttribute(CAttribute::NormalIntensity))?;
            if self.to.contains(Modifier::DIM) {
                queue!(w, SetAttribute(CAttribute::Dim))?;
            }
        }
        if removed.contains(Modifier::ITALIC) {
            queue!(w, SetAttribute(CAttribute::NoItalic))?;
        }
        if removed.contains(Modifier::UNDERLINED) {
            queue!(w, SetAttribute(CAttribute::NoUnderline))?;
        }
        if removed.contains(Modifier::DIM) {
            queue!(w, SetAttribute(CAttribute::NormalIntensity))?;
        }
        if removed.contains(Modifier::CROSSED_OUT) {
            queue!(w, SetAttribute(CAttribute::NotCrossedOut))?;
        }
        if removed.contains(Modifier::SLOW_BLINK) || removed.contains(Modifier::RAPID_BLINK) {
            queue!(w, SetAttribute(CAttribute::NoBlink))?;
        }

        let added = self.to - self.from;
        if added.contains(Modifier::REVERSED) {
            queue!(w, SetAttribute(CAttribute::Reverse))?;
        }
        if added.contains(Modifier::BOLD) {
            queue!(w, SetAttribute(CAttribute::Bold))?;
        }
        if added.contains(Modifier::ITALIC) {
            queue!(w, SetAttribute(CAttribute::Italic))?;
        }
        if added.contains(Modifier::UNDERLINED) {
            queue!(w, SetAttribute(CAttribute::Underlined))?;
        }
        if added.contains(Modifier::DIM) {
            queue!(w, SetAttribute(CAttribute::Dim))?;
        }
        if added.contains(Modifier::CROSSED_OUT) {
            queue!(w, SetAttribute(CAttribute::CrossedOut))?;
        }
        if added.contains(Modifier::SLOW_BLINK) {
            queue!(w, SetAttribute(CAttribute::SlowBlink))?;
        }
        if added.contains(Modifier::RAPID_BLINK) {
            queue!(w, SetAttribute(CAttribute::RapidBlink))?;
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use ratatui::backend::WindowSize;
    use ratatui::layout::Rect;
    use ratatui::style::Style;
    use ratatui::style::Stylize;
    use ratatui::text::Line;
    use ratatui::widgets::Paragraph;
    use ratatui::widgets::Widget;
    use ratatui::widgets::Wrap;

    struct CaptureBackend {
        output: Vec<u8>,
        size: Size,
        cursor: Position,
    }

    impl CaptureBackend {
        fn new(width: u16, height: u16) -> Self {
            Self {
                output: Vec::new(),
                size: Size { width, height },
                cursor: Position { x: 0, y: 0 },
            }
        }

        fn output(&self) -> String {
            String::from_utf8_lossy(&self.output).into_owned()
        }
    }

    impl Write for CaptureBackend {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.output.extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl Backend for CaptureBackend {
        fn draw<'a, I>(&mut self, _content: I) -> io::Result<()>
        where
            I: Iterator<Item = (u16, u16, &'a Cell)>,
        {
            Ok(())
        }

        fn hide_cursor(&mut self) -> io::Result<()> {
            Ok(())
        }

        fn show_cursor(&mut self) -> io::Result<()> {
            Ok(())
        }

        fn get_cursor_position(&mut self) -> io::Result<Position> {
            Ok(self.cursor)
        }

        fn set_cursor_position<P: Into<Position>>(&mut self, position: P) -> io::Result<()> {
            self.cursor = position.into();
            Ok(())
        }

        fn clear(&mut self) -> io::Result<()> {
            Ok(())
        }

        fn clear_region(&mut self, _clear_type: ClearType) -> io::Result<()> {
            Ok(())
        }

        fn append_lines(&mut self, _line_count: u16) -> io::Result<()> {
            Ok(())
        }

        fn scroll_region_up(
            &mut self,
            _region: std::ops::Range<u16>,
            _scroll_by: u16,
        ) -> io::Result<()> {
            Ok(())
        }

        fn scroll_region_down(
            &mut self,
            _region: std::ops::Range<u16>,
            _scroll_by: u16,
        ) -> io::Result<()> {
            Ok(())
        }

        fn size(&self) -> io::Result<Size> {
            Ok(self.size)
        }

        fn window_size(&mut self) -> io::Result<WindowSize> {
            Ok(WindowSize {
                columns_rows: self.size,
                pixels: self.size,
            })
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn diff_buffers_does_not_emit_clear_to_end_for_full_width_row() {
        let area = Rect::new(0, 0, 3, 2);
        let previous = Buffer::empty(area);
        let mut next = Buffer::empty(area);

        next.cell_mut((2, 0))
            .expect("cell should exist")
            .set_symbol("X");

        let commands = diff_buffers(&previous, &next);

        let clear_count = commands
            .iter()
            .filter(|command| matches!(command, DrawCommand::ClearToEnd { y, .. } if *y == 0))
            .count();
        assert_eq!(
            0, clear_count,
            "expected diff_buffers not to emit ClearToEnd; commands: {commands:?}",
        );
        assert!(
            commands
                .iter()
                .any(|command| matches!(command, DrawCommand::Put { x: 2, y: 0, .. })),
            "expected diff_buffers to update the final cell; commands: {commands:?}",
        );
    }

    #[test]
    fn diff_buffers_clear_to_end_starts_after_wide_char() {
        let area = Rect::new(0, 0, 10, 1);
        let mut previous = Buffer::empty(area);
        let mut next = Buffer::empty(area);

        previous.set_string(0, 0, "中文", Style::default());
        next.set_string(0, 0, "中", Style::default());

        let commands = diff_buffers(&previous, &next);
        assert!(
            commands
                .iter()
                .any(|command| matches!(command, DrawCommand::ClearToEnd { x: 2, y: 0, .. })),
            "expected clear-to-end to start after the remaining wide char; commands: {commands:?}"
        );
    }

    #[test]
    fn terminal_draw_coalesces_wrapped_hyperlink_output() {
        let auth_url = format!(
            "https://auth.openai.com/oauth/authorize?response_type=code&state={}",
            "x".repeat(/*n*/ 400)
        );
        let width = 44;
        let height = 20;
        let area = Rect::new(0, 0, width, height);
        let mut terminal =
            Terminal::with_options(CaptureBackend::new(width, height)).expect("terminal");
        terminal.set_viewport_area(area);

        terminal
            .draw(|frame| {
                Paragraph::new(vec![
                    Line::from(vec!["  ".into(), auth_url.as_str().cyan().underlined()]),
                    "".into(),
                    "  Press Esc to cancel".into(),
                ])
                .wrap(Wrap { trim: false })
                .render(area, frame.buffer_mut());
                crate::terminal_hyperlinks::mark_url_hyperlink(frame.buffer_mut(), area, &auth_url);
            })
            .expect("draw");

        let output = terminal.backend().output();
        let open = format!("\x1b]8;;{auth_url}\x07");
        let close = "\x1b]8;;\x07";
        assert_eq!(output.matches(&open).count(), 1);
        assert_eq!(output.matches(close).count(), 1);
        let footer = output.find("Press").expect("footer");
        assert!(output.find(close).expect("hyperlink close") < footer);
    }

    #[test]
    fn terminal_draw_applies_requested_cursor_style() {
        let mut output = Vec::new();
        let mut terminal =
            Terminal::with_options(CaptureBackend::new(/*width*/ 2, /*height*/ 1))
                .expect("terminal");
        terminal.set_viewport_area(Rect::new(0, 0, 2, 1));

        terminal
            .try_draw(|frame| {
                frame.set_cursor_style(SetCursorStyle::SteadyBar);
                frame.set_cursor_position((0, 0));
                io::Result::Ok(())
            })
            .expect("draw");

        queue!(output, SetCursorStyle::SteadyBar).expect("queue style");
        let expected = String::from_utf8(output).expect("utf8");
        let actual = terminal.backend().output();
        assert!(
            actual.contains(&expected),
            "expected terminal output to contain cursor style {expected:?}, got {actual:?}"
        );
    }

    #[test]
    fn reset_cursor_style_emits_default_user_shape() {
        let mut output = Vec::new();
        let mut terminal =
            Terminal::with_options(CaptureBackend::new(/*width*/ 2, /*height*/ 1))
                .expect("terminal");

        terminal.reset_cursor_style().expect("reset cursor style");
        ratatui::backend::Backend::flush(terminal.backend_mut()).expect("flush backend");

        queue!(output, SetCursorStyle::DefaultUserShape).expect("queue style");
        let expected = String::from_utf8(output).expect("utf8");
        let actual = terminal.backend().output();
        assert!(
            actual.contains(&expected),
            "expected terminal output to contain cursor style reset {expected:?}, got {actual:?}"
        );
    }

    /// Terminal launched with `prior_rows` of shell output already on screen.
    fn launched_at(
        width: u16,
        height: u16,
        prior_rows: u16,
    ) -> Terminal<crate::test_backend::VT100Backend> {
        Terminal::with_screen_size_and_cursor_position_for_test(
            crate::test_backend::VT100Backend::new(width, height),
            Size::new(width, height),
            Position::new(/*x*/ 0, prior_rows),
        )
    }

    #[test]
    fn claim_screen_blanks_the_screen_and_leaves_a_one_row_top_margin() {
        let (width, height) = (20u16, 10u16);
        let mut term = launched_at(width, height, /*prior_rows*/ 3);
        write!(term.backend_mut(), "one\r\ntwo\r\nthree").expect("seed prior output");

        term.claim_screen().expect("claim screen");

        // Row 1, not row 0: some terminals pin the shell prompt to the top of the
        // viewport, and anything drawn on row 0 renders underneath it.
        assert_eq!(term.viewport_area.y, 1, "expected a one-row top margin");

        // The prior rows scrolled away, so nothing is left on the visible screen.
        let screen = term.backend().vt100().screen();
        for row in 0..height {
            let text: String = (0..width)
                .filter_map(|col| screen.cell(row, col))
                .map(|cell| cell.contents())
                .collect();
            assert!(
                text.trim().is_empty(),
                "row {row} should be blank after claiming the screen, got {text:?}"
            );
        }
    }

    /// Regression: `claim_screen` blanks the screen, so history must be reported as
    /// reaching no row yet. Leaving the end row at the launch cursor made the bottom
    /// anchor fire on the first draw -- dragging the not-yet-committed session header
    /// to the bottom -- and made the gap span rows that had just been blanked. It only
    /// showed up when launching from a busy terminal, so the header's position
    /// appeared to change at random.
    #[test]
    fn claim_screen_reports_no_history_regardless_of_launch_row() {
        let (width, height) = (20u16, 39u16);

        for prior_rows in [0u16, 1, 20, 38] {
            let mut term = launched_at(width, height, prior_rows);

            term.claim_screen().expect("claim screen");

            assert_eq!(
                term.history_end_row(),
                1,
                "launching at row {prior_rows} must still leave history at the margin"
            );
            // Equal to the origin, so the anchor waits for real history instead of
            // firing on the first draw.
            assert_eq!(
                term.bottom_aligned_y(/*height*/ 10, height),
                None,
                "launching at row {prior_rows} must not anchor before history exists"
            );
        }
    }

    /// Regression: a growing composer moves the viewport top upward, and the rows it
    /// takes may hold history. Callers must scroll that overlap away before moving --
    /// the `area.bottom() > size.height` branch cannot catch it, because
    /// `bottom_aligned_y` makes bottom equal the screen height exactly so that branch
    /// never fires. Missing the scroll painted over the user's prompt.
    #[test]
    fn growing_composer_reports_the_overlap_it_takes_from_history() {
        let (width, height) = (20u16, 39u16);
        let mut term = launched_at(width, height, /*prior_rows*/ 1);

        // Composer 5 rows tall at the bottom, history filling right up to it.
        term.set_viewport_area(Rect::new(/*x*/ 0, 34, width, 5));
        term.note_history_rows_inserted(33);
        assert_eq!(term.history_end_row(), 34, "history reaches the composer");

        // It grows to 7 rows, so the top moves 34 -> 32 and claims two rows of
        // history. The caller must see a two-row overlap to scroll.
        let bottom_y = term
            .bottom_aligned_y(/*height*/ 7, height)
            .expect("anchored viewport reports a bottom row");
        assert_eq!(bottom_y, 32);
        assert_eq!(
            term.history_end_row().saturating_sub(bottom_y),
            2,
            "two rows of history are in the growing composer's way"
        );

        // Once scrolled and recorded, history ends exactly where the composer starts.
        term.note_history_scrolled(2);
        term.set_viewport_area(Rect::new(/*x*/ 0, bottom_y, width, 7));
        assert_eq!(term.history_end_row(), bottom_y);
    }

    /// Regression: a count of history rows cannot survive the viewport growing
    /// upward. `set_viewport_area` clamps the occupancy to the new viewport top, so a
    /// count silently forgot that rows below the top still held history -- the
    /// viewport then painted over them and the user's prompt and command output
    /// disappeared instead of scrolling into scrollback.
    #[test]
    fn history_occupancy_survives_the_viewport_growing_upward() {
        let (width, height) = (20u16, 40u16);
        let mut term = launched_at(width, height, /*prior_rows*/ 1);
        term.set_viewport_area(Rect::new(/*x*/ 0, 33, width, 7));
        term.note_history_rows_inserted(20);
        assert_eq!(term.history_end_row(), 21, "history occupies rows 1..21");

        // The composer grows to 15 rows, so the viewport top moves to 25 -- still
        // below the last history row, so nothing is claimed and nothing is forgotten.
        term.set_viewport_area(Rect::new(/*x*/ 0, 25, width, 15));

        assert_eq!(
            term.history_end_row(),
            21,
            "growing into blank rows must not move the history end row"
        );
        assert_eq!(
            term.next_history_row(),
            Some(21),
            "the gap between history and the composer is still fillable"
        );
    }

    /// Regression: the rows a growing viewport takes from history have to be scrolled
    /// away, and that scroll has to be recorded -- otherwise the tracked end row stays
    /// stale and later writes land on top of live content.
    #[test]
    fn scrolling_history_away_moves_the_end_row_up() {
        let (width, height) = (20u16, 40u16);
        let mut term = launched_at(width, height, /*prior_rows*/ 1);
        term.set_viewport_area(Rect::new(/*x*/ 0, 33, width, 7));
        term.note_history_rows_inserted(30);
        assert_eq!(term.history_end_row(), 31);

        // A viewport wanting row 25 downward overlaps history by 6 rows. The caller
        // scrolls those away and records it, which is what keeps the model honest.
        let bottom_y = 25;
        let overlap = term.history_end_row().saturating_sub(bottom_y);
        assert_eq!(overlap, 6, "six rows of history are in the viewport's way");
        term.note_history_scrolled(overlap);
        term.set_viewport_area(Rect::new(/*x*/ 0, bottom_y, width, 15));

        assert_eq!(
            term.history_end_row(),
            bottom_y,
            "history now ends exactly where the composer starts"
        );
        assert_eq!(
            term.next_history_row(),
            None,
            "no gap left, so callers must scroll rather than write in place"
        );
    }

    #[test]
    fn bottom_anchor_waits_for_history_then_fires_once() {
        let (width, height) = (20u16, 40u16);
        let mut term = launched_at(width, height, /*prior_rows*/ 1);
        term.set_viewport_area(Rect::new(/*x*/ 0, 1, width, 7));

        // Before any history reaches scrollback the session header is still inside the
        // viewport, so anchoring would drag it to the bottom along with the composer.
        assert_eq!(term.bottom_aligned_y(/*height*/ 7, height), None);

        // Inserting the header pushes the viewport down past the rows it wrote, which
        // is what makes room for history above it.
        term.set_viewport_area(Rect::new(/*x*/ 0, 7, width, 7));
        term.note_history_rows_inserted(6);

        assert_eq!(term.bottom_aligned_y(/*height*/ 7, height), Some(33));
    }

    #[test]
    fn viewport_stays_bottom_aligned_when_the_composer_shrinks() {
        let (width, height) = (20u16, 40u16);
        let mut term = launched_at(width, height, /*prior_rows*/ 1);
        term.set_viewport_area(Rect::new(/*x*/ 0, 7, width, 7));
        term.note_history_rows_inserted(6);
        assert_eq!(term.bottom_aligned_y(/*height*/ 7, height), Some(33));

        // A shrinking composer must stay flush with the bottom. Floating upward leaves
        // dead rows beneath it and makes `insert_history` stop filling the gap, because
        // it recognizes bottom-anchoring by comparing `area.bottom()` to the screen.
        assert_eq!(term.bottom_aligned_y(/*height*/ 5, height), Some(35));
        assert_eq!(term.bottom_aligned_y(/*height*/ 12, height), Some(28));
    }
}
