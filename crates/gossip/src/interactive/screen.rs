//! The one writer of the interactive mode: the lines of the stream, and the
//! status line beneath them.
//!
//! The stream scrolls as it always did; there are no panes, no alternate
//! screen and no cursor positioning, so the scrollback of the terminal stays
//! what it is. The status line is the last line of the output while the
//! command line is open or the output is paused. It is written with a
//! carriage return and "clear to the end of the line", erased the same way
//! before the next line of the stream, and written again after it. Because
//! everything goes through this one writer, a line and a status never
//! interleave.

use std::io::{self, Write};

use crate::columns::{ELLIPSIS, cells, end_of, start_of};

/// Back to the first cell of the line, and clear it to its end.
const ERASE_LINE: &str = "\r\x1b[K";

/// Clear the whole screen, and put the cursor in its first cell. The
/// scrollback is left alone.
const CLEAR_SCREEN: &str = "\x1b[2J\x1b[H";

/// The terminal is in raw mode, where a line feed alone does not go back to
/// the first cell.
const LINE_END: &str = "\r\n";

/// What the status line shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Status {
    pub text: String,
    /// The part of the text in front of the cursor, when the cursor belongs
    /// in the text: on the command line. Otherwise the cursor stands at the
    /// end.
    pub before_cursor: Option<String>,
}

impl Status {
    /// A status with the cursor at its end.
    pub fn plain(text: impl Into<String>) -> Status {
        Status {
            text: text.into(),
            before_cursor: None,
        }
    }
}

/// Writes the lines of the stream and the status line to a terminal.
pub struct Screen<W: Write> {
    out: W,
    /// How many cells a line of the terminal has, asked each time the
    /// status is written, since a terminal can be resized.
    width: Box<dyn Fn() -> usize + Send>,
    /// The status to show; none while the lines are flowing.
    status: Option<Status>,
    /// Whether a status is on the last line of the terminal right now.
    shown: bool,
}

impl<W: Write> Screen<W> {
    pub fn new(out: W, width: impl Fn() -> usize + Send + 'static) -> Screen<W> {
        Screen {
            out,
            width: Box::new(width),
            status: None,
            shown: false,
        }
    }

    /// Prints one line of the stream above the status line.
    #[cfg(test)]
    pub fn line(&mut self, text: &str) -> io::Result<()> {
        self.lines([text])
    }

    /// Prints lines of the stream above the status line, in one go: the
    /// status is erased once and written once. A text of several lines, the
    /// message of a panic, is printed as several.
    pub fn lines<'a>(&mut self, texts: impl IntoIterator<Item = &'a str>) -> io::Result<()> {
        self.erase()?;
        for text in texts {
            for (index, part) in text.split('\n').enumerate() {
                if index > 0 {
                    self.out.write_all(LINE_END.as_bytes())?;
                }
                self.out.write_all(part.as_bytes())?;
            }
            self.out.write_all(LINE_END.as_bytes())?;
        }
        self.draw()?;
        self.out.flush()
    }

    /// Shows this status from here on, or none.
    pub fn set_status(&mut self, status: Option<Status>) -> io::Result<()> {
        if self.status == status && self.shown == self.status.is_some() {
            return Ok(());
        }
        self.status = status;
        if self.status.is_some() {
            // Writing a status erases the one that is there.
            self.draw()?;
        } else {
            self.erase()?;
        }
        self.out.flush()
    }

    /// Clears the screen; the next line arrives at its top.
    pub fn clear(&mut self) -> io::Result<()> {
        self.out.write_all(CLEAR_SCREEN.as_bytes())?;
        self.shown = false;
        self.draw()?;
        self.out.flush()
    }

    /// Takes the status line away for good, as when the program ends, so
    /// that whatever is printed next starts on an empty line.
    pub fn leave(&mut self) -> io::Result<()> {
        self.erase()?;
        self.status = None;
        self.out.flush()
    }

    fn erase(&mut self) -> io::Result<()> {
        if self.shown {
            self.out.write_all(ERASE_LINE.as_bytes())?;
            self.shown = false;
        }
        Ok(())
    }

    fn draw(&mut self) -> io::Result<()> {
        let Some(status) = &self.status else {
            return Ok(());
        };
        // The last cell is left free: a terminal that writes into it moves
        // on to the next line, and the status would take two.
        let room = (self.width)().saturating_sub(1).max(1);
        let (text, before_cursor) = fitted(status, room);
        self.out.write_all(ERASE_LINE.as_bytes())?;
        self.out.write_all(text.as_bytes())?;
        if let Some(before_cursor) = before_cursor {
            // Writing the start of the line again leaves the cursor behind
            // it, without a cursor movement of any kind.
            self.out.write_all(b"\r")?;
            self.out.write_all(before_cursor.as_bytes())?;
        }
        self.shown = true;
        Ok(())
    }
}

