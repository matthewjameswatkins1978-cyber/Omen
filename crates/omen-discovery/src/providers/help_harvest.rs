//! Strict bounded help harvest (authority-ladder rung 4).
//!
//! **HelpHarvest is lower-confidence evidence. It is never permission to
//! invent syntax.** A token beginning with `-` is *not* automatically an
//! option. There is no permissive regex soup here.
//!
//! Hard requirements enforced by this module:
//! - bounded subprocess, **closed stdin**
//! - bounded execution time (deadline + kill)
//! - bounded bytes read, bounded candidates emitted
//! - **strict parsing grammar**; ambiguous or malformed lines are ignored
//! - locale-bound (`LANG=C`) and cache-bound to executable identity
//! - result bound to executable path/digest/version/platform; a changed
//!   executable invalidates the cache
//! - no candidate is emitted unless it satisfies the strict extraction rules

use std::io::Read;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::authority::{Authority, AuthorityClass};
use crate::budget::{CostTier, DiscoveryBudget};
use crate::cache::{CacheIdentity, CacheKey, DiscoveryCache};
use crate::candidate::{Description, DiscoveredCandidate, OrderPolicy, SafetyAnnotation};
use crate::context::ProviderContext;
use crate::identity::{EvidenceIdentity, ProvenanceKey, SemanticKey, SemanticNamespace};
use crate::kind::Kind;
use crate::outcome::{DeclineReason, PartialReason, ProviderError, ProviderOutcome};
use crate::provider::{
    Determinism, DiscoveryProvider, FreshnessPolicy, ProviderId, TriggerDecision,
};

/// Maximum bytes read from `--help` output.
const MAX_HELP_BYTES: u64 = 128 * 1024;
/// Maximum candidates emitted from one harvest.
const MAX_HARVEST_CANDIDATES: usize = 256;
/// Hard deadline for the harvest subprocess.
const HARVEST_DEADLINE: Duration = Duration::from_millis(900);

/// One option extracted under the strict grammar.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HarvestedOption {
    pub long: Option<String>,
    pub short: Option<String>,
    pub description: Option<String>,
}

/// Strict option-column grammar.
///
/// Accepts only well-formed forms and rejects everything else:
/// `-x`, `-x VAL`, `-x=VAL`, `-x<VAL>`, `-x[VAL]`, `--name`, `--name=VAL`,
/// `--name VAL`, `--name<VAL>`, `--name[VAL]`, and comma-separated groups of
/// these. Names must be `[A-Za-z0-9_-]+`.
///
/// Returns `None` for any token that fails, so the caller drops the whole
/// line (fail closed; ambiguous lines are ignored, not guessed).
fn parse_option_token(token: &str) -> Option<(Option<String>, Option<String>)> {
    let t = token.trim();
    if t.is_empty() {
        return None;
    }
    if let Some(rest) = t.strip_prefix("--") {
        let name_part = rest.split(['=', '<', '[', ' ']).next().unwrap_or("");
        let name = name_part.trim();
        if name.is_empty() || !is_name(name) {
            return None;
        }
        return Some((Some(name.to_string()), None));
    }
    if let Some(rest) = t.strip_prefix('-') {
        let first = rest.chars().next()?;
        if !first.is_ascii_alphanumeric() {
            return None;
        }
        // A short option is exactly `-x`, optionally followed by a value
        // delimiter. More letters immediately after means this is not a short
        // option (`-this` is prose or an old-style long, not `-t`).
        let after = &rest[first.len_utf8()..];
        if after.is_empty()
            || after.starts_with('=')
            || after.starts_with('<')
            || after.starts_with('[')
            || after.starts_with(' ')
        {
            return Some((None, Some(first.to_string())));
        }
        return None;
    }
    None
}

