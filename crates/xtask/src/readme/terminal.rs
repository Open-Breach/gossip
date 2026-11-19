//! A terminal for the program to run in, and what a reader would see on it.
//!
//! The walkthrough of the README is what the built binary prints when it
//! has a terminal on both sides: the interactive mode is only there then.
//! So the generator opens a pseudo-terminal of a fixed size, starts the
//! binary on it, types keys into it, and reads back what the binary writes.
//! [`Display`] turns those bytes into the lines a terminal would show: it
//! follows the carriage returns and the few escape codes the program
//! writes, and drops the colours.

use std::fs::File;
use std::io::{Read, Write};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, bail};
use rustix::pty::{OpenptFlags, grantpt, openpt, ptsname, unlockpt};
use rustix::termios::{Winsize, tcsetwinsize};

/// The size of the terminal: wide enough that no line of the walkthrough
/// is cut, and always the same.
pub const COLUMNS: u16 = 200;
pub const ROWS: u16 = 50;

/// How long a step of the walkthrough may take before it has failed.
const PATIENCE: Duration = Duration::from_secs(20);

/// What a terminal shows of the bytes written to it: the lines that are
/// finished, and the line the cursor is on.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Display {
    /// The finished lines, oldest first. Clearing the screen empties them.
    lines: Vec<String>,
    /// The line the cursor is on: the status line of the program.
    current: Vec<char>,
    column: usize,
    /// An escape sequence that has begun and not ended.
    escape: Option<String>,
    /// How many lines were finished, the cleared ones too.
    finished: usize,
}

impl Display {
    /// Takes what the program wrote.
    pub fn write(&mut self, text: &str) {
        for character in text.chars() {
            if let Some(escape) = &mut self.escape {
                escape.push(character);
                if escape_is_complete(escape) {
                    let escape = self.escape.take().unwrap_or_default();
                    self.apply(&escape);
                }
                continue;
            }
            match character {
                '\u{1b}' => self.escape = Some(String::new()),
                '\r' => self.column = 0,
                '\n' => {
                    let line: String = self.current.drain(..).collect();
                    self.lines.push(line.trim_end().to_string());
                    self.finished += 1;
                    self.column = 0;
                }
                // The bell, and whatever else does not print.
                character if character.is_control() => {}
                character => {
                    if self.column < self.current.len() {
                        self.current[self.column] = character;
                    } else {
                        self.current.resize(self.column, ' ');
                        self.current.push(character);
                    }
                    self.column += 1;
                }
            }
        }
    }

    /// An escape sequence, without the escape character in front.
    fn apply(&mut self, escape: &str) {
        let Some(body) = escape.strip_prefix('[') else {
            // Not a control sequence: nothing this program writes.
            return;
        };
        let (arguments, command) = body.split_at(body.len() - 1);
        match (command, arguments) {
            // Erase from the cursor to the end of the line, or all of it.
            ("K", "" | "0") => self.current.truncate(self.column),
            ("K", "2") => self.current.clear(),
            // Clear the screen.
            ("J", "2" | "3") => {
                self.lines.clear();
                self.current.clear();
            }
            ("H", _) => self.column = 0,
            // Colours, the cursor's look, and the rest: nothing to show.
            _ => {}
        }
    }

    /// The finished lines that are on the screen.
    pub fn lines(&self) -> &[String] {
        &self.lines
    }

    /// The line the cursor is on, which is where the program keeps its
    /// status line and its command line.
    pub fn status(&self) -> String {
        let line: String = self.current.iter().collect();
        line.trim_end().to_string()
    }

    /// How many lines were finished since the program started.
    pub fn finished(&self) -> usize {
        self.finished
    }
}

/// Whether an escape sequence, without the escape character, has ended.
fn escape_is_complete(escape: &str) -> bool {
    let mut characters = escape.chars();
    match characters.next() {
        // A control sequence ends with a letter or one of a few signs.
        Some('[') => escape.len() > 1 && escape.ends_with(|last: char| ('@'..='~').contains(&last)),
        // An operating system command ends with the bell or `ESC \`.
        Some(']') => escape.ends_with('\u{7}') || escape.ends_with("\u{1b}\\"),
        // Anything else is two characters long.
        Some(_) => true,
        None => false,
    }
}

/// The program running on a pseudo-terminal.
pub struct Terminal {
    child: Child,
    /// Our end of the terminal: what is written here is typed.
    keys: File,
    display: Arc<Mutex<Display>>,
    /// Everything the program wrote, for when a step fails.
    raw: Arc<Mutex<String>>,
}