/// A status as it fits on one line of this many cells, and the part of it
/// in front of the cursor when the cursor is not at its end.
///
/// A text that is too long is cut at its end. On the command line the part
/// around the cursor is what is shown: when the cursor is beyond the room,
/// the start of the line gives way.
fn fitted(status: &Status, room: usize) -> (String, Option<String>) {
    let Some(before) = &status.before_cursor else {
        if cells(&status.text) <= room {
            return (status.text.clone(), None);
        }
        let kept = start_of(&status.text, room.saturating_sub(1));
        return (format!("{kept}{ELLIPSIS}"), None);
    };
    let after = &status.text[before.len()..];
    // The cursor needs a cell of its own behind the text in front of it.
    let before = if cells(before) < room {
        before.clone()
    } else {
        let kept = end_of(before, room.saturating_sub(2));
        format!("{ELLIPSIS}{kept}")
    };
    let left = room - cells(&before).min(room);
    let after = if cells(after) <= left {
        after.to_string()
    } else {
        let kept = start_of(after, left.saturating_sub(1));
        format!("{kept}{ELLIPSIS}")
    };
    let cursor = (!after.is_empty()).then(|| before.clone());
    (format!("{before}{after}"), cursor)
}

#[cfg(test)]
pub mod testing {
    //! A terminal in memory: what the bytes written to it leave on its
    //! screen.

    use std::io::{self, Write};
    use std::sync::{Arc, Mutex};

    use crate::columns::cells;

    /// Something to write to, whose bytes can be read back.
    #[derive(Debug, Clone, Default)]
    pub struct Tape(Arc<Mutex<Vec<u8>>>);

    impl Tape {
        /// Everything that was written, as text.
        pub fn text(&self) -> String {
            String::from_utf8(self.0.lock().unwrap().clone()).expect("the output is UTF-8")
        }

        /// What a terminal shows after everything that was written.
        pub fn terminal(&self) -> Terminal {
            Terminal::after(&self.text())
        }

        /// The lines of the terminal, see [`Terminal::lines`].
        pub fn lines(&self) -> Vec<String> {
            self.terminal().lines()
        }
    }

    impl Write for Tape {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    /// The screen of a terminal in raw mode, as wide as it needs to be: it
    /// knows a carriage return, a line feed, "clear to the end of the line",
    /// "clear the screen" and "cursor home", and ignores colours.
    #[derive(Debug, Default)]
    pub struct Terminal {
        /// The cells of every row. A character that is two cells wide is in
        /// the first of them, and the second is empty.
        rows: Vec<Vec<String>>,
        row: usize,
        column: usize,
    }

    impl Terminal {
        pub fn after(text: &str) -> Terminal {
            let mut terminal = Terminal::default();
            let mut characters = text.chars();
            while let Some(character) = characters.next() {
                match character {
                    '\r' => terminal.column = 0,
                    // In raw mode a line feed goes down and not back.
                    '\n' => terminal.row += 1,
                    '\u{1b}' => {
                        assert_eq!(characters.next(), Some('['), "an escape code of its kind");
                        let mut code = String::new();
                        for next in characters.by_ref() {
                            code.push(next);
                            if next.is_ascii_alphabetic() {
                                break;
                            }
                        }
                        terminal.escape(&code);
                    }
                    character => terminal.put(character),
                }
            }
            terminal
        }

        fn escape(&mut self, code: &str) {
            match code {
                "K" => {
                    let column = self.column;
                    if let Some(row) = self.rows.get_mut(self.row) {
                        row.truncate(column);
                    }
                }
                "2J" => self.rows.clear(),
                "H" => (self.row, self.column) = (0, 0),
                colour if colour.ends_with('m') => {}
                other => panic!("the terminal of the tests does not know the code `{other}`"),
            }
        }

