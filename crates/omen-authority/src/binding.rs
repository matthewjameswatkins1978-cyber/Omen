//! Exact physical execution binding: authorised semantic action A must
//! execute physical action A — nothing else.
//!
//! The Tethers COMMIT verifies the *semantic* intent (capability, version,
//! provider, argument digest). That proof alone does not name a physical
//! command, so [`crate::AuthorityIntent`] carries NO physical fields: a
//! caller can describe semantic intent but can never pair one authorised
//! semantic argument set with an arbitrary physical argv.
//!
//! Physical execution is produced by the Omen-owned trusted resolver in
//! this module ([`resolve_execution`]) from:
//!
//! - the authorised capability + version + provider identity,
//! - the authorised semantic arguments,
//! - a trusted Omen provider/installation provision ([`FixtureProvision`]).
//!
//! The resolver output ([`ExecutionBinding`]) becomes executable only
//! through [`VerifiedExecutionBinding::bind`], which checks it against the
//! verified Tethers dispatch field-by-field AND re-verifies executable
//! identity and physical scope at bind time. [`crate::PhysicalExecutor`]
//! accepts ONLY `&VerifiedExecutionBinding`, so post-COMMIT physical
//! substitution is impossible by type: there is no argv/cwd parameter left
//! to supply.
//!
//! H2 scope: the single trusted provider mapping is the `fixture.ping`
//! fixture used by the proofs. Unknown capabilities/providers refuse.
//! There is NO generic raw-exec capability: semantic capabilities always
//! resolve through this trusted mapping, never from caller-supplied argv.

use crate::AuthorityError;
use crate::admission::AuthorityIntent;
use crate::dispatch::VerifiedDispatch;
use std::path::{Path, PathBuf};

/// The Omen-known fixture triple. The ONLY capability/provider the
/// trusted resolver maps to physical execution.
pub const FIXTURE_CAPABILITY: &str = "fixture.ping";
pub const FIXTURE_CAPABILITY_VERSION: u32 = 1;
pub const FIXTURE_PROVIDER: &str = "tethers-stdio-fixture";

/// Trusted installation truth for the fixture provider: WHERE the
/// provider implementation lives and WHICH sandbox it may write.
/// This is operator provisioning (like the companion dir), supplied
/// pre-authority — never derived from, and never mixed with, the
/// authorised semantic arguments.
#[derive(Debug, Clone)]
pub struct FixtureProvision {
    /// Exact fixture executable (pinned file, hashed at resolve).
    pub exe: PathBuf,
    /// Sandbox dir: marker scope AND physical cwd. Must exist.
    pub workdir: PathBuf,
}

/// Trusted physical execution plan. Fields are private: only
/// [`resolve_execution`] (the Omen-owned resolver) can construct one,
/// so a caller can never hand-craft a binding for an arbitrary command.
/// Read-only getters expose the values for evidence and assertions.
#[derive(Debug, Clone)]
pub struct ExecutionBinding {
    capability: String,
    capability_version: u32,
    provider: String,
    argument_digest: String,
    exe: PathBuf,
    exe_sha256: String,
    argv: Vec<String>,
    cwd: PathBuf,
    /// Bounded environment projection. The fixture needs no environment:
    /// always empty, inheriting the engine's argv-only isolation policy.
    /// (No second environment model; see `omen_engine` supervision.)
    env: Vec<(String, String)>,
    timeout_ms: u64,
    marker_path: PathBuf,
}

impl ExecutionBinding {
    pub fn capability(&self) -> &str {
        &self.capability
    }
    pub fn capability_version(&self) -> u32 {
        self.capability_version
    }
    pub fn provider(&self) -> &str {
        &self.provider
    }
    pub fn argument_digest(&self) -> &str {
        &self.argument_digest
    }
    pub fn exe(&self) -> &Path {
        &self.exe
    }
    pub fn exe_sha256(&self) -> &str {
        &self.exe_sha256
    }
    pub fn argv(&self) -> &[String] {
        &self.argv
    }
    pub fn cwd(&self) -> &Path {
        &self.cwd
    }
    pub fn env(&self) -> &[(String, String)] {
        &self.env
    }
    pub fn timeout_ms(&self) -> u64 {
        self.timeout_ms
    }
    pub fn marker_path(&self) -> &Path {
        &self.marker_path
    }
}

