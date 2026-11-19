//! The line of the command line while it is typed, and the lines typed
//! before it.

/// A line being edited: its text and where the cursor is in it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Line {
    chars: Vec<char>,
    /// How many characters are in front of the cursor.
    cursor: usize,
}

impl Line {
    /// A line with this text, the cursor at its end.
    pub fn new(text: &str) -> Line {
        let chars: Vec<char> = text.chars().collect();
        Line {
            cursor: chars.len(),
            chars,
        }
    }

    pub fn text(&self) -> String {
        self.chars.iter().collect()
    }

    /// The text in front of the cursor.
    pub fn before_cursor(&self) -> String {
        self.chars[..self.cursor].iter().collect()
    }

    pub fn is_empty(&self) -> bool {
        self.chars.is_empty()
    }

    pub fn insert(&mut self, character: char) {
        self.chars.insert(self.cursor, character);
        self.cursor += 1;
    }

    /// Removes the character in front of the cursor.
    pub fn backspace(&mut self) {
        if self.cursor > 0 {
            self.cursor -= 1;
            self.chars.remove(self.cursor);
        }
    }

    /// Removes the character under the cursor.
    pub fn delete(&mut self) {
        if self.cursor < self.chars.len() {
            self.chars.remove(self.cursor);
        }
    }

    pub fn left(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
    }

    pub fn right(&mut self) {
        self.cursor = (self.cursor + 1).min(self.chars.len());
    }

    pub fn home(&mut self) {
        self.cursor = 0;
    }

    pub fn end(&mut self) {
        self.cursor = self.chars.len();
    }

    /// Removes everything in front of the cursor (`Ctrl-U`).
    pub fn kill_to_start(&mut self) {
        self.chars.drain(..self.cursor);
        self.cursor = 0;
    }

    /// Removes the word in front of the cursor, with the spaces between it
    /// and the cursor (`Ctrl-W`).
    pub fn kill_word(&mut self) {
        let mut start = self.cursor;
        while start > 0 && self.chars[start - 1].is_whitespace() {
            start -= 1;
        }
        while start > 0 && !self.chars[start - 1].is_whitespace() {
            start -= 1;
        }
        self.chars.drain(start..self.cursor);
        self.cursor = start;
    }
}

/// The lines that were entered in this session, for `Up` and `Down`.
#[derive(Debug, Clone, Default)]
pub struct History {
    entries: Vec<String>,
    /// Which entry is shown while going through them; none on the line
    /// that is being typed.
    at: Option<usize>,
    /// The line that was being typed when `Up` was first pressed.
    draft: String,
}

impl History {
    /// Remembers a line that was entered. An empty line and the line that
    /// was entered last are not remembered again.
    pub fn push(&mut self, line: &str) {
        self.at = None;
        self.draft.clear();
        if line.trim().is_empty() || self.entries.last().is_some_and(|last| last == line) {
            return;
        }
        self.entries.push(line.to_string());
    }

    /// The line before the one shown, if there is one. `current` is what is
    /// on the line now, kept for the way back down.
    pub fn up(&mut self, current: &str) -> Option<&str> {
        let previous = match self.at {
            None if self.entries.is_empty() => return None,
            None => {
                self.draft = current.to_string();
                self.entries.len() - 1
            }
            Some(0) => return None,
            Some(at) => at - 1,
        };
        self.at = Some(previous);
        Some(&self.entries[previous])
    }

    /// The line after the one shown; after the last one, the line that was
    /// being typed.
    pub fn down(&mut self) -> Option<&str> {
        let at = self.at?;
        if at + 1 < self.entries.len() {
            self.at = Some(at + 1);
            Some(&self.entries[at + 1])
        } else {
            self.at = None;
            Some(&self.draft)
        }
    }