impl Terminal {
    /// Starts a command with the terminal on its three streams.
    pub fn start(mut command: Command) -> anyhow::Result<Terminal> {
        let ours = openpt(OpenptFlags::RDWR | OpenptFlags::NOCTTY)
            .context("no pseudo-terminal could be opened")?;
        grantpt(&ours)?;
        unlockpt(&ours)?;
        let name = ptsname(&ours, Vec::new())?;
        let name = name.to_str().context("the name of the terminal")?;
        let theirs = open_terminal(Path::new(name))?;
        tcsetwinsize(
            &theirs,
            Winsize {
                ws_row: ROWS,
                ws_col: COLUMNS,
                ws_xpixel: 0,
                ws_ypixel: 0,
            },
        )?;
        command
            .stdin(Stdio::from(theirs.try_clone()?))
            .stdout(Stdio::from(theirs.try_clone()?))
            .stderr(Stdio::from(theirs));
        let child = command.spawn().context("the program did not start")?;

        let keys = File::from(ours);
        let display = Arc::new(Mutex::new(Display::default()));
        let raw = Arc::new(Mutex::new(String::new()));
        let mut screen = keys.try_clone()?;
        let (shown, written) = (display.clone(), raw.clone());
        std::thread::Builder::new()
            .name("terminal".to_string())
            .spawn(move || {
                let mut buffer = [0u8; 4096];
                let mut pending = Vec::new();
                // The read fails when the program has gone and closed its
                // end: that is the end of the output.
                while let Ok(count @ 1..) = screen.read(&mut buffer) {
                    pending.extend_from_slice(&buffer[..count]);
                    // Only whole characters: one may be cut by the read.
                    let whole = match std::str::from_utf8(&pending) {
                        Ok(text) => text.len(),
                        Err(error) => error.valid_up_to(),
                    };
                    let text = String::from_utf8_lossy(&pending[..whole]).into_owned();
                    pending.drain(..whole);
                    shown.lock().unwrap().write(&text);
                    written.lock().unwrap().push_str(&text);
                }
            })?;
        Ok(Terminal {
            child,
            keys,
            display,
            raw,
        })
    }

    /// Types this text: every character is a key.
    pub fn type_text(&mut self, text: &str) -> anyhow::Result<()> {
        self.keys.write_all(text.as_bytes())?;
        self.keys.flush()?;
        Ok(())
    }

    /// What the terminal shows now.
    pub fn display(&self) -> Display {
        self.display.lock().unwrap().clone()
    }

    /// Waits until what the terminal shows is what is asked for, and gives
    /// it. An error that shows the terminal when it does not come.
    pub async fn wait(
        &self,
        what: &str,
        wanted: impl Fn(&Display) -> bool,
    ) -> anyhow::Result<Display> {
        let start = Instant::now();
        loop {
            let display = self.display();
            if wanted(&display) {
                return Ok(display);
            }
            if start.elapsed() > PATIENCE {
                bail!(
                    "the terminal never showed {what}. It shows:\n{}\n[{}]\nAll that was \
                     written:\n{:?}",
                    display.lines().join("\n"),
                    display.status(),
                    self.raw.lock().unwrap()
                );
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }

    /// Waits until a finished line contains this text.
    pub async fn wait_for_line(&self, text: &str) -> anyhow::Result<Display> {
        self.wait(&format!("a line with `{text}`"), |display| {
            display.lines().iter().any(|line| line.contains(text))
        })
        .await
    }

    /// Waits until the status line contains this text.
    pub async fn wait_for_status(&self, text: &str) -> anyhow::Result<Display> {
        self.wait(&format!("`{text}` on the status line"), |display| {
            display.status().contains(text)
        })
        .await
    }

    /// Waits for the program to end by itself, and gives its exit code.
    pub async fn wait_for_exit(&mut self) -> anyhow::Result<Option<i32>> {
        let start = Instant::now();
        loop {
            if let Some(status) = self.child.try_wait()? {
                // What it wrote last is read by now, or will be in a moment.
                tokio::time::sleep(Duration::from_millis(50)).await;
                return Ok(status.code());
            }
            if start.elapsed() > PATIENCE {
                bail!(
                    "the program did not end. The terminal shows:\n{}\n[{}]",
                    self.display().lines().join("\n"),
                    self.display().status()
                );
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Opens the program's end of the terminal.
fn open_terminal(path: &Path) -> anyhow::Result<File> {
    std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .with_context(|| format!("the terminal {} could not be opened", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shown(text: &str) -> Display {
        let mut display = Display::default();
        display.write(text);
        display
    }

    #[test]
    fn lines_end_with_a_line_feed_and_the_rest_is_the_status_line() {
        let display = shown("one\r\ntwo\r\nthree");
        assert_eq!(display.lines(), ["one", "two"]);
        assert_eq!(display.status(), "three");
        assert_eq!(display.finished(), 2);
    }

    #[test]
    fn a_status_line_is_erased_and_written_again() {
        // What the program writes around a line while it is paused.
        let display = shown("\r\u{1b}[Kpaused\r\u{1b}[Kone\r\n\r\u{1b}[Kpaused: 1 line");
        assert_eq!(display.lines(), ["one"]);
        assert_eq!(display.status(), "paused: 1 line");
        // A carriage return alone writes over what is there.
        let display = shown("abcdef\rxy");
        assert_eq!(display.status(), "xycdef");
    }

    #[test]
    fn colours_are_dropped_and_clearing_the_screen_empties_it() {
        let display = shown("\u{1b}[2m14:02:11.482\u{1b}[0m \u{1b}[1;31mready\u{1b}[0m\r\n");
        assert_eq!(display.lines(), ["14:02:11.482 ready"]);
        let display = shown("one\r\ntwo\r\n\u{1b}[2J\u{1b}[Hthree\r\n");
        assert_eq!(display.lines(), ["three"]);
        assert_eq!(display.finished(), 3);
    }

    #[test]
    fn an_escape_sequence_may_arrive_in_pieces() {
        let mut display = Display::default();
        display.write("a\u{1b}");
        display.write("[");
        display.write("31");
        display.write("mb\u{1b}]0;title\u{7}c\r\n");
        assert_eq!(display.lines(), ["abc"]);
    }
}
