//! Live-network egress gate: hermetic tests by default, live only by
//! deliberate opt-in.
//!
//! The only production HTTPS transport for commercial model APIs
//! ([`crate::openai_responses::UreqHttpPost`], shared by the Luna and
//! Sonnet providers) refuses to emit unless this process was explicitly
//! admitted. Production entry points call [`admit_production`] once at
//! startup; ordinary unit, integration and conformance tests never admit,
//! so even a fully-configured provider holding a real ambient credential
//! (process environment or OS keyring) fails closed with a typed refusal
//! instead of spending money. A leak becomes a loud test failure, never
//! a silent live request.
//!
//! Deliberate live validation calls [`admit_for_explicit_live_test`],
//! which additionally requires `OMEN_LIVE_TESTS=1` in the process
//! environment: a credential alone is never permission to bill.
//!
//! Privacy: refusal messages name the gate and the remedy only. No
//! secrets, prompts, or URLs with query material flow through here.

use std::sync::atomic::{AtomicBool, Ordering};

/// Opt-in flag that must accompany [`admit_for_explicit_live_test`].
pub const LIVE_TESTS_ENV_VAR: &str = "OMEN_LIVE_TESTS";

/// Inherited test-isolation flag. When present with value `"1"`, live
/// egress is refused even in an admitted process. The variable inherits
/// across process spawn, so test-spawned `omen` and MCP child processes
/// stay denied with zero per-test configuration. Production (started
/// outside the test harness) never carries it.
pub const HERMETIC_TESTS_ENV_VAR: &str = "OMEN_HERMETIC_TESTS";

/// Refusal text surfaced when test isolation denies an admitted process.
/// Carries the same `live network refused` prefix so existing refusal
/// assertions hold; names the inherited flag and the outside-tests remedy.
pub const HERMETIC_REFUSED_MESSAGE: &str = "live network refused: test isolation active (OMEN_HERMETIC_TESTS=1, inherited by child processes); unset it outside test runs to allow production egress";

/// Refusal text surfaced when unadmitted code reaches the live transport.
/// Kept as a constant so tests can assert the exact fail-closed signal.
pub const LIVE_REFUSED_MESSAGE: &str = "live network refused: process not admitted for commercial AI egress (hermetic test default; production admits at startup, explicit live tests admit via OMEN_LIVE_TESTS=1)";

/// Process-wide admission latch. Deny by default; set once, never cleared.
static ADMITTED: AtomicBool = AtomicBool::new(false);

/// Admit this process for commercial AI egress. Called once by production
/// entry points at startup (the `omen` CLI). Never called by ordinary
/// tests. Idempotent.
pub fn admit_production() {
    ADMITTED.store(true, Ordering::SeqCst);
}

/// Admit this process for one deliberate live validation run. Requires
/// `OMEN_LIVE_TESTS=1` in the environment AND this explicit call: a
/// credential alone is never permission. Panics otherwise so a missing
/// opt-in fails loudly instead of silently billing or silently skipping.
pub fn admit_for_explicit_live_test() {
    match std::env::var(LIVE_TESTS_ENV_VAR).as_deref() {
        Ok("1") => ADMITTED.store(true, Ordering::SeqCst),
        _ => panic!(
            "live gate: refusing to admit without deliberate opt-in \
             (set {LIVE_TESTS_ENV_VAR}=1 AND call admit_for_explicit_live_test)"
        ),
    }
}

/// Returns true when inherited test isolation denies live egress.
/// Denial takes precedence over [`admit_production`]: a spawned `omen`
/// or MCP child inside a test run stays denied even though its `main`
/// admits the process.
pub fn hermetic_denial_active() -> bool {
    std::env::var(HERMETIC_TESTS_ENV_VAR).as_deref() == Ok("1")
}

/// Returns true when this process was admitted for live egress.
/// Admission never overrides [`hermetic_denial_active`]; see [`check`].
pub fn is_admitted() -> bool {
    ADMITTED.load(Ordering::SeqCst)
}

/// Pure gate decision over already-read inputs: denial first, then
/// admission. Kept pure so the full truth table is provable without
/// touching process env or the admission latch (no unprotected windows
/// in test processes). [`check`] reads the live inputs and delegates.
fn decide(denied: bool, admitted: bool) -> Result<(), &'static str> {
    if denied {
        return Err(HERMETIC_REFUSED_MESSAGE);
    }
    if admitted {
        Ok(())
    } else {
        Err(LIVE_REFUSED_MESSAGE)
    }
}

/// Choke-point check run by the production HTTPS transport before any
/// packet is built. Denial first: an inherited test-isolation flag
/// refuses even an admitted process. Then admission. `Ok` admits; `Err`
/// carries the exact refusal signal.
pub(crate) fn check() -> Result<(), String> {
    decide(hermetic_denial_active(), is_admitted()).map_err(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Serializes the tests that write process env. Writes are always
    /// toward MORE protection (setting the denial flag, or removing the
    /// live opt-in flag); no test removes the denial flag or touches
    /// the admission latch, so no unprotected window can exist.
    static ENV_MUTEX: Mutex<()> = Mutex::new(());

    #[test]
    fn decision_truth_table_is_pure() {
        // No env reads, no latch, no lock: pure state transitions.
        assert_eq!(decide(false, false), Err(LIVE_REFUSED_MESSAGE));
        assert_eq!(decide(false, true), Ok(()));
        assert_eq!(decide(true, false), Err(HERMETIC_REFUSED_MESSAGE));
        // Denial takes precedence over admission.
        assert_eq!(decide(true, true), Err(HERMETIC_REFUSED_MESSAGE));
    }

    #[test]
    fn inherited_denial_signal_names_flag() {
        let _lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let prev = std::env::var(HERMETIC_TESTS_ENV_VAR).ok();
        unsafe { std::env::set_var(HERMETIC_TESTS_ENV_VAR, "1") };
        assert!(hermetic_denial_active());
        // Latch untouched by any test: a fresh test process starts
        // unadmitted, and denial decides first regardless.
        assert_eq!(check().unwrap_err(), HERMETIC_REFUSED_MESSAGE);
        unsafe {
            match prev {
                Some(v) => std::env::set_var(HERMETIC_TESTS_ENV_VAR, v),
                None => std::env::remove_var(HERMETIC_TESTS_ENV_VAR),
            }
        }
    }

    #[test]
    fn explicit_admission_requires_flag() {
        let _lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        // Removing the LIVE opt-in flag only reduces admission; it can
        // never weaken hermetic denial (separate flag, separate latch).
        let prev = std::env::var(LIVE_TESTS_ENV_VAR).ok();
        unsafe { std::env::remove_var(LIVE_TESTS_ENV_VAR) };
        let caught = std::panic::catch_unwind(admit_for_explicit_live_test);
        assert!(caught.is_err(), "credential/config alone must not admit");
        assert!(!is_admitted(), "failed admission leaves the latch denied");
        unsafe {
            match prev {
                Some(v) => std::env::set_var(LIVE_TESTS_ENV_VAR, v),
                None => std::env::remove_var(LIVE_TESTS_ENV_VAR),
            }
        }
    }
}
