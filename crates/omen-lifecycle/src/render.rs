//! Degraded terminal rendering: one truth, four projections (H items 52-59).
//!
//! Rich / Standard / Plain / Machine project the SAME underlying result.
//! Colour is decoration, never meaning: NO_COLOR and non-TTY keep full
//! textual structure. OSC features are enhancements, never semantic
//! dependencies. When capability is uncertain: under-render, never lie.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OutputMode {
    Rich,
    Standard,
    Plain,
    Machine,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderCaps {
    pub is_tty: bool,
    pub no_color: bool,
    pub dumb: bool,
    pub width: u16,
    pub unicode: bool,
    pub reduced_motion: bool,
}

impl RenderCaps {
    /// Detect from the environment + stdout TTY state (std only; omen-ui
    /// owns rich TTY truth, lifecycle stays dependency-light).
    pub fn detect() -> Self {
        use std::io::IsTerminal;
        Self::from_env(std::io::stdout().is_terminal())
    }

    pub fn from_env(is_tty: bool) -> Self {
        let term = std::env::var("TERM").unwrap_or_default().to_lowercase();
        let dumb = term == "dumb";
        let no_color = std::env::var("NO_COLOR").is_ok();
        let unicode = !dumb
            && (std::env::var("OMEN_ASCII").is_err()
                && (std::env::var("LANG")
                    .unwrap_or_default()
                    .to_lowercase()
                    .contains("utf")
                    || std::env::var("LC_ALL")
                        .unwrap_or_default()
                        .to_lowercase()
                        .contains("utf")
                    || cfg!(windows)));
        let width = std::env::var("COLUMNS")
            .ok()
            .and_then(|w| w.parse().ok())
            .unwrap_or(80);
        let reduced_motion = std::env::var("OMEN_REDUCED_MOTION").is_ok() || !is_tty || dumb;
        Self {
            is_tty,
            no_color,
            dumb,
            width: width.max(20),
            unicode,
            reduced_motion,
        }
    }

    /// Project a requested mode through capability truth. Uncertain caps
    /// degrade DOWN, never up: Rich on a dumb terminal becomes Standard;
    /// anything non-TTY becomes Plain unless Machine was requested.
    pub fn project(&self, want: OutputMode) -> OutputMode {
        match want {
            OutputMode::Machine => OutputMode::Machine,
            OutputMode::Rich => {
                if !self.is_tty || self.dumb {
                    OutputMode::Standard
                } else {
                    OutputMode::Rich
                }
            }
            OutputMode::Standard => {
                if !self.is_tty || self.dumb || self.no_color {
                    OutputMode::Plain
                } else {
                    OutputMode::Standard
                }
            }
            OutputMode::Plain => OutputMode::Plain,
        }
    }

    pub fn prompt_glyph(&self) -> &'static str {
        if self.unicode { "›" } else { ">" }
    }

    /// Canonical compact fallback mark. Always `\O/ >` in ASCII contexts.
    pub fn fallback_mark(&self) -> &'static str {
        "\\O/ >"
    }
}

/// A semantic message: status word + body. Status is ALWAYS a text token
/// ([ok]/[warn]/[fail]/[info]), never colour alone.
#[derive(Debug, Clone)]
pub struct Message {
    pub status: &'static str,
    pub headline: String,
    pub body: Vec<String>,
}

impl Message {
    pub fn render(&self, mode: OutputMode, caps: &RenderCaps) -> String {
        let mut out = String::new();
        match mode {
            OutputMode::Machine => {
                out.push_str(
                    &serde_json::json!({
                        "status": self.status,
                        "headline": self.headline,
                        "body": self.body,
                    })
                    .to_string(),
                );
                out.push('\n');
            }
            OutputMode::Rich | OutputMode::Standard => {
                let tag = format!("[{}]", self.status);
                out.push_str(&wrap(&format!("{tag} {}", self.headline), caps.width));
                out.push('\n');
                for line in &self.body {
                    out.push_str(&wrap(&format!("  {line}"), caps.width));
                    out.push('\n');
                }
            }
            OutputMode::Plain => {
                out.push_str(&format!("[{}] {}\n", self.status, self.headline));
                for line in &self.body {
                    out.push_str(&format!("  {line}\n"));
                }
            }
        }
        // Plain/Machine/pipe output is append-only: no cursor rewriting,
        // no spinners, no ANSI. Rich on a TTY may use decoration, but the
        // tokens above are identical in every projection.
        out
    }
}

/// Narrow terminals: wrap at width, never hiding the status token (it is
/// always on the first line, never wrapped away).
fn wrap(line: &str, width: u16) -> String {
    let w = width.max(20) as usize;
    if line.len() <= w {
        return line.to_string();
    }
    let mut out = String::new();
    let mut cur = String::new();
    for word in line.split_whitespace() {
        if cur.len() + word.len() + 1 > w && !cur.is_empty() {
            out.push_str(cur.trim_end());
            out.push('\n');
            cur = String::from("  ");
        }
        cur.push_str(word);
        cur.push(' ');
    }
    out.push_str(cur.trim_end());
    out
}