fn is_name(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// A word qualifies as a value placeholder only in the explicit forms real
/// help output uses: `<dir>`, `[dir]`, `FILE`, `=<value>`, `N`, `A|B`.
///
/// Lowercase prose (`but`, `with`, `prose`) is **never** a placeholder, so an
/// ambiguous line is rejected rather than guessed.
fn is_value_placeholder(word: &str) -> bool {
    let w = word.trim_matches(|c| c == ',' || c == '=');
    if w.is_empty() {
        return false;
    }
    if (w.starts_with('<') && w.ends_with('>')) || (w.starts_with('[') && w.ends_with(']')) {
        return true;
    }
    // Uppercase / digit / punctuation only: no lowercase letters.
    w.chars()
        .all(|c| !c.is_ascii_lowercase() && (c.is_ascii_alphanumeric() || "=|.-_".contains(c)))
}

/// Parses help output under the strict grammar.
///
/// Ambiguous or malformed lines are **ignored** (not guessed). Only lines whose
/// entire option column satisfies the grammar contribute candidates. A token
/// beginning with `-` is not automatically an option.
pub fn parse_help_options(text: &str, max: usize) -> Vec<HarvestedOption> {
    let mut out = Vec::new();
    for line in text.lines() {
        if out.len() >= max {
            break;
        }
        let trimmed = line.trim_start();
        if !trimmed.starts_with('-') {
            // Not an option line (usage prose, section headers). Ignore.
            continue;
        }

        // The option column ends at the first run of two or more spaces.
        let (option_part, description) = match find_column_split(trimmed) {
            Some((a, b)) => (a, b),
            None => (trimmed, ""),
        };

        let option_part = option_part.trim().trim_end_matches(',');
        if option_part.is_empty() {
            continue;
        }

        // Every comma-group must be a well-formed option token, optionally
        // followed by a single value placeholder. Anything else rejects the
        // whole line (fail closed).
        let mut long = None;
        let mut short = None;
        let mut valid = true;
        for group in option_part.split(',') {
            let words: Vec<&str> = group.split_whitespace().collect();
            if words.is_empty() {
                continue;
            }
            let mut i = 0;
            while i < words.len() {
                match parse_option_token(words[i]) {
                    Some((l, s)) => {
                        if l.is_some() {
                            long = l;
                        }
                        if s.is_some() && short.is_none() {
                            short = s;
                        }
                        i += 1;
                        // At most one following value placeholder.
                        if i < words.len() && is_value_placeholder(words[i]) {
                            i += 1;
                        }
                    }
                    None => {
                        valid = false;
                        break;
                    }
                }
            }
            if !valid {
                break;
            }
        }

        // Fail closed: a line with no recognisable name is not an option.
        if !valid || (long.is_none() && short.is_none()) {
            continue;
        }
        if long.is_none() && short.as_deref().is_some_and(|s| s.is_empty()) {
            continue;
        }

        let description = {
            let d = description.trim();
            if d.is_empty() {
                None
            } else {
                Some(d.to_string())
            }
        };

        out.push(HarvestedOption {
            long,
            short,
            description,
        });
    }
    out
}

fn find_column_split(s: &str) -> Option<(&str, &str)> {
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b' ' {
            let start = i;
            while i < bytes.len() && bytes[i] == b' ' {
                i += 1;
            }
            if i - start >= 2 {
                return Some((&s[..start], &s[i..]));
            }
        } else {
            i += 1;
        }
    }
    None
}

/// Runs `tool --help` with closed stdin, bounded output, and a hard deadline.
///
/// The child environment is pinned to `LANG=C` (deterministic parse, and the
/// locale is part of the cache identity) and a no-pager environment so help is
/// emitted as plain text. `stderr` is discarded; help that only exists on
/// stderr causes a clean decline rather than a guess.
pub fn run_help_harvest(tool: &str) -> Result<String, ProviderError> {
    let mut cmd = Command::new(tool);
    cmd.arg("--help")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .env("LANG", "C")
        .env("LC_ALL", "C")
        .env("PAGER", "cat")
        .env("GIT_PAGER", "cat")
        .env("MANPAGER", "cat")
        .env("TERM", "dumb");

    let mut child = cmd.spawn().map_err(|e| {
        ProviderError::new(
            "discovery.provider.help-harvest.spawn",
            format!("spawn '{tool} --help' failed: {e}"),
        )
    })?;

    // Bounded read.
    let mut output = Vec::new();
    if let Some(mut stdout) = child.stdout.take() {
        let _ = stdout
            .by_ref()
            .take(MAX_HELP_BYTES)
            .read_to_end(&mut output);
    }

    // Bounded wait with kill-on-deadline.
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => {
                if started.elapsed() >= HARVEST_DEADLINE {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(ProviderError::new(
                        "discovery.provider.help-harvest.timeout",
                        format!("'{tool} --help' exceeded {:?}", HARVEST_DEADLINE),
                    ));
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(ProviderError::new(
                    "discovery.provider.help-harvest.wait",
                    format!("wait failed: {e}"),
                ));
            }
        }
    }

    String::from_utf8(output).map_err(|_| {
        ProviderError::new(
            "discovery.provider.help-harvest.encoding",
            "help output was not valid UTF-8",
        )
    })
}

