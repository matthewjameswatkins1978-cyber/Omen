//! Provider credential storage: environment first, OS keyring second.
//!
//! Precedence is deliberate and documented:
//! 1. Process environment (`OPENAI_API_KEY`, `ANTHROPIC_API_KEY`) — explicit,
//!    CI-friendly, always wins. Preserves the pre-keyring behavior exactly.
//! 2. OS credential store via the `keyring` crate (Windows Credential
//!    Manager, macOS Keychain, Linux Secret Service / keyutils) under
//!    service `omen-ai`, account = registry provider id. Used only by the
//!    interactive first-run/later setup flows; never written implicitly.
//!
//! Where no OS backend exists (headless Linux without D-Bus, WSL without a
//! secret service), keyring operations fail with
//! [`CredentialError::Unavailable`] and the chain treats the store as
//! absent — never fatal, never noisy at startup.
//!
//! Secrets hygiene: values are held only in the provider that needs them
//! (existing per-transport redaction applies unchanged). Descriptors and
//! diagnostics carry source LABELS (`environment:…`, `os-keyring:…`),
//! never values.

use std::collections::HashMap;
use std::sync::{Mutex, RwLock};
use thiserror::Error;

/// OS keyring service name for all Omen provider credentials.
pub const KEYRING_SERVICE: &str = "omen-ai";

/// Registry provider id for the Luna preset.
pub const LUNA_PROVIDER_ID: &str = "openai-luna";
/// Registry provider id for the Sonnet preset.
pub const SONNET_PROVIDER_ID: &str = "anthropic-sonnet";

/// Environment variable carrying the Luna credential.
pub const LUNA_ENV_VAR: &str = "OPENAI_API_KEY";
/// Environment variable carrying the Sonnet credential.
pub const SONNET_ENV_VAR: &str = "ANTHROPIC_API_KEY";

/// Credential failures: missing/unavailable are normal discovery outcomes,
/// never panics. Values never appear in these messages.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum CredentialError {
    #[error("no credential stored for '{provider}'")]
    NotFound { provider: String },
    #[error("credential store unavailable for '{provider}': {detail}")]
    Unavailable { provider: String, detail: String },
    #[error("credential store refused the write for '{provider}': {detail}")]
    StoreFailed { provider: String, detail: String },
}

/// A credential source. Reads are total (absence is `Ok(None)` or
/// `NotFound`); only the OS/user flows write.
pub trait CredentialStore: Send + Sync {
    /// Read a credential. `Ok(None)` / `Err(NotFound)` both mean absent;
    /// `Err(Unavailable)` means the backend itself is missing.
    fn get(&self, provider_id: &str) -> Result<Option<String>, CredentialError>;
    /// Persist a credential. Read-only stores refuse.
    fn set(&self, provider_id: &str, secret: &str) -> Result<(), CredentialError>;
    /// Source label for descriptors/diagnostics (no values).
    fn source_label(&self, provider_id: &str) -> String;
}

/// Process-environment store (read-only). One env var per provider.
pub struct EnvCredentialStore;

fn env_var_for(provider_id: &str) -> Option<&'static str> {
    match provider_id {
        LUNA_PROVIDER_ID => Some(LUNA_ENV_VAR),
        SONNET_PROVIDER_ID => Some(SONNET_ENV_VAR),
        _ => None,
    }
}

impl CredentialStore for EnvCredentialStore {
    fn get(&self, provider_id: &str) -> Result<Option<String>, CredentialError> {
        let Some(var) = env_var_for(provider_id) else {
            return Ok(None);
        };
        match std::env::var(var) {
            Ok(k) if !k.trim().is_empty() => Ok(Some(k)),
            _ => Ok(None),
        }
    }

    fn set(&self, provider_id: &str, _secret: &str) -> Result<(), CredentialError> {
        Err(CredentialError::StoreFailed {
            provider: provider_id.to_string(),
            detail: "environment store is read-only; export the variable instead".into(),
        })
    }

    fn source_label(&self, provider_id: &str) -> String {
        match env_var_for(provider_id) {
            Some(var) => format!("environment:{var}"),
            None => "environment:unknown".to_string(),
        }
    }
}

/// OS credential store via the `keyring` crate.
pub struct KeyringCredentialStore;

impl KeyringCredentialStore {
    fn entry(&self, provider_id: &str) -> Result<keyring::Entry, CredentialError> {
        keyring::Entry::new(KEYRING_SERVICE, provider_id).map_err(|e| {
            CredentialError::Unavailable {
                provider: provider_id.to_string(),
                detail: e.to_string(),
            }
        })
    }
}

