use crate::appearance::Theme;
use crate::terminal::TerminalCapabilities;
use nu_ansi_term::{Color, Style};

/// Semantic color roles for Omen UI.
#[derive(Debug, Clone, Copy)]
pub struct ColorRoles {
    pub normal: Style,
    pub subtle: Style,
    pub prompt_path: Style,
    pub prompt_branch: Style,
    pub prompt_symbol: Style,
    pub success: Style,
    pub warning: Style,
    pub error: Style,
    pub reference: Style,
    pub suggestion: Style,
    pub block_border: Style,
}

impl ColorRoles {
    pub fn for_caps(caps: &TerminalCapabilities) -> Self {
        Self::for_theme(caps, Theme::Omen)
    }

    pub fn for_theme(caps: &TerminalCapabilities, theme: Theme) -> Self {
        if !caps.has_color {
            return Self::plain();
        }

        let (path, branch, symbol, success, warning) = match theme {
            Theme::Omen => (
                Color::Cyan,
                Color::Purple,
                Color::Green,
                Color::Green,
                Color::Yellow,
            ),
            Theme::Phosphor => (
                Color::Green,
                Color::LightGreen,
                Color::Green,
                Color::Green,
                Color::Yellow,
            ),
            Theme::Amber => (
                Color::Yellow,
                Color::LightYellow,
                Color::Yellow,
                Color::Yellow,
                Color::LightYellow,
            ),
            Theme::Ice => (
                Color::Cyan,
                Color::White,
                Color::LightCyan,
                Color::LightCyan,
                Color::Yellow,
            ),
        };
        Self {
            normal: Style::new(),
            subtle: Style::new().fg(Color::DarkGray),
            prompt_path: Style::new().fg(path).bold(),
            prompt_branch: Style::new().fg(branch),
            prompt_symbol: Style::new().fg(symbol),
            success: Style::new().fg(success),
            warning: Style::new().fg(warning),
            error: Style::new().fg(Color::Red).bold(),
            reference: Style::new().fg(Color::Blue),
            suggestion: Style::new().fg(Color::DarkGray),
            block_border: Style::new().fg(Color::DarkGray),
        }
    }

    pub fn plain() -> Self {
        Self {
            normal: Style::new(),
            subtle: Style::new(),
            prompt_path: Style::new(),
            prompt_branch: Style::new(),
            prompt_symbol: Style::new(),
            success: Style::new(),
            warning: Style::new(),
            error: Style::new(),
            reference: Style::new(),
            suggestion: Style::new(),
            block_border: Style::new(),
        }
    }
}
