use fuzzy_matcher::FuzzyMatcher;
use fuzzy_matcher::skim::SkimMatcherV2;
use omen_core::ValidityState;
use omen_knowledge::{Database, FactRegistry};
use reedline::{Completer, CompletionResult, Hinter, Span, Suggestion};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// Cached active fact entry in the hot semantic index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CachedFact {
    pub resource_uri: String,
    pub validity: ValidityState,
}

/// Bounded hot semantic cache owned per-session in Omen 0.3.
/// This guarantees zero SQLite queries and zero filesystem scans on the keystroke-critical path.
#[derive(Debug, Clone)]
pub struct HotSemanticIndex {
    /// Bounded active facts from the Fact Registry (up to 100).
    pub active_facts: Vec<CachedFact>,
    /// Bounded immediate filesystem entries in current working directory (up to 200).
    pub workspace_entries: Vec<String>,
    /// Atlas-registered tools.
    pub known_tools: Vec<String>,
    /// Registered semantic actions.
    pub known_actions: Vec<String>,
    /// Dynamic typed references.
    pub static_refs: Vec<String>,
}

impl Default for HotSemanticIndex {
    fn default() -> Self {
        Self {
            active_facts: Vec::new(),
            workspace_entries: Vec::new(),
            known_tools: vec![
                "threadmoth".into(),
                "cargo".into(),
                "git".into(),
                "ripgrep".into(),
            ],
            known_actions: vec![
                ":status".into(),
                ":doctor".into(),
                ":tools".into(),
                ":inspect".into(),
                ":why".into(),
                ":history".into(),
                ":rerun".into(),
                ":services".into(),
                ":stop".into(),
            ],
            static_refs: vec![
                "@last".into(),
                "@last.failed".into(),
                "@last.artifact".into(),
                "@last.changed".into(),
                "@failed".into(),
                "@errors".into(),
            ],
        }
    }
}

impl HotSemanticIndex {
    /// Refreshes the hot index from the SQLite database and current working directory.
    /// Executed strictly out-of-band (e.g. before prompt display or after execution),
    /// never on keystrokes.
    pub fn refresh(&mut self, cwd: &Path, db: Option<&Database>) {
        if let Some(db_conn) = db {
            if let Ok(facts) = FactRegistry::list_active_facts(db_conn, 100) {
                self.active_facts = facts
                    .into_iter()
                    .map(|f| CachedFact {
                        resource_uri: f.resource_uri.to_string(),
                        validity: f.validity,
                    })
                    .collect();
            }
        } else {
            self.active_facts.clear();
        }

        self.workspace_entries.clear();
        if let Ok(entries) = std::fs::read_dir(cwd) {
            for entry in entries.flatten().take(200) {
                self.workspace_entries
                    .push(entry.file_name().to_string_lossy().to_string());
            }
        }
    }
}

pub struct CompletionContext {
    pub cwd: PathBuf,
    pub hot_index: HotSemanticIndex,
}

impl Default for CompletionContext {
    fn default() -> Self {
        Self {
            cwd: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            hot_index: HotSemanticIndex::default(),
        }
    }
}

pub struct OmenCompleter {
    matcher: SkimMatcherV2,
    context: Arc<Mutex<CompletionContext>>,
}

impl OmenCompleter {
    pub fn new(context: Arc<Mutex<CompletionContext>>) -> Self {
        Self {
            matcher: SkimMatcherV2::default(),
            context,
        }
    }

    pub fn context(&self) -> Arc<Mutex<CompletionContext>> {
        self.context.clone()
    }