impl CredentialStore for KeyringCredentialStore {
    fn get(&self, provider_id: &str) -> Result<Option<String>, CredentialError> {
        let entry = self.entry(provider_id)?;
        match entry.get_password() {
            Ok(secret) if !secret.trim().is_empty() => Ok(Some(secret)),
            Ok(_) => Ok(None),
            Err(keyring::Error::NoEntry) => Ok(None),
            // Platform backends report absence many ways (D-Bus missing,
            // locked store, unsupported platform): all mean "no usable
            // backend here", never a startup fatality.
            Err(e) => Err(CredentialError::Unavailable {
                provider: provider_id.to_string(),
                detail: e.to_string(),
            }),
        }
    }

    fn set(&self, provider_id: &str, secret: &str) -> Result<(), CredentialError> {
        let entry = self.entry(provider_id)?;
        entry
            .set_password(secret)
            .map_err(|e| CredentialError::StoreFailed {
                provider: provider_id.to_string(),
                detail: e.to_string(),
            })
    }

    fn source_label(&self, provider_id: &str) -> String {
        format!("os-keyring:{KEYRING_SERVICE}/{provider_id}")
    }
}

/// In-memory store for tests. Never touches the OS or the environment.
/// Cheaply cloneable: clones share the same backing map.
#[derive(Debug, Default, Clone)]
pub struct MemoryCredentialStore {
    inner: std::sync::Arc<Mutex<HashMap<String, String>>>,
}

impl MemoryCredentialStore {
    pub fn new() -> Self {
        Self::default()
    }
}

impl CredentialStore for MemoryCredentialStore {
    fn get(&self, provider_id: &str) -> Result<Option<String>, CredentialError> {
        Ok(self.inner.lock().unwrap().get(provider_id).cloned())
    }

    fn set(&self, provider_id: &str, secret: &str) -> Result<(), CredentialError> {
        self.inner
            .lock()
            .unwrap()
            .insert(provider_id.to_string(), secret.to_string());
        Ok(())
    }

    fn source_label(&self, provider_id: &str) -> String {
        format!("memory:{provider_id}")
    }
}

/// Ordered credential chain: first hit wins. Canonical order is
/// environment, then OS keyring.
pub struct CredentialChain {
    stores: Vec<Box<dyn CredentialStore>>,
}

impl CredentialChain {
    pub fn new(stores: Vec<Box<dyn CredentialStore>>) -> Self {
        Self { stores }
    }

    /// Canonical production chain: environment, then OS keyring.
    pub fn standard() -> Self {
        Self::new(vec![
            Box::new(EnvCredentialStore),
            Box::new(KeyringCredentialStore),
        ])
    }

    /// First available credential plus the source label that provided it.
    /// Backend outages are skipped; total absence is `Ok(None)`.
    pub fn get_with_source(
        &self,
        provider_id: &str,
    ) -> Result<Option<(String, String)>, CredentialError> {
        for store in &self.stores {
            match store.get(provider_id) {
                Ok(Some(secret)) => {
                    return Ok(Some((secret, store.source_label(provider_id))));
                }
                Ok(None) => {}
                Err(CredentialError::NotFound { .. })
                | Err(CredentialError::Unavailable { .. }) => continue,
                Err(e) => return Err(e),
            }
        }
        Ok(None)
    }

    /// First available credential, label discarded.
    pub fn get(&self, provider_id: &str) -> Result<Option<String>, CredentialError> {
        Ok(self.get_with_source(provider_id)?.map(|(secret, _)| secret))
    }
}

/// Outcome of one interactive provider-key setup run. Pure logic over a
/// caller-supplied secret reader so TTY handling stays with the caller
/// and tests supply canned answers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeySetupOutcome {
    /// Key stored (in the given store); confirmation may name the source.
    Stored { source: String },
    /// User declined or supplied nothing: fallback preserved, no state.
    Skipped,
    /// Store refused the write.
    Failed { reason: String },
}

/// Minimal shape check: non-empty after trim. Validity itself is runtime
/// truth discovered at use time (AuthenticationRequired), never claimed
/// here — and per policy no network validation runs without explicit
/// consent, which this flow never requests.
pub fn accept_key_shape(secret: &str) -> bool {
    secret.trim().len() >= 8
}

