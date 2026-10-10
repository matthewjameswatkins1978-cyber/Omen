//! Masked secret entry for first-run/later provider-key setup.
//!
//! Paste-friendly single-line reader: typed characters echo as `*`,
//! Backspace edits, Enter submits, Esc/Ctrl-C cancels with nothing stored.
//! The secret value never touches stdout, logs, or history — only the mask
//! is drawn. Non-interactive terminals cancel immediately (never block a
//! pipe). Terminal state is restored on every exit path.

use crossterm::event::{KeyCode, KeyModifiers};
use std::io::{self, Write};

/// Outcome of one masked read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SecretRead {
    Submitted(String),
    Cancelled,
}

/// Per-keystroke editing outcome (pure logic; TTY loop below).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SecretKeyOutcome {
    Continue,
    Submit,
    Cancel,
}

/// Apply one key to the in-progress buffer. Paste arrives as rapid
/// character events and appends naturally; control characters other than
/// Backspace/Enter/Escape are ignored.
pub fn apply_secret_key(
    buffer: &mut String,
    code: KeyCode,
    modifiers: KeyModifiers,
) -> SecretKeyOutcome {
    if modifiers.contains(KeyModifiers::CONTROL) {
        match code {
            KeyCode::Char('c') | KeyCode::Char('C') => return SecretKeyOutcome::Cancel,
            _ => return SecretKeyOutcome::Continue,
        }
    }
    match code {
        KeyCode::Enter => SecretKeyOutcome::Submit,
        KeyCode::Esc => SecretKeyOutcome::Cancel,
        KeyCode::Backspace => {
            buffer.pop();
            SecretKeyOutcome::Continue
        }
        KeyCode::Char(ch) => {
            buffer.push(ch);
            SecretKeyOutcome::Continue
        }
        _ => SecretKeyOutcome::Continue,
    }
}

/// Read one masked line after printing `prompt`. Returns
/// [`SecretRead::Cancelled`] without touching the terminal when the
/// capabilities report a non-interactive session.
pub fn read_masked_line(
    prompt: &str,
    caps: &crate::terminal::TerminalCapabilities,
) -> io::Result<SecretRead> {
    use crossterm::event::{self, Event};
    use crossterm::terminal;

    if !caps.is_interactive {
        return Ok(SecretRead::Cancelled);
    }
    print!("{prompt}");
    io::stdout().flush()?;

    terminal::enable_raw_mode()?;
    let result = (|| -> io::Result<SecretRead> {
        let mut buffer = String::new();
        let mut stdout = io::stdout();
        loop {
            let Event::Key(key) = event::read()? else {
                continue;
            };
            match apply_secret_key(&mut buffer, key.code, key.modifiers) {
                SecretKeyOutcome::Continue => {
                    // Redraw the mask line: carriage return + stars.
                    write!(stdout, "\r\x1b[K{prompt}{}", "*".repeat(buffer.len()))?;
                    stdout.flush()?;
                }
                SecretKeyOutcome::Submit => break Ok(SecretRead::Submitted(buffer)),
                SecretKeyOutcome::Cancel => break Ok(SecretRead::Cancelled),
            }
        }
    })();
    let _ = terminal::disable_raw_mode();
    println!();
    result
}

/// Blocking yes/no question on the cooked terminal (used before entering
/// raw mode). Returns `false` on EOF/non-interactive input.
pub fn ask_yes_no(prompt: &str) -> bool {
    print!("{prompt} [y/N] ");
    let _ = io::stdout().flush();
    let mut line = String::new();
    match io::stdin().read_line(&mut line) {
        Ok(_) => matches!(line.trim().to_lowercase().as_str(), "y" | "yes"),
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secret_editing_shapes() {
        let mut buffer = String::new();
        let no_mod = KeyModifiers::empty();
        for ch in "sk-test".chars() {
            assert_eq!(
                apply_secret_key(&mut buffer, KeyCode::Char(ch), no_mod),
                SecretKeyOutcome::Continue
            );
        }
        assert_eq!(buffer, "sk-test");
        assert_eq!(
            apply_secret_key(&mut buffer, KeyCode::Backspace, no_mod),
            SecretKeyOutcome::Continue
        );
        assert_eq!(buffer, "sk-tes");
        assert_eq!(
            apply_secret_key(&mut buffer, KeyCode::Enter, no_mod),
            SecretKeyOutcome::Submit
        );
        let mut empty = String::new();
        assert_eq!(
            apply_secret_key(&mut empty, KeyCode::Esc, no_mod),
            SecretKeyOutcome::Cancel
        );
        assert_eq!(
            apply_secret_key(&mut empty, KeyCode::Char('c'), KeyModifiers::CONTROL),
            SecretKeyOutcome::Cancel
        );
        // Other control combos and navigation keys never touch the buffer.
        assert_eq!(
            apply_secret_key(&mut empty, KeyCode::Char('v'), KeyModifiers::CONTROL),
            SecretKeyOutcome::Continue
        );
        assert_eq!(
            apply_secret_key(&mut empty, KeyCode::Left, no_mod),
            SecretKeyOutcome::Continue
        );
        assert!(empty.is_empty());
    }
}