    /// Forgets where in the history the line was, when the command line is
    /// closed without entering it.
    pub fn leave(&mut self) {
        self.at = None;
        self.draft.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A line as text with `|` where the cursor is.
    fn shown(line: &Line) -> String {
        format!(
            "{}|{}",
            line.before_cursor(),
            &line.text()[line.before_cursor().len()..]
        )
    }

    #[test]
    fn typing_puts_characters_at_the_cursor() {
        let mut line = Line::default();
        assert!(line.is_empty());
        for character in "level warn".chars() {
            line.insert(character);
        }
        assert_eq!(shown(&line), "level warn|");
        assert_eq!(line.text(), "level warn");

        line.home();
        line.insert(':');
        assert_eq!(shown(&line), ":|level warn");
        line.end();
        line.insert('!');
        assert_eq!(shown(&line), ":level warn!|");
    }

    #[test]
    fn a_line_starts_with_the_cursor_at_its_end() {
        assert_eq!(shown(&Line::new("grep ")), "grep |");
        assert_eq!(shown(&Line::new("")), "|");
    }

    #[test]
    fn left_right_home_and_end_move_the_cursor_and_stop_at_the_ends() {
        let mut line = Line::new("abc");
        line.left();
        assert_eq!(shown(&line), "ab|c");
        line.left();
        line.left();
        line.left();
        assert_eq!(shown(&line), "|abc");
        line.right();
        assert_eq!(shown(&line), "a|bc");
        line.end();
        line.right();
        assert_eq!(shown(&line), "abc|");
        line.home();
        assert_eq!(shown(&line), "|abc");
    }

    #[test]
    fn backspace_and_delete_remove_one_character() {
        let mut line = Line::new("abcd");
        line.backspace();
        assert_eq!(shown(&line), "abc|");
        line.left();
        line.backspace();
        assert_eq!(shown(&line), "a|c");
        line.delete();
        assert_eq!(shown(&line), "a|");
        // At the ends there is nothing to remove.
        line.delete();
        line.home();
        line.backspace();
        assert_eq!(shown(&line), "|a");
    }

    #[test]
    fn ctrl_u_removes_everything_in_front_of_the_cursor() {
        let mut line = Line::new("level warn");
        line.left();
        line.left();
        line.kill_to_start();
        assert_eq!(shown(&line), "|rn");
        line.kill_to_start();
        assert_eq!(shown(&line), "|rn");
    }

    #[test]
    fn ctrl_w_removes_the_word_in_front_of_the_cursor() {
        let mut line = Line::new("add fc-stg/fc-api  fc-stg/fc-app");
        line.kill_word();
        assert_eq!(shown(&line), "add fc-stg/fc-api  |");
        // The spaces between the cursor and the word go with it.
        line.kill_word();
        assert_eq!(shown(&line), "add |");
        line.kill_word();
        assert_eq!(shown(&line), "|");
        line.kill_word();
        assert_eq!(shown(&line), "|");

        // In the middle of a line, what is behind the cursor stays.
        let mut line = Line::new("grep payment intent");
        for _ in 0..7 {
            line.left();
        }
        line.kill_word();
        assert_eq!(shown(&line), "grep | intent");
    }

    #[test]
    fn a_line_holds_any_character() {
        let mut line = Line::new("grep zürich 🚀");
        line.backspace();
        line.backspace();
        line.left();
        line.left();
        line.left();
        line.left();
        line.insert('Z');
        assert_eq!(shown(&line), "grep züZ|rich");
    }

    #[test]
    fn up_and_down_go_through_the_lines_entered_before() {
        let mut history = History::default();
        // Nothing was entered yet.
        assert_eq!(history.up("lev"), None);
        assert_eq!(history.down(), None);

        history.push("level warn");
        history.push("grep payment");
        history.push("field tenant_id=t_123");

        assert_eq!(history.up("ad"), Some("field tenant_id=t_123"));
        assert_eq!(history.up("ignored"), Some("grep payment"));
        assert_eq!(history.up("ignored"), Some("level warn"));
        // The oldest is where it stops.
        assert_eq!(history.up("ignored"), None);
        assert_eq!(history.down(), Some("grep payment"));
        assert_eq!(history.down(), Some("field tenant_id=t_123"));
        // Past the newest is the line that was being typed.
        assert_eq!(history.down(), Some("ad"));
        assert_eq!(history.down(), None);
    }

    #[test]
    fn the_history_skips_empty_lines_and_the_same_line_twice_in_a_row() {
        let mut history = History::default();
        history.push("level warn");
        history.push("");
        history.push("   ");
        history.push("level warn");
        history.push("grep x");
        history.push("level warn");
        assert_eq!(history.up(""), Some("level warn"));
        assert_eq!(history.up(""), Some("grep x"));
        assert_eq!(history.up(""), Some("level warn"));
        assert_eq!(history.up(""), None);
    }

    #[test]
    fn entering_or_leaving_starts_the_history_from_its_end_again() {
        let mut history = History::default();
        history.push("level warn");
        history.push("grep x");
        assert_eq!(history.up("draft"), Some("grep x"));
        assert_eq!(history.up("draft"), Some("level warn"));
        history.leave();
        assert_eq!(history.down(), None);
        assert_eq!(history.up(""), Some("grep x"));
        history.push("events off");
        assert_eq!(history.up(""), Some("events off"));
    }
}