        fn put(&mut self, character: char) {
            while self.rows.len() <= self.row {
                self.rows.push(Vec::new());
            }
            let row = &mut self.rows[self.row];
            while row.len() < self.column {
                row.push(" ".to_string());
            }
            let width = cells(&character.to_string());
            if width == 0 {
                // It joins the character in front of it, which may grow by
                // a cell for it: the selector that makes a sign an emoji.
                let Some(first) = (0..self.column).rev().find(|cell| !row[*cell].is_empty()) else {
                    return;
                };
                let before = cells(&row[first]);
                row[first].push(character);
                for _ in before..cells(&row[first]) {
                    self.set(String::new());
                }
                return;
            }
            self.set(character.to_string());
            for _ in 1..width {
                self.set(String::new());
            }
        }

        /// Puts something into the cell of the cursor and moves on.
        fn set(&mut self, content: String) {
            let row = &mut self.rows[self.row];
            if self.column < row.len() {
                row[self.column] = content;
            } else {
                row.push(content);
            }
            self.column += 1;
        }

        /// The lines on the screen, from the top down to the line of the
        /// cursor, without the spaces at their ends.
        pub fn lines(&self) -> Vec<String> {
            (0..=self.row.max(self.rows.len().saturating_sub(1)))
                .map(|row| {
                    let cells = self.rows.get(row).map(Vec::as_slice).unwrap_or_default();
                    cells.concat().trim_end().to_string()
                })
                .collect()
        }

        /// The line the cursor is on.
        pub fn last_line(&self) -> String {
            self.lines().get(self.row).cloned().unwrap_or_default()
        }

        /// The cell the cursor is in, counted from the start of its line.
        pub fn cursor_column(&self) -> usize {
            self.column
        }

        /// The row the cursor is in, counted from the top.
        pub fn cursor_row(&self) -> usize {
            self.row
        }
    }
}

#[cfg(test)]
mod tests {
    use super::testing::Tape;
    use super::*;

    fn screen(width: usize) -> (Screen<Tape>, Tape) {
        let tape = Tape::default();
        (Screen::new(tape.clone(), move || width), tape)
    }

    fn prompt(text: &str, before: &str) -> Status {
        Status {
            text: text.to_string(),
            before_cursor: Some(before.to_string()),
        }
    }

    #[test]
    fn lines_are_printed_one_under_the_other_from_the_first_cell() {
        let (mut screen, tape) = screen(80);
        screen.line("one").unwrap();
        screen.line("two").unwrap();
        // In raw mode that takes a carriage return with every line feed.
        assert_eq!(tape.text(), "one\r\ntwo\r\n");
        assert_eq!(tape.lines(), ["one", "two", ""]);
        assert_eq!(tape.terminal().cursor_column(), 0);
    }

    #[test]
    fn a_text_of_several_lines_does_not_stagger() {
        let (mut screen, tape) = screen(80);
        screen
            .line("thread 'main' panicked\n   0: main\n   1: start")
            .unwrap();
        screen.line("next").unwrap();
        assert_eq!(
            tape.lines(),
            [
                "thread 'main' panicked",
                "   0: main",
                "   1: start",
                "next",
                ""
            ]
        );
    }

    #[test]
    fn without_a_status_nothing_but_the_lines_is_written() {
        let (mut screen, tape) = screen(80);
        screen.set_status(None).unwrap();
        screen.line("one").unwrap();
        screen.leave().unwrap();
        assert_eq!(tape.text(), "one\r\n");
    }

    #[test]
    fn the_status_is_the_last_line_and_lines_arrive_above_it() {
        let (mut screen, tape) = screen(80);
        screen.line("one").unwrap();
        screen
            .set_status(Some(Status::plain("paused · 0 lines waiting")))
            .unwrap();
        assert_eq!(tape.lines(), ["one", "paused · 0 lines waiting"]);

        // A line of the stream takes the place of the status, and the status
        // is written again beneath it: they never share a line.
        screen.line("two").unwrap();
        screen.line("three").unwrap();
        assert_eq!(
            tape.lines(),
            ["one", "two", "three", "paused · 0 lines waiting"]
        );

        // Another status takes the place of the one that is there.
        screen
            .set_status(Some(Status::plain("paused · 12 lines waiting")))
            .unwrap();
        assert_eq!(
            tape.lines(),
            ["one", "two", "three", "paused · 12 lines waiting"]
        );
        // A shorter one leaves nothing of the longer one behind.
        screen.set_status(Some(Status::plain("paused"))).unwrap();
        assert_eq!(tape.terminal().last_line(), "paused");
    }

