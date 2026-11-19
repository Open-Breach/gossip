//! The terminal itself: raw mode and its way back, the keys as they are
//! read, and the width of a line. This is the one place that knows
//! `crossterm`.

use std::io::{self, Write};
use std::sync::Once;
use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use tokio::sync::mpsc;

use super::keys::Key;

/// The terminal in raw mode, for as long as this lives: keys arrive one by
/// one and are not echoed.
///
/// Raw mode is taken back when this is dropped, which happens on every way
/// out of the interactive mode: quitting, an error, a signal. A panic takes
/// it back too, before the panic is printed.
pub struct RawMode(());

impl RawMode {
    pub fn enable() -> io::Result<RawMode> {
        take_raw_mode_back_on_panic();
        crossterm::terminal::enable_raw_mode()?;
        Ok(RawMode(()))
    }
}

impl Drop for RawMode {
    fn drop(&mut self) {
        let _ = crossterm::terminal::disable_raw_mode();
    }
}

/// Makes a panic leave the terminal usable: raw mode off before the panic
/// is printed, so that its message does not stagger, and the status line
/// out of its way.
fn take_raw_mode_back_on_panic() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let print_panic = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |panic| {
            let _ = crossterm::terminal::disable_raw_mode();
            let mut stderr = io::stderr().lock();
            let _ = stderr.write_all(b"\r\x1b[K");
            let _ = stderr.flush();
            drop(stderr);
            print_panic(panic);
        }));
    });
}

/// How many cells a line of the terminal has.
pub fn width() -> usize {
    crossterm::terminal::size().map_or(80, |(columns, _)| usize::from(columns))
}

/// Reads the keys of the terminal in a thread of its own and hands them
/// over as they are pressed. The thread ends when the receiver is dropped.
pub fn keys() -> mpsc::UnboundedReceiver<Key> {
    let (tx, rx) = mpsc::unbounded_channel();
    let read = move || {
        // Looking up every now and then lets the thread see that nobody
        // listens any more.
        const LOOK_UP: Duration = Duration::from_millis(100);
        while !tx.is_closed() {
            match event::poll(LOOK_UP) {
                Ok(false) => {}
                Ok(true) => match event::read() {
                    Ok(Event::Key(pressed)) => {
                        if let Some(key) = key_of(pressed)
                            && tx.send(key).is_err()
                        {
                            return;
                        }
                    }
                    Ok(_) => {}
                    Err(_) => return,
                },
                Err(_) => return,
            }
        }
    };
    // Without the thread no key is read; the stream goes on regardless.
    let _ = std::thread::Builder::new()
        .name("keys".to_string())
        .spawn(read);
    rx
}

/// A key as the interactive mode knows it; none for a key it has no use for.
fn key_of(pressed: KeyEvent) -> Option<Key> {
    if pressed.kind == KeyEventKind::Release {
        return None;
    }
    let control = pressed.modifiers.contains(KeyModifiers::CONTROL);
    let alt = pressed.modifiers.contains(KeyModifiers::ALT);
    Some(match pressed.code {
        KeyCode::Char(_) if alt => return None,
        KeyCode::Char(character) if control => Key::Ctrl(character.to_ascii_lowercase()),
        KeyCode::Char(character) => Key::Char(character),
        KeyCode::Enter => Key::Enter,
        KeyCode::Esc => Key::Esc,
        KeyCode::Backspace => Key::Backspace,
        KeyCode::Delete => Key::Delete,
        KeyCode::Left => Key::Left,
        KeyCode::Right => Key::Right,
        KeyCode::Up => Key::Up,
        KeyCode::Down => Key::Down,
        KeyCode::Home => Key::Home,
        KeyCode::End => Key::End,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pressed(code: KeyCode, modifiers: KeyModifiers) -> Option<Key> {
        key_of(KeyEvent::new(code, modifiers))
    }

    #[test]
    fn keys_as_the_terminal_reports_them() {
        let none = KeyModifiers::NONE;
        assert_eq!(pressed(KeyCode::Char(' '), none), Some(Key::Char(' ')));
        assert_eq!(pressed(KeyCode::Char(':'), none), Some(Key::Char(':')));
        // A capital letter comes with Shift and is the letter it is.
        assert_eq!(
            pressed(KeyCode::Char('Q'), KeyModifiers::SHIFT),
            Some(Key::Char('Q'))
        );
        assert_eq!(
            pressed(KeyCode::Char('?'), KeyModifiers::SHIFT),
            Some(Key::Char('?'))
        );
        assert_eq!(pressed(KeyCode::Char('ü'), none), Some(Key::Char('ü')));
        assert_eq!(pressed(KeyCode::Enter, none), Some(Key::Enter));
        assert_eq!(pressed(KeyCode::Esc, none), Some(Key::Esc));
        assert_eq!(pressed(KeyCode::Backspace, none), Some(Key::Backspace));
        assert_eq!(pressed(KeyCode::Delete, none), Some(Key::Delete));
        assert_eq!(pressed(KeyCode::Left, none), Some(Key::Left));
        assert_eq!(pressed(KeyCode::Right, none), Some(Key::Right));
        assert_eq!(pressed(KeyCode::Up, none), Some(Key::Up));
        assert_eq!(pressed(KeyCode::Down, none), Some(Key::Down));
        assert_eq!(pressed(KeyCode::Home, none), Some(Key::Home));
        assert_eq!(pressed(KeyCode::End, none), Some(Key::End));
    }

    #[test]
    fn a_letter_with_ctrl_is_a_key_of_its_own() {
        let control = KeyModifiers::CONTROL;
        assert_eq!(pressed(KeyCode::Char('c'), control), Some(Key::Ctrl('c')));
        assert_eq!(pressed(KeyCode::Char('l'), control), Some(Key::Ctrl('l')));
        // Whether or not Shift is held with it.
        assert_eq!(
            pressed(KeyCode::Char('L'), control | KeyModifiers::SHIFT),
            Some(Key::Ctrl('l'))
        );
    }

    #[test]
    fn keys_the_interactive_mode_has_no_use_for_are_dropped() {
        let none = KeyModifiers::NONE;
        assert_eq!(pressed(KeyCode::Tab, none), None);
        assert_eq!(pressed(KeyCode::F(1), none), None);
        assert_eq!(pressed(KeyCode::PageUp, none), None);
        assert_eq!(pressed(KeyCode::Insert, none), None);
        // Alt and a letter is not that letter.
        assert_eq!(pressed(KeyCode::Char('q'), KeyModifiers::ALT), None);
        // The release of a key, where a terminal reports it, is no key.
        let mut released = KeyEvent::new(KeyCode::Char('q'), none);
        released.kind = KeyEventKind::Release;
        assert_eq!(key_of(released), None);
    }
}