/// The ONLY value the executor accepts. Private fields + private
/// construction (only [`VerifiedExecutionBinding::bind`]) make a forged
/// or mutated binding unrepresentable outside this module: the physical
/// command a caller sees can be read, never written.
#[derive(Debug, Clone)]
pub struct VerifiedExecutionBinding {
    execution_id: String,
    action_id: String,
    capability_name: String,
    exe: PathBuf,
    exe_sha256: String,
    argv: Vec<String>,
    cwd: PathBuf,
    env: Vec<(String, String)>,
    timeout_ms: u64,
}

impl VerifiedExecutionBinding {
    /// Bind a trusted resolver binding against a verified Tethers
    /// dispatch. Refuses (zero spawn, by construction — the executor is
    /// never reached) on ANY mismatch: capability, version, provider,
    /// semantic argument digest, argv shape, exe identity drift since
    /// resolve, or lost physical scope (cwd).
    pub fn bind(
        dispatch: &VerifiedDispatch,
        binding: &ExecutionBinding,
    ) -> Result<Self, AuthorityError> {
        let mismatch = |field: &str, got: &str, want: &str| {
            AuthorityError::Validate(format!("binding.{field}_mismatch: {got} != {want}"))
        };
        if binding.capability != dispatch.capability_name {
            return Err(mismatch(
                "capability",
                &binding.capability,
                &dispatch.capability_name,
            ));
        }
        if binding.capability_version != dispatch.capability_version {
            return Err(AuthorityError::Validate(format!(
                "binding.capability_version_mismatch: {} != {}",
                binding.capability_version, dispatch.capability_version
            )));
        }
        if binding.provider != dispatch.provider_identity {
            return Err(mismatch(
                "provider",
                &binding.provider,
                &dispatch.provider_identity,
            ));
        }
        if binding.argument_digest != dispatch.argument_digest {
            return Err(mismatch(
                "argument_digest",
                &binding.argument_digest,
                &dispatch.argument_digest,
            ));
        }
        // Structural shape: the resolver's argv IS the execution; any
        // binding whose argv does not start at the bound executable is
        // not a resolver product.
        match binding.argv.first() {
            Some(first) if Path::new(first) == binding.exe => {}
            _ => {
                return Err(AuthorityError::Validate(
                    "binding.argv_shape: argv[0] != bound executable".to_string(),
                ));
            }
        }
        // Physical scope re-verification (resolve -> bind window): the
        // cwd must still be the bound sandbox directory. Checked before
        // executable identity so a destroyed scope reports as scope loss.
        if !binding.cwd.is_dir() {
            return Err(AuthorityError::Validate(format!(
                "binding.cwd_unresolvable: {}",
                binding.cwd.display()
            )));
        }
        // Executable identity re-verification (resolve -> bind window):
        // the TARGET command, not merely the Gate binary.
        let current = sha256_file(&binding.exe)?;
        if current != binding.exe_sha256 {
            return Err(AuthorityError::Validate(format!(
                "binding.exe_identity_changed: {} records {}",
                binding.exe.display(),
                binding.exe_sha256,
            )));
        }
        Ok(Self {
            execution_id: dispatch.execution_id.clone(),
            action_id: dispatch.action_id.clone(),
            capability_name: dispatch.capability_name.clone(),
            exe: binding.exe.clone(),
            exe_sha256: binding.exe_sha256.clone(),
            argv: binding.argv.clone(),
            cwd: binding.cwd.clone(),
            env: binding.env.clone(),
            timeout_ms: binding.timeout_ms,
        })
    }

