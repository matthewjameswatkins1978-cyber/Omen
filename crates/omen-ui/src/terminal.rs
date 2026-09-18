use serde::{Deserialize, Serialize};

/// Detected terminal capabilities.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalCapabilities {
    pub is_interactive: bool,
    pub has_color: bool,
    pub has_truecolor: bool,
    pub has_unicode: bool,
    pub has_osc7: bool,
    pub has_osc8: bool,
    pub has_osc133: bool,
    pub width: u16,
    pub height: u16,
}

impl Default for TerminalCapabilities {
    fn default() -> Self {
        Self::detect()
    }
}

impl TerminalCapabilities {
    pub fn detect() -> Self {
        let is_interactive = crossterm::tty::IsTty::is_tty(&std::io::stdout());
        let term_var = std::env::var("TERM").unwrap_or_default().to_lowercase();
        let colorterm_var = std::env::var("COLORTERM")
            .unwrap_or_default()
            .to_lowercase();
        let no_color = std::env::var("NO_COLOR").is_ok();

        let is_dumb = term_var == "dumb";

        let has_color = !no_color && !is_dumb && (is_interactive || !term_var.is_empty());
        let has_truecolor =
            has_color && (colorterm_var.contains("truecolor") || colorterm_var.contains("24bit"));

        let has_unicode = !is_dumb
            && (std::env::var("LANG")
                .unwrap_or_default()
                .to_lowercase()
                .contains("utf")
                || std::env::var("LC_ALL")
                    .unwrap_or_default()
                    .to_lowercase()
                    .contains("utf")
                || cfg!(windows));

        let (width, height) = if is_interactive {
            crossterm::terminal::size().unwrap_or((80, 24))
        } else {
            (80, 24)
        };

        // OSC capabilities detection based on known terminals
        let term_prog = std::env::var("TERM_PROGRAM")
            .unwrap_or_default()
            .to_lowercase();
        let wt_session = std::env::var("WT_SESSION").is_ok();
        let is_wezterm = term_prog.contains("wezterm");
        let is_iterm = term_prog.contains("iterm");
        let is_kitty = term_var.contains("kitty");
        let is_ghostty = term_prog.contains("ghostty");

        let has_osc7 = is_wezterm || is_iterm || is_kitty || is_ghostty;
        let has_osc8 = is_wezterm || is_iterm || is_kitty || is_ghostty || wt_session;
        let has_osc133 = is_wezterm || is_iterm || is_ghostty;

        Self {
            is_interactive,
            has_color,
            has_truecolor,
            has_unicode,
            has_osc7,
            has_osc8,
            has_osc133,
            width,
            height,
        }
    }

    pub fn dumb() -> Self {
        Self {
            is_interactive: false,
            has_color: false,
            has_truecolor: false,
            has_unicode: false,
            has_osc7: false,
            has_osc8: false,
            has_osc133: false,
            width: 80,
            height: 24,
        }
    }
}
