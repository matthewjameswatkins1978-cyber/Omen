use crate::core::{CommandSpec, OutputBounds, StdinSpec};
use crate::failure::{ReplayDescriptor, ReplayFidelity};
use crate::runner::{RunOutcome, run};
use std::path::PathBuf;
use thiserror::Error;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum ReplayError {
    #[error("only a mechanically validated Exact fixture record can be replayed")]
    NotExact,
    #[error("Exact replay record fields are internally inconsistent")]
    InconsistentRecord,
}

/// Reconstruct the one execution shape permitted by Exact fixture records:
/// explicit program/argv, inherited cwd, cleared environment, and closed stdin.
pub fn exact_fixture_command(descriptor: &ReplayDescriptor) -> Result<CommandSpec, ReplayError> {
    if descriptor.fidelity() != ReplayFidelity::Exact {
        return Err(ReplayError::NotExact);
    }
    if descriptor.argv_count() != descriptor.argv().len()
        || descriptor.cwd_policy() != "inherit"
        || descriptor.env_policy_kind() != "clear"
        || !descriptor.env_keys().is_empty()
        || descriptor.stdin_mode() != "closed"
        || descriptor.stdin_byte_length().is_some()
    {
        return Err(ReplayError::InconsistentRecord);
    }

    Ok(CommandSpec::new(PathBuf::from(descriptor.program()))
        .args(descriptor.argv().iter().cloned())
        .stdin(StdinSpec::Closed)
        .deadline(descriptor.deadline()))
}

/// Execute a validated fixture replay through the same bounded runner.
/// Redacted/Partial records cannot execute.
pub async fn replay_exact_fixture(
    descriptor: &ReplayDescriptor,
    bounds: &OutputBounds,
) -> Result<RunOutcome, ReplayError> {
    let spec = exact_fixture_command(descriptor)?;
    Ok(run(&spec, bounds).await)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{Deadline, EnvPolicy};
    use crate::invariant::InvariantId;

    #[test]
    fn exact_fixture_reconstructs_only_the_sealed_command_shape() {
        let original = CommandSpec::new("fixture").args(["--exit-code", "7"]);
        let descriptor = ReplayDescriptor::try_exact_fixture(
            &original,
            "exit-code",
            vec![InvariantId::ExitCausePreserved],
            vec!["--exit-code".into(), "7".into()],
        )
        .unwrap();

        let replay = exact_fixture_command(&descriptor).unwrap();
        assert_eq!(replay.program, original.program);
        assert_eq!(replay.argv, original.argv);
        assert_eq!(replay.cwd, None);
        assert_eq!(replay.env, EnvPolicy::Clear);
        assert_eq!(replay.stdin, StdinSpec::Closed);
        assert_eq!(replay.deadline, Deadline::default());
    }

    #[test]
    fn redacted_or_partial_records_cannot_be_executed() {
        let spec = CommandSpec::new("fixture").arg("--secret-value");
        let redacted = ReplayDescriptor::redacted(&spec, "unknown", Vec::new());
        assert_eq!(exact_fixture_command(&redacted), Err(ReplayError::NotExact));
    }
}
