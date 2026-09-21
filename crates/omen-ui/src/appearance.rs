use crate::prompt::{PromptRenderer, PromptState};
use crate::terminal::TerminalCapabilities;
use serde::{Deserialize, Serialize};
use std::io::{self, Write};
use std::path::Path;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    #[default]
    Omen,
    Phosphor,
    Amber,
    Ice,
}

impl Theme {
    pub const ALL: [Self; 4] = [Self::Omen, Self::Phosphor, Self::Amber, Self::Ice];

    pub fn label(self) -> &'static str {
        match self {
            Self::Omen => "Omen",
            Self::Phosphor => "Phosphor",
            Self::Amber => "Amber",
            Self::Ice => "Ice",
        }
    }

    pub fn next(self) -> Self {
        let index = Self::ALL
            .iter()
            .position(|theme| *theme == self)
            .unwrap_or(0);
        Self::ALL[(index + 1) % Self::ALL.len()]
    }

    pub fn previous(self) -> Self {
        let index = Self::ALL
            .iter()
            .position(|theme| *theme == self)
            .unwrap_or(0);
        Self::ALL[(index + Self::ALL.len() - 1) % Self::ALL.len()]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum PromptDensity {
    #[default]
    Normal,
    Compact,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct HumanSettings {
    pub theme: Theme,
    pub density: PromptDensity,
    pub epigraph: bool,
}

impl Default for HumanSettings {
    fn default() -> Self {
        Self {
            theme: Theme::Omen,
            density: PromptDensity::Normal,
            epigraph: true,
        }
    }
}

impl HumanSettings {
    pub fn default_path() -> Option<PathBuf> {
        if let Ok(appdata) = std::env::var("APPDATA") {
            return Some(PathBuf::from(appdata).join("Omen").join("config.toml"));
        }
        if let Ok(config) = std::env::var("XDG_CONFIG_HOME") {
            return Some(PathBuf::from(config).join("omen").join("config.toml"));
        }
        std::env::var_os("HOME")
            .map(PathBuf::from)
            .map(|home| home.join(".config").join("omen").join("config.toml"))
    }

    pub fn from_toml(text: &str) -> Result<Self, String> {
        toml::from_str(text).map_err(|error| error.to_string())
    }

    pub fn to_toml(&self) -> Result<String, String> {
        toml::to_string_pretty(self).map_err(|error| error.to_string())
    }

    pub fn load(path: &Path) -> Result<Option<Self>, String> {
        if !path.exists() {
            return Ok(None);
        }
        let text = std::fs::read_to_string(path).map_err(|error| error.to_string())?;
        Self::from_toml(&text).map(Some)
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        std::fs::write(path, self.to_toml()?).map_err(|error| error.to_string())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChooserAction {
    Next,
    Previous,
    Accept,
    Cancel,
}

#[derive(Debug, Clone)]
pub struct AppearanceChooser {
    original: Theme,
    current: Theme,
}

impl AppearanceChooser {
    pub fn new(original: Theme) -> Self {
        Self {
            original,
            current: original,
        }
    }

    pub fn current(&self) -> Theme {
        self.current
    }

    pub fn apply(&mut self, action: ChooserAction) -> Option<Theme> {
        match action {
            ChooserAction::Next => self.current = self.current.next(),
            ChooserAction::Previous => self.current = self.current.previous(),
            ChooserAction::Accept => return Some(self.current),
            ChooserAction::Cancel => {
                self.current = self.original;
                return Some(self.original);
            }
        }
        None
    }

    pub fn preview(&self, caps: &TerminalCapabilities) -> String {
        let state = PromptState::new(Path::new("~/Projects/Omen"), Some("main".into()), 0, false)
            .with_theme(self.current);
        format!(
            "{}\nTheme: {}",
            PromptRenderer::render(&state, caps),
            self.current.label()
        )
    }
}

pub fn startup_message(settings: &HumanSettings, caps: &TerminalCapabilities) -> String {
    if !settings.epigraph || !caps.is_interactive {
        return String::new();
    }
    "Omen started\nThe future has dependencies.\n".to_string()
}

/// Run the first-run appearance chooser. The terminal is restored on every
/// exit path, including Escape and input errors.
pub fn run_appearance_chooser(initial: Theme, caps: &TerminalCapabilities) -> io::Result<Theme> {
    if !caps.is_interactive {
        return Ok(initial);
    }

    use crossterm::cursor::{Hide, MoveTo, Show};
    use crossterm::event::{self, Event, KeyCode};
    use crossterm::execute;
    use crossterm::terminal::{self, Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen};

    let mut stdout = io::stdout();
    terminal::enable_raw_mode()?;
    let result = (|| -> io::Result<Theme> {
        execute!(stdout, EnterAlternateScreen, Hide)?;
        let mut chooser = AppearanceChooser::new(initial);
        loop {
            execute!(stdout, MoveTo(0, 0), Clear(ClearType::All))?;
            writeln!(stdout, "Welcome to Omen. Choose an appearance:")?;
            writeln!(stdout)?;
            writeln!(stdout, "{}", chooser.preview(caps))?;
            writeln!(stdout)?;
            writeln!(
                stdout,
                "←/↑ previous   →/↓ next   Enter accept   Esc cancel"
            )?;
            stdout.flush()?;

            if let Event::Key(key) = event::read()? {
                match key.code {
                    KeyCode::Left | KeyCode::Up => {
                        chooser.apply(ChooserAction::Previous);
                    }
                    KeyCode::Right | KeyCode::Down => {
                        chooser.apply(ChooserAction::Next);
                    }
                    KeyCode::Enter => break Ok(chooser.apply(ChooserAction::Accept).unwrap()),
                    KeyCode::Esc => break Ok(chooser.apply(ChooserAction::Cancel).unwrap()),
                    _ => {}
                }
            }
        }
    })();
    let restore_result = execute!(stdout, Show, LeaveAlternateScreen);
    let raw_mode_result = terminal::disable_raw_mode();
    let cleanup_result = restore_result.and(raw_mode_result);

    match result {
        Err(error) => Err(error),
        Ok(theme) => {
            cleanup_result?;
            Ok(theme)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chooser_cancel_restores_original_theme() {
        let mut chooser = AppearanceChooser::new(Theme::Amber);
        assert_eq!(chooser.apply(ChooserAction::Next), None);
        assert_eq!(chooser.apply(ChooserAction::Cancel), Some(Theme::Amber));
        assert_eq!(chooser.current(), Theme::Amber);
    }

    #[test]
    fn settings_round_trip_as_human_toml() {
        let settings = HumanSettings {
            theme: Theme::Ice,
            density: PromptDensity::Compact,
            epigraph: false,
        };
        let text = settings.to_toml().unwrap();
        assert!(text.contains("theme = \"ice\""));
        assert_eq!(HumanSettings::from_toml(&text).unwrap(), settings);
    }
}