    #[test]
    fn the_status_never_stays_when_it_is_taken_away() {
        let (mut screen, tape) = screen(80);
        screen.line("one").unwrap();
        screen.set_status(Some(Status::plain("paused"))).unwrap();
        screen.set_status(None).unwrap();
        assert_eq!(tape.lines(), ["one", ""]);
        screen.line("two").unwrap();
        assert_eq!(tape.lines(), ["one", "two", ""]);
        assert_eq!(tape.terminal().cursor_column(), 0);
    }

    #[test]
    fn the_status_is_written_with_a_carriage_return_and_clear_to_end_of_line_only() {
        let (mut screen, tape) = screen(80);
        screen.set_status(Some(Status::plain("paused"))).unwrap();
        screen.line("one").unwrap();
        screen.set_status(None).unwrap();
        let written = tape.text();
        let codes: Vec<&str> = written.matches("\u{1b}[").collect();
        assert_eq!(codes.len(), written.matches("\u{1b}[K").count());
        assert_eq!(
            written,
            "\r\u{1b}[Kpaused\r\u{1b}[Kone\r\n\r\u{1b}[Kpaused\r\u{1b}[K"
        );
    }

    #[test]
    fn another_status_takes_one_erase_not_two() {
        let (mut screen, tape) = screen(80);
        screen.set_status(Some(Status::plain(":l"))).unwrap();
        screen.set_status(Some(Status::plain(":le"))).unwrap();
        screen.set_status(None).unwrap();
        assert_eq!(tape.text(), "\r\u{1b}[K:l\r\u{1b}[K:le\r\u{1b}[K");
    }

    #[test]
    fn the_same_status_again_writes_nothing() {
        let (mut screen, tape) = screen(80);
        screen.set_status(Some(Status::plain("paused"))).unwrap();
        let written = tape.text().len();
        screen.set_status(Some(Status::plain("paused"))).unwrap();
        assert_eq!(tape.text().len(), written);
    }

    #[test]
    fn many_lines_in_one_go_erase_and_write_the_status_once() {
        let (mut screen, tape) = screen(80);
        screen.set_status(Some(Status::plain("paused"))).unwrap();
        let before = tape.text().matches("paused").count();
        screen.lines(["one", "two", "three"]).unwrap();
        assert_eq!(tape.lines(), ["one", "two", "three", "paused"]);
        assert_eq!(tape.text().matches("paused").count(), before + 1);
    }

    #[test]
    fn the_cursor_of_the_command_line_stands_where_the_next_key_goes() {
        let (mut screen, tape) = screen(80);
        screen.line("one").unwrap();
        screen
            .set_status(Some(prompt(":level warn", ":level warn")))
            .unwrap();
        assert_eq!(tape.terminal().last_line(), ":level warn");
        assert_eq!(tape.terminal().cursor_column(), 11);

        // In the middle of the line, after `Left`.
        screen
            .set_status(Some(prompt(":level warn", ":level")))
            .unwrap();
        assert_eq!(tape.terminal().last_line(), ":level warn");
        assert_eq!(tape.terminal().cursor_column(), 6);
        // At its start, after `Home`: behind the colon.
        screen.set_status(Some(prompt(":level warn", ":"))).unwrap();
        assert_eq!(tape.terminal().cursor_column(), 1);

        // A line of the stream arrives above, and the cursor is back where
        // it was.
        screen.line("two").unwrap();
        assert_eq!(tape.lines(), ["one", "two", ":level warn"]);
        assert_eq!(tape.terminal().cursor_column(), 1);
    }