    pub fn execution_id(&self) -> &str {
        &self.execution_id
    }
    pub fn action_id(&self) -> &str {
        &self.action_id
    }
    pub fn capability_name(&self) -> &str {
        &self.capability_name
    }
    pub fn exe(&self) -> &Path {
        &self.exe
    }
    pub fn exe_sha256(&self) -> &str {
        &self.exe_sha256
    }
    pub fn argv(&self) -> &[String] {
        &self.argv
    }
    pub fn cwd(&self) -> &Path {
        &self.cwd
    }
    pub fn env(&self) -> &[(String, String)] {
        &self.env
    }
    pub fn timeout_ms(&self) -> u64 {
        self.timeout_ms
    }
}

/// Trusted Omen provider/adapter resolution: the authorised semantic
/// action determines the exact physical action. Refuses unknown
/// capabilities/providers, malformed semantic arguments, and missing
/// provisioned installation truth. Never reads caller argv (there is
/// none to read).
pub fn resolve_execution(
    intent: &AuthorityIntent,
    provision: &FixtureProvision,
) -> Result<ExecutionBinding, AuthorityError> {
    if intent.expected_capability != FIXTURE_CAPABILITY {
        return Err(AuthorityError::Validate(format!(
            "binding.unknown_capability: {}",
            intent.expected_capability
        )));
    }
    if intent.expected_capability_version != FIXTURE_CAPABILITY_VERSION {
        return Err(AuthorityError::Validate(format!(
            "binding.unknown_capability_version: {}",
            intent.expected_capability_version
        )));
    }
    if intent.expected_provider != FIXTURE_PROVIDER {
        return Err(AuthorityError::Validate(format!(
            "binding.unknown_provider: {}",
            intent.expected_provider
        )));
    }
    let obj = intent
        .expected_arguments
        .as_object()
        .ok_or_else(|| AuthorityError::Validate("binding.arguments_not_object".to_string()))?;
    let message = obj
        .get("message")
        .and_then(|v| v.as_str())
        .ok_or_else(|| AuthorityError::Validate("binding.message_not_string".to_string()))?;
    let path = obj
        .get("path")
        .and_then(|v| v.as_str())
        .ok_or_else(|| AuthorityError::Validate("binding.path_not_string".to_string()))?;
    if message.is_empty() || message.len() > 64 {
        return Err(AuthorityError::Validate(
            "binding.message_shape: non-empty, <= 64 chars".to_string(),
        ));
    }
    if !message
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(AuthorityError::Validate(
            "binding.message_shape: [A-Za-z0-9_-] only".to_string(),
        ));
    }
    if path.is_empty() || path.len() > 256 {
        return Err(AuthorityError::Validate(
            "binding.path_shape: non-empty, <= 256 chars".to_string(),
        ));
    }
    if !provision.exe.is_file() {
        return Err(AuthorityError::Validate(format!(
            "binding.exe_missing: {}",
            provision.exe.display()
        )));
    }
    if !provision.workdir.is_dir() {
        return Err(AuthorityError::Validate(format!(
            "binding.workdir_missing: {}",
            provision.workdir.display()
        )));
    }
    // Marker scope is derived, never supplied: the authorised message
    // names the marker inside the provisioned sandbox. A caller cannot
    // steer the physical write anywhere else.
    let marker_path = provision.workdir.join(format!("{message}.marker"));
    let exe = provision.exe.clone();
    let exe_sha256 = sha256_file(&exe)?;
    let argv = vec![
        exe.to_string_lossy().into_owned(),
        marker_path.to_string_lossy().into_owned(),
    ];
    let argument_digest = crate::canonical_digest(&intent.expected_arguments)?;
    Ok(ExecutionBinding {
        capability: intent.expected_capability.clone(),
        capability_version: intent.expected_capability_version,
        provider: intent.expected_provider.clone(),
        argument_digest,
        exe,
        exe_sha256,
        argv,
        cwd: provision.workdir.clone(),
        env: Vec::new(),
        timeout_ms: intent.timeout_ms,
        marker_path,
    })
}

fn sha256_file(path: &Path) -> Result<String, AuthorityError> {
    let bytes = std::fs::read(path).map_err(|e| {
        AuthorityError::Validate(format!("binding.exe_unreadable: {}: {e}", path.display()))
    })?;
    Ok(format!(
        "{:x}",
        <sha2::Sha256 as sha2::Digest>::digest(bytes)
    ))
}
