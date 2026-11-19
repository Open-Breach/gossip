//! What a key does: in the stream, where a key is a command of its own, and
//! on the command line, where it edits the line.
//!
//! Nothing in here reads a terminal. A key comes in, and what it means goes
//! out.

use super::prompt::{History, Line};

/// A key, as far as the interactive mode tells keys apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Char(char),
    /// A letter with `Ctrl`, in lower case.
    Ctrl(char),
    Enter,
    Esc,
    Backspace,
    Delete,
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
}

/// What a key asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Pause the output, or resume it.
    Pause,
    /// Clear the screen.
    Clear,
    Quit,
    /// Print the keys and the commands.
    Help,
    /// Apply this line of the command line.
    Run(String),
    /// The command line changed, or opened, or closed: show it as it is.
    Prompt,
    /// The answer to the question on the status line.
    Answer(bool),
    /// The key means nothing here.
    Nothing,
}

/// The text the command line starts with when `/` opens it.
const SEARCH: &str = "grep ";

/// The keyboard of the interactive mode: whether the command line is open,
/// what is on it, and what was entered before.
#[derive(Debug, Default)]
pub struct Keyboard {
    line: Option<Line>,
    history: History,
    /// A question that waits for `y` or `n`.
    question: Option<String>,
}

impl Keyboard {
    /// Puts a question on the status line; the next key answers it.
    pub fn ask(&mut self, question: impl Into<String>) {
        self.line = None;
        self.question = Some(question.into());
    }

    /// The question that waits for an answer.
    pub fn question(&self) -> Option<&str> {
        self.question.as_deref()
    }

    /// Opens the command line with this text typed, the cursor behind it.
    pub fn open_with(&mut self, text: &str) {
        self.question = None;
        self.line = Some(Line::new(text));
    }

    /// The command line while it is open: its text, and the text in front
    /// of the cursor.
    pub fn prompt(&self) -> Option<(String, String)> {
        self.line
            .as_ref()
            .map(|line| (line.text(), line.before_cursor()))
    }

    /// Takes a key and says what it asks for.
    pub fn press(&mut self, key: Key) -> Action {
        if self.question.take().is_some() {
            return match key {
                Key::Char('y' | 'Y') => Action::Answer(true),
                // `Ctrl-C` quits from anywhere.
                Key::Ctrl('c') => Action::Quit,
                // Anything else is a no: nothing is done by accident.
                _ => Action::Answer(false),
            };
        }
        match &mut self.line {
            None => self.in_the_stream(key),
            Some(_) => self.on_the_command_line(key),
        }
    }

    /// A key while the stream scrolls: each is a command of its own, and
    /// any other key is ignored.
    fn in_the_stream(&mut self, key: Key) -> Action {
        match key {
            Key::Char(' ') => Action::Pause,
            Key::Ctrl('l') => Action::Clear,
            Key::Char(':') => {
                self.line = Some(Line::default());
                Action::Prompt
            }
            Key::Char('/') => {
                self.line = Some(Line::new(SEARCH));
                Action::Prompt
            }
            Key::Char('q') | Key::Ctrl('c') => Action::Quit,
            Key::Char('?') => Action::Help,
            _ => Action::Nothing,
        }
    }

