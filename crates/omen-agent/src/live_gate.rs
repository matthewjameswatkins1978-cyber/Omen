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

/// Returns true when this process was admitted for live egress.
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
/// packet is built. `Ok` admits; `Err` carries [`LIVE_REFUSED_MESSAGE`].
pub(crate) fn check() -> Result<(), String> {
    if is_admitted() {
        Ok(())
    } else {
        Err(LIVE_REFUSED_MESSAGE.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gate_denies_by_default_with_exact_signal() {
        // Fail-safe reset first: deterministic even if the ambient
        // environment carries OMEN_LIVE_TESTS=1. This asserts the
        // hermetic default every ordinary test process inherits.
        deny_for_test();
        assert!(!is_admitted());
        assert_eq!(
            check().unwrap_err(),
            LIVE_REFUSED_MESSAGE,
            "refusal must carry the exact fail-closed signal"
        );
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
