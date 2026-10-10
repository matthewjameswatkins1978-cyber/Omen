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

/// Test-only latch reset toward deny. This can only make a test process
/// MORE hermetic, never less: it clears admission. Gate tests call it
/// first so they are deterministic even if the ambient environment
/// carries `OMEN_LIVE_TESTS=1`. There is deliberately no test-only
/// admit: the only admission path in tests is the explicit live gate.
#[cfg(test)]
pub fn deny_for_test() {
    ADMITTED.store(false, Ordering::SeqCst);
}

/// Choke-point check run by the production HTTPS transport before any
/// packet is built. Denial first: an inherited test-isolation flag
/// refuses even an admitted process. Then admission. `Ok` admits; `Err`
/// carries the exact refusal signal.
pub(crate) fn check() -> Result<(), String> {
    if hermetic_denial_active() {
        return Err(HERMETIC_REFUSED_MESSAGE.to_string());
    }
    if is_admitted() {
        Ok(())
    } else {
        Err(LIVE_REFUSED_MESSAGE.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Serializes tests that mutate process env. Denial itself is
    /// monotonic and idempotent (nothing in tests clears the latch),
    /// so the lock only keeps save/restore pairs deterministic.
    static ENV_MUTEX: Mutex<()> = Mutex::new(());

    #[test]
    fn gate_denies_by_default_with_exact_signal() {
        let _lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        // Deterministic even if the ambient environment carries either
        // flag (cargo config sets OMEN_HERMETIC_TESTS=1 for test runs).
        let prev_hermetic = std::env::var(HERMETIC_TESTS_ENV_VAR).ok();
        unsafe { std::env::remove_var(HERMETIC_TESTS_ENV_VAR) };
        deny_for_test();
        assert!(!is_admitted());
        assert_eq!(
            check().unwrap_err(),
            LIVE_REFUSED_MESSAGE,
            "refusal must carry the exact fail-closed signal"
        );
        unsafe {
            match prev_hermetic {
                Some(v) => std::env::set_var(HERMETIC_TESTS_ENV_VAR, v),
                None => std::env::remove_var(HERMETIC_TESTS_ENV_VAR),
            }
        }
    }

    #[test]
    fn inherited_denial_refuses_without_admission() {
        // Serial guard: this test mutates process env. Denial is
        // monotonic and idempotent, and nothing in tests clears the latch,
        // so parallel tests can only observe MORE refusal, never less.
        // Still, the guard keeps the env restore deterministic.
        let _lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let prev = std::env::var(HERMETIC_TESTS_ENV_VAR).ok();
        unsafe { std::env::set_var(HERMETIC_TESTS_ENV_VAR, "1") };
        deny_for_test();
        assert!(hermetic_denial_active());
        let err = check().unwrap_err();
        assert!(
            err.contains("live network refused") && err.contains(HERMETIC_TESTS_ENV_VAR),
            "denial refusal must name the inherited flag, got: {err}"
        );
        unsafe {
            match prev {
                Some(v) => std::env::set_var(HERMETIC_TESTS_ENV_VAR, v),
                None => std::env::remove_var(HERMETIC_TESTS_ENV_VAR),
            }
        }
    }

    #[test]
    fn explicit_admission_without_env_flag_panics() {
        deny_for_test();
        if std::env::var(LIVE_TESTS_ENV_VAR).as_deref() == Ok("1") {
            admit_for_explicit_live_test();
            assert!(is_admitted(), "explicit opt-in plus flag admits");
            deny_for_test();
            assert!(!is_admitted(), "hermetic default restored");
            return;
        }
        let caught = std::panic::catch_unwind(admit_for_explicit_live_test);
        assert!(caught.is_err(), "credential/config alone must not admit");
        assert!(!is_admitted(), "failed admission leaves the latch denied");
    }
}