/// Run the first-run/later setup dialogue logic:
/// - `wants_setup`: user's answer to "configure now?" (false/None = skip).
/// - `read_secret`: called once when setup proceeds; `None`/empty = skip.
/// - stores into `store` on acceptance.
pub fn run_key_setup(
    provider_id: &str,
    wants_setup: bool,
    read_secret: impl FnOnce() -> Option<String>,
    store: &dyn CredentialStore,
) -> KeySetupOutcome {
    if !wants_setup {
        return KeySetupOutcome::Skipped;
    }
    let secret = read_secret()
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    if !accept_key_shape(&secret) {
        return KeySetupOutcome::Skipped;
    }
    match store.set(provider_id, &secret) {
        Ok(()) => KeySetupOutcome::Stored {
            source: store.source_label(provider_id),
        },
        Err(e) => KeySetupOutcome::Failed {
            reason: e.to_string(),
        },
    }
}

/// Terminal I/O for the setup offer, supplied by the caller (first-run
/// session, `omen setup`, tests). Keeps TTY handling out of this module.
pub struct SetupIo<A, R, S>
where
    A: Fn(&str) -> bool,
    R: Fn() -> Option<String>,
    S: Fn(&str),
{
    pub ask_yes_no: A,
    pub read_secret: R,
    pub say: S,
}

/// Offer Luna key setup once: no-op when a credential already resolves
/// through `chain` (never re-asks). Otherwise asks, reads one masked
/// secret, and stores it in `store`. Every path speaks its outcome;
/// setup itself never fails the caller.
pub fn offer_luna_setup_with<A, R, S>(
    io: &SetupIo<A, R, S>,
    store: &dyn CredentialStore,
    chain: &CredentialChain,
) where
    A: Fn(&str) -> bool,
    R: Fn() -> Option<String>,
    S: Fn(&str),
{
    if chain.get(LUNA_PROVIDER_ID).ok().flatten().is_some() {
        return;
    }
    (io.say)("GPT-6 Luna is the default reasoning provider when a key is configured.");
    let outcome = run_key_setup(
        LUNA_PROVIDER_ID,
        (io.ask_yes_no)("Configure Luna now? The key stays in your OS credential store"),
        || (io.read_secret)(),
        store,
    );
    match outcome {
        KeySetupOutcome::Stored { source } => {
            (io.say)(&format!(
                "[ok] Luna key stored ({source}). Verify with any `?` question."
            ));
        }
        KeySetupOutcome::Skipped => {
            (io.say)("Skipped: deterministic fallback stays active. Later route: `omen setup`.");
        }
        KeySetupOutcome::Failed { reason } => {
            (io.say)(&format!(
                "[warn] Luna key not stored ({reason}). Deterministic fallback stays active."
            ));
        }
    }
}

/// Production offer: standard chain (environment, then OS keyring).
pub fn offer_luna_setup<A, R, S>(io: &SetupIo<A, R, S>, store: &dyn CredentialStore)
where
    A: Fn(&str) -> bool,
    R: Fn() -> Option<String>,
    S: Fn(&str),
{
    offer_luna_setup_with(io, store, &CredentialChain::standard());
}

/// Process-wide last-known-good source labels (for descriptors built
/// without a chain handy). Empty by default; the setup flows record here.
static SOURCE_HINTS: std::sync::LazyLock<RwLock<HashMap<String, String>>> =
    std::sync::LazyLock::new(|| RwLock::new(HashMap::new()));

pub fn record_source_hint(provider_id: &str, source: &str) {
    SOURCE_HINTS
        .write()
        .unwrap()
        .insert(provider_id.to_string(), source.to_string());
}

