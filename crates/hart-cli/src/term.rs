//! Host terminal handling: raw mode for the console and the QEMU-style
//! Ctrl-A escape.

use std::sync::OnceLock;

static SAVED: OnceLock<libc::termios> = OnceLock::new();

/// Puts stdin's terminal in raw mode until dropped. Signals (Ctrl-C) and
/// line editing go to the guest.
pub struct RawMode {
    saved: libc::termios,
}

impl RawMode {
    /// Returns `None` when stdin is not a terminal.
    pub fn enable() -> Option<Self> {
        // SAFETY: termios is plain data and fd 0 is valid for the process lifetime.
        unsafe {
            if libc::isatty(0) != 1 {
                return None;
            }
            let mut t: libc::termios = std::mem::zeroed();
            if libc::tcgetattr(0, &mut t) != 0 {
                return None;
            }
            let saved = t;
            libc::cfmakeraw(&mut t);
            if libc::tcsetattr(0, libc::TCSANOW, &t) != 0 {
                return None;
            }
            let _ = SAVED.set(saved);
            install_panic_hook();
            Some(Self { saved })
        }
    }
}

impl Drop for RawMode {
    fn drop(&mut self) {
        // SAFETY: restores the attributes read in `enable`.
        unsafe {
            libc::tcsetattr(0, libc::TCSANOW, &self.saved);
        }
    }
}

fn install_panic_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if let Some(saved) = SAVED.get() {
            // SAFETY: restores the attributes read in `RawMode::enable`.
            unsafe {
                libc::tcsetattr(0, libc::TCSANOW, saved);
            }
        }
        previous(info);
    }));
}

pub const ESCAPE: u8 = 0x01;

pub const ESCAPE_HELP: &str = "\r\nC-a x    exit hart\r\nC-a s    show instruction count and speed\r\nC-a C-a  send C-a to the guest\r\nC-a h    show this help\r\n";

/// What a key press on the host console means.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    /// Send this byte to the guest.
    Guest(u8),
    Quit,
    Stats,
    Help,
}

/// Splits host input into guest bytes and Ctrl-A escape commands.
#[derive(Debug, Default)]
pub struct EscapeParser {
    after_escape: bool,
}

impl EscapeParser {
    pub fn feed(&mut self, byte: u8) -> Option<Key> {
        if self.after_escape {
            self.after_escape = false;
            return match byte {
                b'x' | b'X' => Some(Key::Quit),
                b's' | b'S' => Some(Key::Stats),
                b'h' | b'H' | b'?' => Some(Key::Help),
                ESCAPE | b'a' => Some(Key::Guest(ESCAPE)),
                _ => None,
            };
        }
        if byte == ESCAPE {
            self.after_escape = true;
            return None;
        }
        Some(Key::Guest(byte))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(input: &[u8]) -> Vec<Key> {
        let mut p = EscapeParser::default();
        input.iter().filter_map(|&b| p.feed(b)).collect()
    }

    #[test]
    fn plain_bytes_pass_through() {
        assert_eq!(keys(b"ls\r"), vec![Key::Guest(b'l'), Key::Guest(b's'), Key::Guest(b'\r')]);
        assert_eq!(keys(&[0x03]), vec![Key::Guest(0x03)], "Ctrl-C goes to the guest");
    }

    #[test]
    fn escape_commands() {
        assert_eq!(keys(&[ESCAPE, b'x']), vec![Key::Quit]);
        assert_eq!(keys(&[ESCAPE, b's']), vec![Key::Stats]);
        assert_eq!(keys(&[ESCAPE, b'h']), vec![Key::Help]);
        assert_eq!(keys(&[ESCAPE, ESCAPE]), vec![Key::Guest(ESCAPE)]);
        assert_eq!(keys(&[ESCAPE, b'q', b'z']), vec![Key::Guest(b'z')], "unknown escapes are dropped");
        assert_eq!(keys(&[b'a', ESCAPE, b'x', b'b']), vec![Key::Guest(b'a'), Key::Quit, Key::Guest(b'b')]);
    }
}