/// Spinner/animation policy: none when reduced motion, non-TTY, or dumb.
/// Returns true when a spinner frame may be emitted.
pub fn spinner_allowed(caps: &RenderCaps) -> bool {
    caps.is_tty && !caps.reduced_motion && !caps.dumb
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static ENV_GUARD: Mutex<()> = Mutex::new(());

    struct EnvRestore {
        key: &'static str,
        prev: Option<String>,
    }

    impl EnvRestore {
        fn set(key: &'static str, value: &str) -> Self {
            let prev = std::env::var(key).ok();
            // SAFETY: serialized by ENV_GUARD; restored on drop.
            unsafe { std::env::set_var(key, value) };
            Self { key, prev }
        }
        fn unset(key: &'static str) -> Self {
            let prev = std::env::var(key).ok();
            // SAFETY: serialized by ENV_GUARD; restored on drop.
            unsafe { std::env::remove_var(key) };
            Self { key, prev }
        }
    }

    impl Drop for EnvRestore {
        fn drop(&mut self) {
            unsafe {
                match &self.prev {
                    Some(v) => std::env::set_var(self.key, v),
                    None => std::env::remove_var(self.key),
                }
            }
        }
    }

    fn caps() -> RenderCaps {
        RenderCaps {
            is_tty: false,
            no_color: false,
            dumb: false,
            width: 80,
            unicode: false,
            reduced_motion: true,
        }
    }

    #[test]
    fn same_meaning_every_projection() {
        let m = Message {
            status: "fail",
            headline: "update failed".to_string(),
            body: vec!["previous install preserved".to_string()],
        };
        for mode in [
            OutputMode::Rich,
            OutputMode::Standard,
            OutputMode::Plain,
            OutputMode::Machine,
        ] {
            let t = m.render(mode, &caps());
            assert!(t.contains("fail"), "{mode:?}");
            assert!(t.contains("update failed"), "{mode:?}");
            assert!(t.contains("previous install preserved"), "{mode:?}");
        }
    }

    #[test]
    fn degrade_down_never_up() {
        let c = caps();
        assert_eq!(c.project(OutputMode::Rich), OutputMode::Standard);
        assert_eq!(c.project(OutputMode::Machine), OutputMode::Machine);
        let tty = RenderCaps { is_tty: true, ..c };
        assert_eq!(tty.project(OutputMode::Rich), OutputMode::Rich);
    }

    #[test]
    fn status_token_survives_narrow() {
        let narrow = RenderCaps {
            width: 24,
            ..caps()
        };
        let m = Message {
            status: "warn",
            headline: "a very long headline that must wrap somewhere".to_string(),
            body: vec![],
        };
        let t = m.render(OutputMode::Plain, &narrow);
        assert!(t.lines().next().unwrap().starts_with("[warn]"));
    }

    #[test]
    fn ascii_fallback_mark() {
        assert_eq!(caps().fallback_mark(), "\\O/ >");
    }

    #[test]
    fn no_spinner_when_reduced() {
        assert!(!spinner_allowed(&caps()));
    }

    #[test]
    fn degraded_terminal_matrix() {
        let _lock = ENV_GUARD.lock().unwrap();
        // NO_COLOR forces Plain projection of a Standard request.
        let _no = EnvRestore::set("NO_COLOR", "1");
        let _term = EnvRestore::set("TERM", "xterm-256color");
        let c = RenderCaps::from_env(true);
        assert!(c.no_color);
        assert_eq!(c.project(OutputMode::Standard), OutputMode::Plain);
        drop(_no);
        drop(_term);
        // Dumb terminal: unicode off, Rich degrades, motion reduced.
        let _dumb = EnvRestore::set("TERM", "dumb");
        let _nc = EnvRestore::unset("NO_COLOR");
        let d = RenderCaps::from_env(true);
        assert!(d.dumb && !d.unicode && d.reduced_motion);
        assert_eq!(d.project(OutputMode::Rich), OutputMode::Standard);
        assert!(!spinner_allowed(&d));
        drop(_dumb);
        drop(_nc);
        // Forced ASCII: prompt falls back to '>'.
        let _ascii = EnvRestore::set("OMEN_ASCII", "1");
        let _t2 = EnvRestore::set("TERM", "xterm-256color");
        let a = RenderCaps::from_env(true);
        assert!(!a.unicode);
        assert_eq!(a.prompt_glyph(), ">");
        // Narrow width honored and bounded below.
        let _w = EnvRestore::set("COLUMNS", "30");
        let n = RenderCaps::from_env(false);
        assert_eq!(n.width, 30);
        assert_eq!(n.project(OutputMode::Standard), OutputMode::Plain);
    }
}