    /// A key while the command line is open.
    fn on_the_command_line(&mut self, key: Key) -> Action {
        let Some(line) = &mut self.line else {
            return Action::Nothing;
        };
        match key {
            Key::Enter => {
                let entered = line.text();
                self.line = None;
                if entered.trim().is_empty() {
                    self.history.leave();
                    return Action::Prompt;
                }
                self.history.push(&entered);
                return Action::Run(entered);
            }
            // `Ctrl-C` quits from anywhere.
            Key::Ctrl('c') => return Action::Quit,
            Key::Esc => return self.close(),
            // The screen is cleared and the line stays as it is.
            Key::Ctrl('l') => return Action::Clear,
            // Erasing past the start of the line closes it, as in `less`.
            Key::Backspace | Key::Ctrl('h') if line.is_empty() => return self.close(),
            Key::Backspace | Key::Ctrl('h') => line.backspace(),
            Key::Delete | Key::Ctrl('d') => line.delete(),
            Key::Left | Key::Ctrl('b') => line.left(),
            Key::Right | Key::Ctrl('f') => line.right(),
            Key::Home | Key::Ctrl('a') => line.home(),
            Key::End | Key::Ctrl('e') => line.end(),
            Key::Ctrl('u') => line.kill_to_start(),
            Key::Ctrl('w') => line.kill_word(),
            Key::Up | Key::Ctrl('p') => match self.history.up(&line.text()) {
                Some(earlier) => *line = Line::new(earlier),
                None => return Action::Nothing,
            },
            Key::Down | Key::Ctrl('n') => match self.history.down() {
                Some(later) => *line = Line::new(later),
                None => return Action::Nothing,
            },
            Key::Char(character) => line.insert(character),
            Key::Ctrl(_) => return Action::Nothing,
        }
        Action::Prompt
    }

