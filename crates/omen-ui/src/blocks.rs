use crate::terminal::TerminalCapabilities;
use std::path::Path;

pub struct SemanticBlock;

impl SemanticBlock {
    /// Generates OSC 7 sequence to notify terminal of current working directory.
    pub fn osc7_cwd(path: &Path, caps: &TerminalCapabilities) -> String {
        if !caps.has_osc7 {
            return String::new();
        }
        let normalized = path.to_string_lossy().replace('\\', "/");
        let path_part = if normalized.starts_with('/') {
            normalized
        } else {
            format!("/{normalized}")
        };
        format!("\x1b]7;file://localhost{path_part}\x1b\\")
    }

    /// Generates OSC 8 hyperlink sequence if supported, else returns raw text.
    pub fn osc8_link(url: &str, text: &str, caps: &TerminalCapabilities) -> String {
        if !caps.has_osc8 {
            return text.to_string();
        }
        format!("\x1b]8;;{url}\x1b\\{text}\x1b]8;;\x1b\\")
    }

    /// FinalTerm / OSC 133;A: Prompt start.
    pub fn osc133_prompt_start(caps: &TerminalCapabilities) -> String {
        if !caps.has_osc133 {
            return String::new();
        }
        "\x1b]133;A\x1b\\".to_string()
    }

    /// FinalTerm / OSC 133;B: Command input start.
    pub fn osc133_command_start(caps: &TerminalCapabilities) -> String {
        if !caps.has_osc133 {
            return String::new();
        }
        "\x1b]133;B\x1b\\".to_string()
    }

    /// FinalTerm / OSC 133;C: Command execution start (post-enter).
    pub fn osc133_command_executed(caps: &TerminalCapabilities) -> String {
        if !caps.has_osc133 {
            return String::new();
        }
        "\x1b]133;C\x1b\\".to_string()
    }

    /// FinalTerm / OSC 133;D: Command finished with exit code.
    pub fn osc133_command_finished(exit_code: i32, caps: &TerminalCapabilities) -> String {
        if !caps.has_osc133 {
            return String::new();
        }
        format!("\x1b]133;D;{exit_code}\x1b\\")
    }

    /// Convenience helper to wrap a local file in an OSC 8 link.
    pub fn link_file(path: &Path, label: Option<&str>, caps: &TerminalCapabilities) -> String {
        let display = label.unwrap_or_else(|| path.to_str().unwrap_or(""));
        let url = format!("file://{}", path.display());
        Self::osc8_link(&url, display, caps)
    }

    /// Convenience helper to wrap an artifact URI in an OSC 8 link.
    pub fn link_artifact(
        artifact_uri: &str,
        label: Option<&str>,
        caps: &TerminalCapabilities,
    ) -> String {
        let display = label.unwrap_or(artifact_uri);
        Self::osc8_link(artifact_uri, display, caps)
    }
}
