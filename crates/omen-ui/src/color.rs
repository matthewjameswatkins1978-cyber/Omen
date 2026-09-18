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
        if !caps.has_color {
            return Self::plain();
        }

        Self {
            normal: Style::new(),
            subtle: Style::new().fg(Color::DarkGray),
            prompt_path: Style::new().fg(Color::Cyan).bold(),
            prompt_branch: Style::new().fg(Color::Purple),
            prompt_symbol: Style::new().fg(Color::Green),
            success: Style::new().fg(Color::Green),
            warning: Style::new().fg(Color::Yellow),
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
