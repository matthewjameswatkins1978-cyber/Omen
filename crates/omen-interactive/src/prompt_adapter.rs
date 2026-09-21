use omen_ui::{HumanSettings, PromptRenderer, PromptState, TerminalCapabilities};
use reedline::Prompt;
use std::borrow::Cow;
use std::path::PathBuf;

pub struct OmenPrompt {
    cwd: PathBuf,
    branch: Option<String>,
    dirty_count: usize,
    has_failure: bool,
    caps: TerminalCapabilities,
    mode_indicator: Option<String>,
    settings: HumanSettings,
}

impl OmenPrompt {
    pub fn new(
        cwd: PathBuf,
        branch: Option<String>,
        dirty_count: usize,
        has_failure: bool,
        caps: TerminalCapabilities,
    ) -> Self {
        Self {
            cwd,
            branch,
            dirty_count,
            has_failure,
            caps,
            mode_indicator: None,
            settings: HumanSettings::default(),
        }
    }

    pub fn with_mode_indicator(mut self, indicator: impl Into<String>) -> Self {
        self.mode_indicator = Some(indicator.into());
        self
    }

    pub fn set_mode_indicator(&mut self, indicator: Option<String>) {
        self.mode_indicator = indicator;
    }

    pub fn update_state(
        &mut self,
        cwd: PathBuf,
        branch: Option<String>,
        dirty_count: usize,
        has_failure: bool,
    ) {
        self.cwd = cwd;
        self.branch = branch;
        self.dirty_count = dirty_count;
        self.has_failure = has_failure;
    }

    pub fn dirty_count(&self) -> usize {
        self.dirty_count
    }

    pub fn mode_indicator(&self) -> Option<&str> {
        self.mode_indicator.as_deref()
    }

    pub fn apply_settings(&mut self, settings: HumanSettings) {
        self.settings = settings;
    }
}

impl Prompt for OmenPrompt {
    fn render_prompt_left(&self) -> Cow<'_, str> {
        let mut state = PromptState::new(
            &self.cwd,
            self.branch.clone(),
            self.dirty_count,
            self.has_failure,
        );
        if let Some(ref ind) = self.mode_indicator {
            state = state.with_mode_indicator(ind.clone());
        }
        state = state
            .with_theme(self.settings.theme)
            .with_density(self.settings.density);
        let rendered = PromptRenderer::render(&state, &self.caps);
        Cow::Owned(rendered)
    }

    fn render_prompt_right(&self) -> Cow<'_, str> {
        Cow::Borrowed("")
    }

    fn render_prompt_indicator(&self, _prompt_mode: reedline::PromptEditMode) -> Cow<'_, str> {
        Cow::Borrowed("")
    }

    fn render_prompt_multiline_indicator(&self) -> Cow<'_, str> {
        Cow::Borrowed("::: ")
    }

    fn render_prompt_history_search_indicator(
        &self,
        history_search: reedline::PromptHistorySearch,
    ) -> Cow<'_, str> {
        let prefix = match history_search.status {
            reedline::PromptHistorySearchStatus::Passing => "history: ",
            reedline::PromptHistorySearchStatus::Failing => "failing history: ",
        };
        Cow::Owned(format!("({prefix}{}) ", history_search.term))
    }
}