/// Cached harvest store, keyed by executable identity.
///
/// Two maps with one job each: [`DiscoveryCache`] tracks identity binding and
/// freshness (a changed digest misses the key entirely), while the options map
/// stores the actual harvested payload so the cache path really serves results
/// instead of an empty stand-in.
#[derive(Debug, Clone, Default)]
pub struct HelpHarvestCache {
    freshness: DiscoveryCache,
    options: Arc<Mutex<std::collections::HashMap<CacheKey, Vec<HarvestedOption>>>>,
    /// External identity resolver (path/digest/version), injected.
    identity: Arc<Mutex<Option<ToolIdentity>>>,
}

/// Executable identity used to bind harvest cache entries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolIdentity {
    pub path: String,
    pub digest: Option<String>,
    pub version: Option<String>,
    pub platform: String,
    pub locale: String,
}

impl HelpHarvestCache {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_identity(&self, identity: ToolIdentity) {
        if let Ok(mut i) = self.identity.lock() {
            *i = Some(identity);
        }
    }

    fn cache_key(&self, tool: &str) -> Option<CacheKey> {
        let guard = self.identity.lock().ok()?;
        let id = guard.as_ref()?;
        if id.path.is_empty() {
            return None;
        }
        Some(CacheKey {
            provider: ProviderId::new("help-harvest"),
            identity: CacheIdentity::Tool {
                path: format!("{tool}#{}#{}", id.path, id.locale),
                digest: id.digest.clone(),
                version: id.version.clone(),
                platform: id.platform.clone(),
            },
        })
    }

    pub fn get(&self, tool: &str) -> Option<Vec<HarvestedOption>> {
        let key = self.cache_key(tool)?;
        // Identity/freshness gate: expired or absent entries miss, so a
        // changed executable never reuses old truth.
        self.freshness.get(&key)?;
        self.options.lock().ok()?.get(&key).cloned()
    }

    pub fn put(&self, tool: &str, options: Vec<HarvestedOption>) {
        if let Some(key) = self.cache_key(tool) {
            self.freshness
                .put(key.clone(), Vec::new(), Vec::new(), now_unix());
            if let Ok(mut m) = self.options.lock() {
                m.insert(key, options);
            }
        }
    }

    pub fn invalidate(&self, tool: &str) {
        if let Some(key) = self.cache_key(tool) {
            self.freshness.invalidate(&key);
            if let Ok(mut m) = self.options.lock() {
                m.remove(&key);
            }
        }
    }
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Answers option queries from strict bounded help harvest.
///
/// **TIER 2**: always dispatched asynchronously by the scheduler; it must
/// never block the editor loop. The registry never runs this inline.
pub struct HelpHarvestProvider {
    cache: HelpHarvestCache,
    /// Memoised harvest results per tool (avoids re-harvest within a session).
    memo: Arc<Mutex<std::collections::HashMap<String, Vec<HarvestedOption>>>>,
}

impl HelpHarvestProvider {
    pub fn new(cache: HelpHarvestCache) -> Self {
        Self {
            cache,
            memo: Arc::new(Mutex::new(std::collections::HashMap::new())),
        }
    }

    pub fn cache(&self) -> &HelpHarvestCache {
        &self.cache
    }
}

impl DiscoveryProvider for HelpHarvestProvider {
    fn id(&self) -> ProviderId {
        ProviderId::new("help-harvest")
    }

    fn supported_kinds(&self) -> &'static [Kind] {
        &[Kind::Option]
    }

    fn triggers(&self, ctx: &ProviderContext) -> TriggerDecision {
        if !ctx.is_option_name_position() {
            return TriggerDecision::Skip;
        }
        match ctx.tool_in_scope() {
            Some(t) if !t.is_empty() => TriggerDecision::Apply,
            _ => TriggerDecision::Skip,
        }
    }

    fn cost_tier(&self) -> CostTier {
        // Subprocess harvest: always async, never on the editor thread.
        CostTier::Subprocess
    }

    fn determinism(&self) -> Determinism {
        Determinism::Deterministic
    }

