use crate::appearance::{PromptDensity, Theme};
use crate::color::ColorRoles;
use crate::terminal::TerminalCapabilities;
use std::path::Path;

/// Contextual prompt state.
#[derive(Debug, Clone)]
pub struct PromptState {
    pub cwd_display: String,
    pub branch: Option<String>,
    pub dirty_facts_count: usize,
    pub has_failure: bool,
    pub mode_indicator: Option<String>,
    pub theme: Theme,
    pub density: PromptDensity,
}

impl PromptState {
    pub fn new(
        cwd: &Path,
        branch: Option<String>,
        dirty_facts_count: usize,
        has_failure: bool,
    ) -> Self {
        let cwd_display = Self::compact_path(cwd);
        Self {
            cwd_display,
            branch,
            dirty_facts_count,
            has_failure,
            mode_indicator: None,
            theme: Theme::default(),
            density: PromptDensity::Normal,
        }
    }

    pub fn with_mode_indicator(mut self, indicator: impl Into<String>) -> Self {
        self.mode_indicator = Some(indicator.into());
        self
    }

    fn compact_path(path: &Path) -> String {
        let human = crate::humanize::humanize_path(path);
        if let Ok(home) = std::env::var("USERPROFILE").or_else(|_| std::env::var("HOME")) {
            let home_h = crate::humanize::humanize_path_str(&home);
            if let Some(rest) = human.strip_prefix(&home_h) {
                return format!("~{rest}");
            }
            #[cfg(windows)]
            if human.len() >= home_h.len()
                && human[..home_h.len()].eq_ignore_ascii_case(&home_h)
            {
                return format!("~{}", &human[home_h.len()..]);
            }
        }
        human
    }

    pub fn with_theme(mut self, theme: Theme) -> Self {
        self.theme = theme;
        self
    }

    pub fn with_density(mut self, density: PromptDensity) -> Self {
        self.density = density;
        self
    }
}

pub struct PromptRenderer;

impl PromptRenderer {
    pub fn render(state: &PromptState, caps: &TerminalCapabilities) -> String {
        let colors = ColorRoles::for_theme(caps, state.theme);

        if state.density == PromptDensity::Compact {
            let p_start = crate::blocks::SemanticBlock::osc133_prompt_start(caps);
            let c_start = crate::blocks::SemanticBlock::osc133_command_start(caps);
            let prompt_text = if caps.has_unicode {
                "\\O/ ›"
            } else {
                "\\O/ >"
            };
            let prompt_sym = colors.prompt_symbol.paint(prompt_text);
            return format!("{p_start}{prompt_sym} {c_start}");
        }

        let path_part = colors.prompt_path.paint(&state.cwd_display);

        let branch_part = match &state.branch {
            Some(b) => format!("  {}", colors.prompt_branch.paint(b)),
            None => String::new(),
        };

        let status_part = if state.has_failure {
            let sym = if caps.has_unicode { "✕" } else { "X" };
            format!(" {}", colors.error.paint(sym))
        } else if state.dirty_facts_count > 0 {
            let sym = "!";
            format!(
                " {}",
                colors
                    .warning
                    .paint(format!("{sym} {} dirty", state.dirty_facts_count))
            )
        } else {
            let sym = if caps.has_unicode { "✓" } else { "ok" };
            format!(" {}", colors.success.paint(sym))
        };

        let mode_part = match &state.mode_indicator {
            Some(ind) => format!("{} ", colors.subtle.paint(ind)),
            None => String::new(),
        };

        // Header line: path branch status
        // Prompt line: [mode] >
        let prompt_text = if caps.has_unicode {
            "\\O/ ›"
        } else {
            "\\O/ >"
        };
        let prompt_sym = colors.prompt_symbol.paint(prompt_text);
        let p_start = crate::blocks::SemanticBlock::osc133_prompt_start(caps);
        let c_start = crate::blocks::SemanticBlock::osc133_command_start(caps);

        format!("{p_start}{path_part}{branch_part}{status_part}\n{mode_part}{prompt_sym} {c_start}")
    }
}
