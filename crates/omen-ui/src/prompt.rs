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
        }
    }

    fn compact_path(path: &Path) -> String {
        let path_str = path.to_string_lossy().replace('\\', "/");
        if let Ok(home) = std::env::var("USERPROFILE").or_else(|_| std::env::var("HOME")) {
            let home_norm = home.replace('\\', "/");
            if let Some(rest) = path_str.strip_prefix(&home_norm) {
                return format!("~{rest}");
            }
        }
        path_str
    }
}

pub struct PromptRenderer;

impl PromptRenderer {
    pub fn render(state: &PromptState, caps: &TerminalCapabilities) -> String {
        let colors = ColorRoles::for_caps(caps);

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

        // Header line: path branch status
        // Prompt line: >
        let prompt_sym = colors.prompt_symbol.paint(">");

        format!("{path_part}{branch_part}{status_part}\n{prompt_sym} ")
    }
}