pub fn source_hint(provider_id: &str) -> Option<String> {
    SOURCE_HINTS.read().unwrap().get(provider_id).cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    static ENV_SERIAL: Mutex<()> = Mutex::new(());

    struct EnvRestore {
        key: &'static str,
        previous: Option<String>,
    }
    impl Drop for EnvRestore {
        fn drop(&mut self) {
            unsafe {
                match &self.previous {
                    Some(v) => std::env::set_var(self.key, v),
                    None => std::env::remove_var(self.key),
                }
            }
        }
    }

    #[test]
    fn chain_tries_stores_in_order() {
        let memory = MemoryCredentialStore::new();
        memory.set("test-provider-x", "memory-key").unwrap();
        let chain = CredentialChain::new(vec![Box::new(EnvCredentialStore), Box::new(memory)]);
        // "test-provider-x" has no env mapping, so the env store yields
        // None deterministically and the second store wins.
        let got = chain.get("test-provider-x").unwrap();
        assert_eq!(got.as_deref(), Some("memory-key"));
    }

    #[test]
    fn chain_prefers_environment_when_set() {
        let _serial = ENV_SERIAL.lock().unwrap();
        let previous = std::env::var(LUNA_ENV_VAR).ok();
        let _restore = EnvRestore {
            key: LUNA_ENV_VAR,
            previous,
        };
        unsafe { std::env::set_var(LUNA_ENV_VAR, "env-key") };
        let memory = MemoryCredentialStore::new();
        memory.set(LUNA_PROVIDER_ID, "memory-key").unwrap();
        let chain = CredentialChain::new(vec![Box::new(EnvCredentialStore), Box::new(memory)]);
        let got = chain.get_with_source(LUNA_PROVIDER_ID).unwrap().unwrap();
        assert_eq!(got.0, "env-key");
        assert_eq!(got.1, "environment:OPENAI_API_KEY");
    }

    #[test]
    fn memory_store_roundtrips_without_os_or_env() {
        let store = MemoryCredentialStore::new();
        assert!(store.get("x").unwrap().is_none());
        store.set("x", "s3cret-value").unwrap();
        assert_eq!(store.get("x").unwrap().as_deref(), Some("s3cret-value"));
        assert!(store.source_label("x").starts_with("memory:"));
    }

    #[test]
    fn env_store_is_read_only_and_labelled() {
        let store = EnvCredentialStore;
        assert!(store.set("openai-luna", "k").is_err());
        assert_eq!(
            store.source_label("openai-luna"),
            "environment:OPENAI_API_KEY"
        );
        assert_eq!(
            store.source_label("anthropic-sonnet"),
            "environment:ANTHROPIC_API_KEY"
        );
    }

    #[test]
    fn setup_flow_shapes() {
        let store = MemoryCredentialStore::new();
        // Decline: no read, no write.
        let outcome = run_key_setup("openai-luna", false, || panic!("must not prompt"), &store);
        assert_eq!(outcome, KeySetupOutcome::Skipped);
        assert!(store.get("openai-luna").unwrap().is_none());
        // Empty paste: skip, nothing stored.
        let outcome = run_key_setup("openai-luna", true, || Some("   ".into()), &store);
        assert_eq!(outcome, KeySetupOutcome::Skipped);
        // Accept: stored under the provider id.
        let outcome = run_key_setup(
            "openai-luna",
            true,
            || Some("sk-test-key-value".into()),
            &store,
        );
        assert!(matches!(outcome, KeySetupOutcome::Stored { .. }));
        assert_eq!(
            store.get("openai-luna").unwrap().as_deref(),
            Some("sk-test-key-value")
        );
    }

    #[test]
    fn offer_flow_speaks_every_outcome() {
        use std::sync::Mutex;
        let said: Mutex<Vec<String>> = Mutex::new(Vec::new());
        // Memory-only chain over a shared store: hermetic regardless of
        // ambient env/keychain, and the second offer goes silent.
        let shared = MemoryCredentialStore::new();
        let chain = CredentialChain::new(vec![Box::new(shared.clone())]);
        let io = SetupIo {
            ask_yes_no: |_| true,
            read_secret: || Some("sk-offer-key-value".into()),
            say: |line| said.lock().unwrap().push(line.to_string()),
        };
        offer_luna_setup_with(&io, &shared, &chain);
        assert_eq!(
            shared.get(LUNA_PROVIDER_ID).unwrap().as_deref(),
            Some("sk-offer-key-value")
        );
        let transcript = said.lock().unwrap().join("\n");
        assert!(transcript.contains("[ok] Luna key stored"));
        // Second offer: credential now resolves through the chain, so the
        // flow stays silent (never re-asks).
        said.lock().unwrap().clear();
        let io = SetupIo {
            ask_yes_no: |_| panic!("must not prompt when configured"),
            read_secret: || panic!("must not read when configured"),
            say: |line| said.lock().unwrap().push(line.to_string()),
        };
        offer_luna_setup_with(&io, &shared, &chain);
        assert!(said.lock().unwrap().is_empty());
    }

    #[test]
    fn key_shape_gate() {
        assert!(!accept_key_shape(""));
        assert!(!accept_key_shape("   "));
        assert!(!accept_key_shape("short"));
        assert!(accept_key_shape("sk-test-key-value"));
    }
}