    pub fn complete_items(&mut self, line: &str, pos: usize) -> Vec<Suggestion> {
        let (start, word) = match line[..pos].rfind(|c: char| c.is_whitespace()) {
            Some(idx) => (idx + 1, &line[idx + 1..pos]),
            None => (0, &line[..pos]),
        };

        if word.is_empty() {
            return Vec::new();
        }

        let ctx = if let Ok(c) = self.context.lock() {
            c
        } else {
            return Vec::new();
        };

        let mut candidates: Vec<(String, Option<String>, i64)> = Vec::new();

        if word.starts_with(':') {
            for action in &ctx.hot_index.known_actions {
                if let Some(score) = self.matcher.fuzzy_match(action, word) {
                    candidates.push((action.clone(), Some("semantic action".into()), score));
                }
            }
        } else if word.starts_with('@') {
            for r in &ctx.hot_index.static_refs {
                if let Some(score) = self.matcher.fuzzy_match(r, word) {
                    let mut score_adj = score;
                    if r == "@failed" || r == "@errors" {
                        score_adj += 10;
                    }
                    candidates.push((r.clone(), Some("typed reference".into()), score_adj));
                }
            }
            for f in &ctx.hot_index.active_facts {
                let ref_str = format!("@{}", f.resource_uri);
                if let Some(score) = self.matcher.fuzzy_match(&ref_str, word) {
                    let mut final_score = score;
                    if f.validity == ValidityState::Dirty {
                        final_score += 50; // elevate dirty facts
                    }
                    candidates.push((
                        ref_str,
                        Some(format!("fact ({:?})", f.validity)),
                        final_score,
                    ));
                }
            }
        } else if start == 0 {
            for tool in &ctx.hot_index.known_tools {
                if let Some(score) = self.matcher.fuzzy_match(tool, word) {
                    candidates.push((tool.clone(), Some("known tool".into()), score));
                }
            }
            for action in &ctx.hot_index.known_actions {
                if let Some(score) = self.matcher.fuzzy_match(action, word) {
                    candidates.push((action.clone(), Some("semantic action".into()), score));
                }
            }
            for name in &ctx.hot_index.workspace_entries {
                if let Some(score) = self.matcher.fuzzy_match(name, word) {
                    candidates.push((name.clone(), Some("workspace entry".into()), score));
                }
            }
        } else {
            let first_word = line.split_whitespace().next().unwrap_or("");
            match first_word {
                "cargo" => {
                    for sub in &["build", "test", "check", "run", "clean", "clippy", "fmt"] {
                        if let Some(score) = self.matcher.fuzzy_match(sub, word) {
                            candidates.push((
                                (*sub).into(),
                                Some("cargo subcommand".into()),
                                score,
                            ));
                        }
                    }
                }
                "git" => {
                    for sub in &[
                        "status", "diff", "log", "commit", "add", "checkout", "branch", "switch",
                    ] {
                        if let Some(score) = self.matcher.fuzzy_match(sub, word) {
                            candidates.push(((*sub).into(), Some("git subcommand".into()), score));
                        }
                    }
                }
                "threadmoth" => {
                    for sub in &["mutate", "plan", "apply-plan", "replace-exact", "doctor"] {
                        if let Some(score) = self.matcher.fuzzy_match(sub, word) {
                            candidates.push((
                                (*sub).into(),
                                Some("threadmoth command".into()),
                                score,
                            ));
                        }
                    }
                }
                _ => {}
            }

            for r in &ctx.hot_index.static_refs {
                if let Some(score) = self.matcher.fuzzy_match(r, word) {
                    candidates.push((r.clone(), Some("typed reference".into()), score));
                }
            }

            for name in &ctx.hot_index.workspace_entries {
                if let Some(score) = self.matcher.fuzzy_match(name, word) {
                    candidates.push((name.clone(), Some("path".into()), score));
                }
            }
        }

        candidates.sort_by_key(|a| std::cmp::Reverse(a.2));

        candidates
            .into_iter()
            .take(15)
            .map(|(val, desc, _)| Suggestion {
                value: val,
                description: desc,
                extra: None,
                span: Span { start, end: pos },
                append_whitespace: true,
                display_override: None,
                match_indices: None,
                style: None,
            })
            .collect()
    }
}

impl Completer for OmenCompleter {
    fn complete(&mut self, line: &str, pos: usize) -> CompletionResult {
        let items = self.complete_items(line, pos);
        CompletionResult::fresh(items)
    }
}

pub struct OmenHinter {
    completer: Arc<Mutex<OmenCompleter>>,
}

impl OmenHinter {
    pub fn new(completer: Arc<Mutex<OmenCompleter>>) -> Self {
        Self { completer }
    }
}

impl Hinter for OmenHinter {
    fn handle(
        &mut self,
        line: &str,
        pos: usize,
        _history: &dyn reedline::History,
        _use_ansi: bool,
        _cwd: &str,
    ) -> String {
        if line.is_empty() || pos < line.len() {
            return String::new();
        }

        if let Ok(mut comp) = self.completer.lock() {
            let suggestions = comp.complete_items(line, pos);
            if let Some(first) = suggestions.first() {
                let span_len = first.span.end - first.span.start;
                if first.value.len() > span_len {
                    return first.value[span_len..].to_string();
                }
            }
        }

        String::new()
    }

    fn complete_hint(&self) -> String {
        String::new()
    }

    fn next_hint_token(&self) -> String {
        String::new()
    }
}
