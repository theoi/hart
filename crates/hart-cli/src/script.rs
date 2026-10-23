//! Console automation: `--script` types lines after the prompt appears, and
//! `--exit-on` watches for a string.

use std::collections::VecDeque;

const TAIL: usize = 8192;

/// Types one line per prompt and reports when the last command's prompt
/// has returned.
#[derive(Debug)]
pub struct Script {
    lines: VecDeque<String>,
    prompt: Vec<u8>,
    tail: Vec<u8>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum ScriptEvent {
    Type(Vec<u8>),
    Finished,
}

impl Script {
    pub fn new(text: &str, prompt: &str) -> Self {
        Self { lines: text.lines().map(str::to_string).collect(), prompt: prompt.as_bytes().to_vec(), tail: Vec::new() }
    }

    /// Feeds console output. Call once per batch of output; the prompt must
    /// be the last thing printed.
    pub fn on_output(&mut self, out: &[u8]) -> Option<ScriptEvent> {
        self.tail.extend_from_slice(out);
        if self.tail.len() > TAIL {
            self.tail.drain(..self.tail.len() - TAIL);
        }
        if out.is_empty() || !self.tail.ends_with(&self.prompt) {
            return None;
        }
        self.tail.clear();
        Some(match self.lines.pop_front() {
            Some(line) => {
                let mut bytes = line.into_bytes();
                bytes.push(b'\n');
                ScriptEvent::Type(bytes)
            }
            None => ScriptEvent::Finished,
        })
    }
}

/// Watches console output for a string, including across batches.
#[derive(Debug)]
pub struct Watch {
    needle: Vec<u8>,
    tail: Vec<u8>,
}

impl Watch {
    pub fn new(needle: &str) -> Self {
        Self { needle: needle.as_bytes().to_vec(), tail: Vec::new() }
    }

    pub fn seen(&mut self, out: &[u8]) -> bool {
        if self.needle.is_empty() {
            return true;
        }
        self.tail.extend_from_slice(out);
        let found = self.tail.windows(self.needle.len()).any(|w| w == self.needle.as_slice());
        let keep = self.needle.len() - 1;
        if self.tail.len() > keep {
            self.tail.drain(..self.tail.len() - keep);
        }
        found
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn types_after_each_prompt_then_finishes() {
        let mut s = Script::new("uname -a\necho hi\n", "~ # ");
        assert_eq!(s.on_output(b"booting...\r\n"), None);
        assert_eq!(s.on_output(b"Welcome to hart!\r\n~ # "), Some(ScriptEvent::Type(b"uname -a\n".to_vec())));
        assert_eq!(s.on_output(b"uname -a\r\nLinux hart"), None);
        assert_eq!(s.on_output(b" riscv64\r\n~ # "), Some(ScriptEvent::Type(b"echo hi\n".to_vec())));
        assert_eq!(s.on_output(b"echo hi\r\nhi\r\n~ "), None);
        assert_eq!(s.on_output(b"# "), Some(ScriptEvent::Finished));
    }

    #[test]
    fn watch_spans_batches() {
        let mut w = Watch::new("OpenSBI v");
        assert!(!w.seen(b"xxOpen"));
        assert!(w.seen(b"SBI v1.8"));
        let mut w = Watch::new("abc");
        assert!(!w.seen(b"ab"));
        assert!(!w.seen(b"xc"));
    }
}