    fn close(&mut self) -> Action {
        self.line = None;
        self.history.leave();
        Action::Prompt
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn typed(keyboard: &mut Keyboard, text: &str) -> Action {
        let mut last = Action::Nothing;
        for character in text.chars() {
            last = keyboard.press(Key::Char(character));
        }
        last
    }

    /// The command line as text with `|` where the cursor is; none when it
    /// is closed.
    fn prompt(keyboard: &Keyboard) -> Option<String> {
        keyboard
            .prompt()
            .map(|(text, before)| format!("{before}|{}", &text[before.len()..]))
    }

    #[test]
    fn the_keys_of_the_stream() {
        let mut keyboard = Keyboard::default();
        assert_eq!(keyboard.press(Key::Char(' ')), Action::Pause);
        assert_eq!(keyboard.press(Key::Ctrl('l')), Action::Clear);
        assert_eq!(keyboard.press(Key::Char('q')), Action::Quit);
        assert_eq!(keyboard.press(Key::Ctrl('c')), Action::Quit);
        assert_eq!(keyboard.press(Key::Char('?')), Action::Help);
        assert_eq!(prompt(&keyboard), None);
    }

    #[test]
    fn any_other_key_in_the_stream_is_ignored() {
        let mut keyboard = Keyboard::default();
        for key in [
            Key::Char('a'),
            Key::Char('Q'),
            Key::Char('1'),
            Key::Enter,
            Key::Esc,
            Key::Backspace,
            Key::Delete,
            Key::Left,
            Key::Right,
            Key::Up,
            Key::Down,
            Key::Home,
            Key::End,
            Key::Ctrl('u'),
            Key::Ctrl('w'),
            Key::Ctrl('z'),
            Key::Ctrl('s'),
        ] {
            assert_eq!(keyboard.press(key), Action::Nothing, "{key:?}");
            assert_eq!(prompt(&keyboard), None, "{key:?}");
        }
    }

    #[test]
    fn a_colon_opens_the_command_line_and_enter_applies_it() {
        let mut keyboard = Keyboard::default();
        assert_eq!(keyboard.press(Key::Char(':')), Action::Prompt);
        assert_eq!(prompt(&keyboard).as_deref(), Some("|"));
        assert_eq!(typed(&mut keyboard, "level warn"), Action::Prompt);
        assert_eq!(prompt(&keyboard).as_deref(), Some("level warn|"));
        assert_eq!(
            keyboard.press(Key::Enter),
            Action::Run("level warn".to_string())
        );
        assert_eq!(prompt(&keyboard), None);
    }

    #[test]
    fn a_slash_opens_the_command_line_with_grep_typed() {
        let mut keyboard = Keyboard::default();
        assert_eq!(keyboard.press(Key::Char('/')), Action::Prompt);
        assert_eq!(prompt(&keyboard).as_deref(), Some("grep |"));
        typed(&mut keyboard, "payment");
        // So `/payment` is `:grep payment`.
        assert_eq!(
            keyboard.press(Key::Enter),
            Action::Run("grep payment".to_string())
        );
    }

    #[test]
    fn on_the_command_line_the_keys_of_the_stream_are_characters() {
        let mut keyboard = Keyboard::default();
        keyboard.press(Key::Char(':'));
        typed(&mut keyboard, "grep q ? / : x");
        assert_eq!(prompt(&keyboard).as_deref(), Some("grep q ? / : x|"));
        assert_eq!(
            keyboard.press(Key::Enter),
            Action::Run("grep q ? / : x".to_string())
        );
    }

    #[test]
    fn esc_closes_the_command_line_without_applying() {
        let mut keyboard = Keyboard::default();
        keyboard.press(Key::Char(':'));
        typed(&mut keyboard, "level warn");
        assert_eq!(keyboard.press(Key::Esc), Action::Prompt);
        assert_eq!(prompt(&keyboard), None);
        // The stream has its keys back, and the line was not remembered.
        assert_eq!(keyboard.press(Key::Char(' ')), Action::Pause);
        keyboard.press(Key::Char(':'));
        assert_eq!(keyboard.press(Key::Up), Action::Nothing);
    }

    #[test]
    fn ctrl_c_quits_while_the_command_line_is_open() {
        let mut keyboard = Keyboard::default();
        keyboard.press(Key::Char(':'));
        typed(&mut keyboard, "level");
        assert_eq!(keyboard.press(Key::Ctrl('c')), Action::Quit);
    }

    #[test]
    fn an_empty_line_is_not_a_command() {
        let mut keyboard = Keyboard::default();
        keyboard.press(Key::Char(':'));
        assert_eq!(keyboard.press(Key::Enter), Action::Prompt);
        assert_eq!(prompt(&keyboard), None);
        keyboard.press(Key::Char(':'));
        typed(&mut keyboard, "   ");
        assert_eq!(keyboard.press(Key::Enter), Action::Prompt);
        assert_eq!(prompt(&keyboard), None);
    }

    #[test]
    fn the_command_line_is_edited_with_the_usual_keys() {
        let mut keyboard = Keyboard::default();
        keyboard.press(Key::Char(':'));
        typed(&mut keyboard, "levl warn");
        for _ in 0..6 {
            keyboard.press(Key::Left);
        }
        assert_eq!(prompt(&keyboard).as_deref(), Some("lev|l warn"));
        keyboard.press(Key::Char('e'));
        assert_eq!(prompt(&keyboard).as_deref(), Some("leve|l warn"));
        keyboard.press(Key::Right);
        keyboard.press(Key::Backspace);
        keyboard.press(Key::Char('l'));
        assert_eq!(prompt(&keyboard).as_deref(), Some("level| warn"));
        keyboard.press(Key::Home);
        assert_eq!(prompt(&keyboard).as_deref(), Some("|level warn"));
        keyboard.press(Key::Delete);
        assert_eq!(prompt(&keyboard).as_deref(), Some("|evel warn"));
        keyboard.press(Key::End);
        assert_eq!(prompt(&keyboard).as_deref(), Some("evel warn|"));
        keyboard.press(Key::Ctrl('w'));
        assert_eq!(prompt(&keyboard).as_deref(), Some("evel |"));
        keyboard.press(Key::Ctrl('u'));
        assert_eq!(prompt(&keyboard).as_deref(), Some("|"));
        // Every one of them shows the line again.
        typed(&mut keyboard, "q");
        for key in [
            Key::Left,
            Key::Right,
            Key::Home,
            Key::End,
            Key::Delete,
            Key::Ctrl('u'),
            Key::Ctrl('w'),
        ] {
            assert_eq!(keyboard.press(key), Action::Prompt, "{key:?}");
        }
    }

    #[test]
    fn backspace_on_an_empty_line_closes_it() {
        let mut keyboard = Keyboard::default();
        keyboard.press(Key::Char('/'));
        for _ in 0.."grep ".len() {
            assert_eq!(keyboard.press(Key::Backspace), Action::Prompt);
            assert!(prompt(&keyboard).is_some());
        }
        assert_eq!(keyboard.press(Key::Backspace), Action::Prompt);
        assert_eq!(prompt(&keyboard), None);
    }

    #[test]
    fn up_and_down_bring_back_what_was_entered() {
        let mut keyboard = Keyboard::default();
        for line in ["level warn", "grep payment"] {
            keyboard.press(Key::Char(':'));
            typed(&mut keyboard, line);
            keyboard.press(Key::Enter);
        }
        keyboard.press(Key::Char(':'));
        typed(&mut keyboard, "fie");
        assert_eq!(keyboard.press(Key::Up), Action::Prompt);
        assert_eq!(prompt(&keyboard).as_deref(), Some("grep payment|"));
        keyboard.press(Key::Up);
        assert_eq!(prompt(&keyboard).as_deref(), Some("level warn|"));
        assert_eq!(keyboard.press(Key::Up), Action::Nothing);
        keyboard.press(Key::Down);
        keyboard.press(Key::Down);
        // Back on the line that was being typed.
        assert_eq!(prompt(&keyboard).as_deref(), Some("fie|"));
        assert_eq!(keyboard.press(Key::Down), Action::Nothing);

        // A line from the history can be changed and entered as a new one.
        keyboard.press(Key::Up);
        keyboard.press(Key::Up);
        keyboard.press(Key::Ctrl('w'));
        typed(&mut keyboard, "error");
        assert_eq!(
            keyboard.press(Key::Enter),
            Action::Run("level error".to_string())
        );
    }

    #[test]
    fn ctrl_l_clears_also_while_the_command_line_is_open() {
        let mut keyboard = Keyboard::default();
        keyboard.press(Key::Char(':'));
        typed(&mut keyboard, "level");
        assert_eq!(keyboard.press(Key::Ctrl('l')), Action::Clear);
        assert_eq!(prompt(&keyboard).as_deref(), Some("level|"));
    }

    #[test]
    fn a_question_is_answered_by_the_next_key() {
        let mut keyboard = Keyboard::default();
        keyboard.ask("overwrite the template dev-api? y/n");
        assert_eq!(
            keyboard.question(),
            Some("overwrite the template dev-api? y/n")
        );
        assert_eq!(keyboard.press(Key::Char('y')), Action::Answer(true));
        assert_eq!(keyboard.question(), None);
        // The stream has its keys back.
        assert_eq!(keyboard.press(Key::Char(' ')), Action::Pause);

        for no in [
            Key::Char('n'),
            Key::Char('N'),
            Key::Esc,
            Key::Enter,
            Key::Char(' '),
        ] {
            keyboard.ask("sure? y/n");
            assert_eq!(keyboard.press(no), Action::Answer(false), "{no:?}");
            assert_eq!(keyboard.question(), None);
        }
        keyboard.ask("sure? y/n");
        assert_eq!(keyboard.press(Key::Char('Y')), Action::Answer(true));
        // `Ctrl-C` quits also here.
        keyboard.ask("sure? y/n");
        assert_eq!(keyboard.press(Key::Ctrl('c')), Action::Quit);
    }

    #[test]
    fn the_command_line_can_be_opened_with_a_command_typed() {
        let mut keyboard = Keyboard::default();
        keyboard.open_with("save ");
        assert_eq!(prompt(&keyboard).as_deref(), Some("save |"));
        typed(&mut keyboard, "dev-api");
        assert_eq!(
            keyboard.press(Key::Enter),
            Action::Run("save dev-api".to_string())
        );
    }

    #[test]
    fn a_control_key_that_edits_nothing_is_ignored_on_the_command_line() {
        let mut keyboard = Keyboard::default();
        keyboard.press(Key::Char(':'));
        typed(&mut keyboard, "level");
        for key in [Key::Ctrl('z'), Key::Ctrl('s'), Key::Ctrl('x')] {
            assert_eq!(keyboard.press(key), Action::Nothing, "{key:?}");
        }
        assert_eq!(prompt(&keyboard).as_deref(), Some("level|"));
    }
}