    #[test]
    fn the_cursor_counts_cells_not_characters() {
        let (mut screen, tape) = screen(80);
        screen
            .set_status(Some(prompt(":grep 🚀 zürich", ":grep 🚀 ")))
            .unwrap();
        assert_eq!(tape.terminal().last_line(), ":grep 🚀 zürich");
        assert_eq!(tape.terminal().cursor_column(), 9);
    }

    #[test]
    fn a_status_longer_than_the_terminal_is_cut_to_one_line() {
        let (mut screen, tape) = screen(20);
        screen
            .set_status(Some(Status::plain("paused · 1,234,567 lines waiting")))
            .unwrap();
        // The last cell stays free.
        assert_eq!(tape.terminal().last_line(), "paused · 1,234,567…");
        assert_eq!(cells(&tape.terminal().last_line()), 19);
    }

    #[test]
    fn a_long_command_line_shows_the_part_around_the_cursor() {
        let (mut screen, tape) = screen(20);
        let text = ":grep a text that is longer than the line";
        // The cursor at the end: the start gives way.
        screen.set_status(Some(prompt(text, text))).unwrap();
        assert_eq!(tape.terminal().last_line(), "…ger than the line");
        assert_eq!(tape.terminal().cursor_column(), 18);
        assert!(tape.terminal().cursor_column() < 20);

        // The cursor near the start: the end gives way.
        screen.set_status(Some(prompt(text, ":grep"))).unwrap();
        assert_eq!(tape.terminal().last_line(), ":grep a text that …");
        assert_eq!(tape.terminal().cursor_column(), 5);

        // Whatever the cursor does, the line is one line.
        for cut in 0..=text.len() {
            screen.set_status(Some(prompt(text, &text[..cut]))).unwrap();
            let terminal = tape.terminal();
            assert!(cells(&terminal.last_line()) <= 19, "{cut}");
            assert!(terminal.cursor_column() <= 19, "{cut}");
            assert_eq!(terminal.cursor_row(), 0, "{cut}");
        }
    }

    #[test]
    fn a_terminal_of_no_width_at_all_still_gets_one_cell() {
        let (mut screen, tape) = screen(0);
        screen.set_status(Some(Status::plain("paused"))).unwrap();
        assert_eq!(tape.terminal().last_line(), "…");
        screen.set_status(Some(prompt(":level", ":level"))).unwrap();
        assert!(cells(&tape.terminal().last_line()) <= 1);
    }

    #[test]
    fn clearing_puts_the_next_line_at_the_top() {
        let (mut screen, tape) = screen(80);
        screen.line("one").unwrap();
        screen.line("two").unwrap();
        screen.clear().unwrap();
        assert_eq!(tape.lines(), [""]);
        // Acceptance 5.
        screen.line("three").unwrap();
        assert_eq!(tape.lines(), ["three", ""]);
        assert!(tape.text().contains("\u{1b}[2J\u{1b}[H"));
    }

    #[test]
    fn clearing_keeps_the_status() {
        let (mut screen, tape) = screen(80);
        screen.line("one").unwrap();
        screen.set_status(Some(Status::plain("paused"))).unwrap();
        screen.clear().unwrap();
        assert_eq!(tape.lines(), ["paused"]);
        screen.line("two").unwrap();
        assert_eq!(tape.lines(), ["two", "paused"]);
    }

    #[test]
    fn leaving_takes_the_status_away_and_the_cursor_to_an_empty_line() {
        let (mut screen, tape) = screen(80);
        screen.line("one").unwrap();
        screen
            .set_status(Some(prompt(":level warn", ":level")))
            .unwrap();
        screen.leave().unwrap();
        assert_eq!(tape.lines(), ["one", ""]);
        assert_eq!(tape.terminal().cursor_column(), 0);
        // Leaving twice is leaving once.
        let written = tape.text().len();
        screen.leave().unwrap();
        assert_eq!(tape.text().len(), written);
    }

    #[test]
    fn coloured_lines_pass_through_untouched() {
        let (mut screen, tape) = screen(80);
        screen.line("\u{1b}[2m14:02:11.482\u{1b}[0m ready").unwrap();
        assert!(
            tape.text()
                .starts_with("\u{1b}[2m14:02:11.482\u{1b}[0m ready\r\n")
        );
        assert_eq!(tape.lines(), ["14:02:11.482 ready", ""]);
    }
}