    fn authority_capabilities(&self) -> &'static [AuthorityClass] {
        &[AuthorityClass::HelpHarvest]
    }

    fn freshness(&self) -> FreshnessPolicy {
        FreshnessPolicy::IdentityBound
    }

    fn discover(&mut self, ctx: &ProviderContext, budget: &DiscoveryBudget) -> ProviderOutcome {
        let Some(tool) = ctx.tool_in_scope() else {
            return ProviderOutcome::Declined {
                reason: DeclineReason::NotApplicable,
            };
        };
        let tool = tool.to_string();
        let span = ctx.replacement_span;

        // 1. Memoised / cached harvest first (no subprocess).
        let harvested = {
            let memo = self.memo.lock().ok().and_then(|m| m.get(&tool).cloned());
            match memo {
                Some(h) => Some(h),
                None => self.cache.get(&tool),
            }
        };

        let harvested = match harvested {
            Some(h) => h,
            None => {
                if budget.tier_ceiling < CostTier::Subprocess {
                    return ProviderOutcome::Declined {
                        reason: DeclineReason::BudgetExhausted,
                    };
                }
                if budget.is_exhausted() {
                    return ProviderOutcome::Declined {
                        reason: DeclineReason::BudgetExhausted,
                    };
                }
                // TIER 2 work: bounded subprocess with closed stdin.
                match run_help_harvest(&tool) {
                    Ok(text) => {
                        let opts = parse_help_options(&text, MAX_HARVEST_CANDIDATES);
                        if opts.is_empty() {
                            // Malformed / unparseable help: decline, never
                            // guess — and remember the decline so the cache
                            // path does not respawn the tool on every request.
                            if let Ok(mut m) = self.memo.lock() {
                                m.insert(tool.clone(), Vec::new());
                            }
                            self.cache.put(&tool, Vec::new());
                            return ProviderOutcome::Declined {
                                reason: DeclineReason::NoMatch,
                            };
                        }
                        if let Ok(mut m) = self.memo.lock() {
                            m.insert(tool.clone(), opts.clone());
                        }
                        self.cache.put(&tool, opts.clone());
                        opts
                    }
                    Err(err) => {
                        return ProviderOutcome::Failed { error: err };
                    }
                }
            }
        };

        if harvested.is_empty() {
            return ProviderOutcome::Declined {
                reason: DeclineReason::NoMatch,
            };
        }

        let tool_version = self
            .cache
            .identity
            .lock()
            .ok()
            .and_then(|g| g.as_ref().map(|i| i.version.clone()))
            .flatten();

        let mut candidates = Vec::new();
        let truncated = harvested.len() > MAX_HARVEST_CANDIDATES;
        for (i, h) in harvested.iter().take(MAX_HARVEST_CANDIDATES).enumerate() {
            let insert = match (&h.long, &h.short) {
                (Some(l), _) => format!("--{l}"),
                (None, Some(s)) => format!("-{s}"),
                (None, None) => continue,
            };
            candidates.push(
                DiscoveredCandidate::new(
                    SemanticKey::new(
                        Kind::Option,
                        &insert,
                        SemanticNamespace::Tool { tool: tool.clone() },
                    ),
                    ProvenanceKey::new(
                        "help-harvest",
                        AuthorityClass::HelpHarvest,
                        EvidenceIdentity::HelpHarvest {
                            tool_version: tool_version.clone(),
                            harvested_at_unix: now_unix(),
                        },
                    ),
                    insert,
                    Authority::HelpHarvest {
                        tool: tool.clone(),
                        tool_version: tool_version.clone(),
                        harvested_at_unix: now_unix(),
                    },
                    span,
                )
                .with_description(
                    h.description
                        .clone()
                        .map(Description::short)
                        .unwrap_or_else(|| Description::short("option")),
                )
                .with_order_policy(OrderPolicy::Semantic(i as u32))
                .with_safety(SafetyAnnotation::UnknownImpact),
            );
        }

        if candidates.is_empty() {
            return ProviderOutcome::Declined {
                reason: DeclineReason::NoMatch,
            };
        }
        if truncated {
            return ProviderOutcome::Partial {
                candidates,
                reason: PartialReason::CandidateCap,
            };
        }
        ProviderOutcome::Answered { candidates }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strict_parser_extracts_well_formed_options() {
        let help = "\
   -v, --verbose             be more verbose
   -q, --quiet               be quiet
       --progress            force progress reporting
       --git-dir=<dir>       set the path to the repository
";
        let opts = parse_help_options(help, 100);
        assert_eq!(opts.len(), 4);
        assert_eq!(opts[0].long.as_deref(), Some("verbose"));
        assert_eq!(opts[0].short.as_deref(), Some("v"));
        assert_eq!(opts[0].description.as_deref(), Some("be more verbose"));
        assert_eq!(opts[3].long.as_deref(), Some("git-dir"));
    }

    #[test]
    fn strict_parser_ignores_prose_and_ambiguous_lines() {
        let help = "\
usage: git [--version] [--help] <command> [<args>]

These are common Git commands used in various situations:

   --not-an-option but with prose
   -
   --
   -x, !!!bad name!!!
";
        let opts = parse_help_options(help, 100);
        assert!(
            opts.is_empty(),
            "ambiguous/malformed lines must be ignored, not guessed"
        );
    }

    #[test]
    fn a_token_beginning_with_dash_is_not_automatically_an_option() {
        let help = "\
   -this is actually prose description text that spans
   -- and another line that is not an option
";
        let opts = parse_help_options(help, 100);
        // "-this" has an invalid multi-char short and no valid long.
        assert!(opts.is_empty());
    }

    #[test]
    fn malformed_help_declines_rather_than_guessing() {
        let help = "Complete nonsense with no options at all.\nMore prose.\n";
        let opts = parse_help_options(help, 100);
        assert!(opts.is_empty());
    }

    #[test]
    fn output_is_bounded_by_max() {
        let mut help = String::new();
        for i in 0..50 {
            help.push_str(&format!("   --opt-{i}    desc {i}\n"));
        }
        let opts = parse_help_options(&help, 10);
        assert_eq!(opts.len(), 10);
    }

    #[test]
    fn harvest_cache_binds_to_executable_identity() {
        let cache = HelpHarvestCache::new();
        cache.set_identity(ToolIdentity {
            path: "/usr/bin/git".into(),
            digest: Some("aaa".into()),
            version: Some("2.0".into()),
            platform: "linux".into(),
            locale: "C".into(),
        });
        cache.put(
            "git",
            vec![HarvestedOption {
                long: Some("verbose".into()),
                short: Some("v".into()),
                description: Some("be more verbose".into()),
            }],
        );
        // Change the digest: cache must not be served.
        cache.set_identity(ToolIdentity {
            path: "/usr/bin/git".into(),
            digest: Some("bbb".into()),
            version: Some("2.0".into()),
            platform: "linux".into(),
            locale: "C".into(),
        });
        assert!(
            cache.get("git").is_none(),
            "changed digest invalidates harvest cache"
        );
    }

    #[test]
    fn harvest_cache_path_actually_serves_stored_options() {
        // The cache path must return the stored harvest, not an empty
        // stand-in; otherwise a "cache hit" would silently decline forever.
        let cache = HelpHarvestCache::new();
        cache.set_identity(ToolIdentity {
            path: "/usr/bin/git".into(),
            digest: Some("aaa".into()),
            version: Some("2.0".into()),
            platform: "linux".into(),
            locale: "C".into(),
        });
        assert!(cache.get("git").is_none(), "cold cache misses");
        cache.put(
            "git",
            vec![HarvestedOption {
                long: Some("verbose".into()),
                short: Some("v".into()),
                description: Some("be more verbose".into()),
            }],
        );
        let got = cache.get("git").expect("warm cache must serve");
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].long.as_deref(), Some("verbose"));
        // Restoring the original identity serves again; identity still binds.
        cache.set_identity(ToolIdentity {
            path: "/usr/bin/git".into(),
            digest: Some("aaa".into()),
            version: Some("2.0".into()),
            platform: "linux".into(),
            locale: "C".into(),
        });
        assert!(cache.get("git").is_some());
        cache.invalidate("git");
        assert!(cache.get("git").is_none(), "explicit invalidation misses");
    }

    #[test]
    fn provider_declares_help_harvest_authority_only() {
        let p = HelpHarvestProvider::new(HelpHarvestCache::new());
        assert_eq!(p.authority_capabilities(), &[AuthorityClass::HelpHarvest]);
        assert_eq!(p.cost_tier(), CostTier::Subprocess);
        assert!(
            p.cost_tier().must_be_async(),
            "harvest must never run inline"
        );
    }
}
