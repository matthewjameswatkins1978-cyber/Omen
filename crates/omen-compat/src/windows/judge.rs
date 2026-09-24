//! Invariant judgments from Windows observations.
//!
//! OBSERVATION ≠ INVARIANT ≠ RESULT. Windows console control events are not
//! POSIX signals (D2-019). Raw exit bits are not cause proof (D2-021).

use crate::core::EvidenceGrade;
use crate::invariant::{InvariantId, InvariantResult};
use crate::windows::observe::{
    WindowsConsoleModeSnapshot, WindowsCtrlReceipt, WindowsExitObservation, WindowsReport,
    WindowsStdHandleObservation,
};
use std::time::Duration;

/// Judge: interactive child has usable Windows console semantics on std handles.
pub fn judge_interactive_child_console_handles(report: &WindowsReport) -> InvariantResult {
    let inv = InvariantId::WindowsInteractiveChildConsoleHandlesValid;
    let obs = report.std_handle_observation();
    if obs.console_handles_usable() {
        InvariantResult::pass(
            inv,
            EvidenceGrade::Strong,
            format!(
                "stdin/stdout non-null and console-capable (stdin={:?} stdout={:?}); \
                 modes in={:?} out={:?}",
                report.stdin_file_type,
                report.stdout_file_type,
                report.stdin_console_mode,
                report.stdout_console_mode
            ),
        )
        .with_observations(vec![obs.observation()])
    } else if report.stdin_non_null && report.stdout_non_null {
        InvariantResult::inconclusive(
            inv,
            EvidenceGrade::Partial,
            format!(
                "handles non-null but console capability not proven \
                 (stdin={:?} stdout={:?})",
                report.stdin_file_type, report.stdout_file_type
            ),
        )
        .with_observations(vec![obs.observation()])
    } else {
        InvariantResult::fail(
            inv,
            EvidenceGrade::Partial,
            format!(
                "missing required std handles (stdin_null={} stdout_null={})",
                report.stdin_non_null, report.stdout_non_null
            ),
        )
        .with_observations(vec![obs.observation()])
    }
}

/// Judge: interactive child has a separately targetable Windows control group.
///
/// PASS only from behavioural control-event evidence (not creation flags alone).
/// Without that evidence: INCONCLUSIVE / UNAVAILABLE — never invented.
pub fn judge_targetable_control_group(
    targeted_receipt: Option<&WindowsCtrlReceipt>,
    creation_group_evidence: Option<bool>,
) -> InvariantResult {
    let inv = InvariantId::WindowsInteractiveChildHasTargetableControlGroup;
    match targeted_receipt {
        Some(WindowsCtrlReceipt::Observed { kind, source }) => InvariantResult::pass(
            inv,
            EvidenceGrade::Strong,
            format!("targeted control receipt observed ({kind}) via {source}"),
        ),
        Some(WindowsCtrlReceipt::NotObserved) => InvariantResult::inconclusive(
            inv,
            EvidenceGrade::Partial,
            format!(
                "no targeted control receipt; creation-group evidence={creation_group_evidence:?} \
                 is configuration, not targeting proof"
            ),
        ),
        Some(WindowsCtrlReceipt::Unavailable { reason }) => InvariantResult::unavailable(
            inv,
            format!("targeted control receipt unavailable: {reason}"),
        ),
        None => {
            let configured = creation_group_evidence.unwrap_or(false);
            InvariantResult::inconclusive(
                inv,
                EvidenceGrade::Partial,
                format!(
                    "no behavioural targeting evidence; CREATE_NEW_PROCESS_GROUP-like \
                     configuration observed={configured} (configured ≠ observed targeting)"
                ),
            )
        }
    }
}

/// Judge: user-like Ctrl-C through outer ConPTY reached the interactive child.
pub fn judge_terminal_ctrl_c_reaches_child(receipt: &WindowsCtrlReceipt) -> InvariantResult {
    let inv = InvariantId::WindowsTerminalCtrlCReachesInteractiveChild;
    match receipt {
        WindowsCtrlReceipt::Observed { kind, source } if *kind == "CTRL_C_EVENT" => {
            InvariantResult::pass(
                inv,
                EvidenceGrade::Strong,
                format!("fixture observed CTRL_C_EVENT via {source}"),
            )
        }
        WindowsCtrlReceipt::Observed { kind, source } => InvariantResult::fail(
            inv,
            EvidenceGrade::Partial,
            format!("expected CTRL_C_EVENT, observed {kind} via {source}"),
        ),
        WindowsCtrlReceipt::NotObserved => InvariantResult::inconclusive(
            inv,
            EvidenceGrade::Partial,
            "Ctrl-C input injected; CTRL_C_EVENT receipt not observed within bound",
        ),
        WindowsCtrlReceipt::Unavailable { reason } => {
            InvariantResult::unavailable(inv, format!("Ctrl-C receipt unavailable: {reason}"))
        }
    }
}

/// Judge: Omen shell survives interactive-child Ctrl-C (independent of receipt).
pub fn judge_shell_survives_child_ctrl_c(shell_alive: bool, shell_pid: u32) -> InvariantResult {
    let inv = InvariantId::WindowsShellSurvivesInteractiveChildCtrlC;
    if shell_alive {
        InvariantResult::pass(
            inv,
            EvidenceGrade::Strong,
            format!("shell pid={shell_pid} alive after child Ctrl-C"),
        )
    } else {
        InvariantResult::fail(
            inv,
            EvidenceGrade::Strong,
            format!("shell pid={shell_pid} not alive after child Ctrl-C"),
        )
    }
}

/// Judge: verified console-mode mutation restored to pre-child snapshot.
pub fn judge_console_mode_restored(
    pre: &WindowsConsoleModeSnapshot,
    dirty_verified: bool,
    post: &WindowsConsoleModeSnapshot,
) -> InvariantResult {
    let inv = InvariantId::WindowsConsoleModeRestoredAfterAbnormalChildExit;
    if !dirty_verified {
        return InvariantResult::inconclusive(
            inv,
            EvidenceGrade::Partial,
            "child console-mode mutation not verified; restoration claim blocked",
        );
    }
    if !pre.available || !post.available {
        return InvariantResult::unavailable(
            inv,
            format!(
                "console mode snapshot unavailable (pre={} post={})",
                pre.available, post.available
            ),
        );
    }
    if pre.input_consequential() == post.input_consequential()
        && pre.output_consequential() == post.output_consequential()
    {
        InvariantResult::pass(
            inv,
            EvidenceGrade::Strong,
            format!(
                "consequential modes restored (pre in={:?} out={:?})",
                pre.input_consequential(),
                pre.output_consequential()
            ),
        )
        .with_observations(vec![pre.observation(), post.observation()])
    } else {
        InvariantResult::fail(
            inv,
            EvidenceGrade::Strong,
            format!(
                "mode mismatch pre in={:?} out={:?} post in={:?} out={:?}",
                pre.input_consequential(),
                pre.output_consequential(),
                post.input_consequential(),
                post.output_consequential()
            ),
        )
        .with_observations(vec![pre.observation(), post.observation()])
    }
}

/// Judge: outer ConPTY resize visible to interactive child.
pub fn judge_outer_resize_visible(
    before: (u16, u16),
    after: (u16, u16),
    child_observed: bool,
) -> InvariantResult {
    let inv = InvariantId::WindowsOuterConPtyResizeVisibleToInteractiveChild;
    let size_changed = before != after;
    if size_changed && child_observed {
        InvariantResult::pass(
            inv,
            EvidenceGrade::Strong,
            format!("outer resize {before:?} -> {after:?} observed by fixture"),
        )
    } else if size_changed {
        InvariantResult::fail(
            inv,
            EvidenceGrade::Partial,
            format!("outer resize {before:?} -> {after:?} not observed by fixture"),
        )
    } else {
        InvariantResult::inconclusive(
            inv,
            EvidenceGrade::Partial,
            "outer resize did not change observed dimensions",
        )
    }
}

/// Judge: product engine ConPTY client has usable console semantics.
pub fn judge_engine_client_console(report: &WindowsReport) -> InvariantResult {
    let inv = InvariantId::WindowsEngineConPtyClientConsoleValid;
    judge_interactive_child_console_handles_inner(inv, report)
}

fn judge_interactive_child_console_handles_inner(
    inv: InvariantId,
    report: &WindowsReport,
) -> InvariantResult {
    let obs = report.std_handle_observation();
    if obs.console_handles_usable() {
        InvariantResult::pass(
            inv,
            EvidenceGrade::Strong,
            format!(
                "engine client console handles usable (stdin={:?} stdout={:?})",
                report.stdin_file_type, report.stdout_file_type
            ),
        )
        .with_observations(vec![obs.observation()])
    } else if report.stdin_non_null && report.stdout_non_null {
        InvariantResult::inconclusive(
            inv,
            EvidenceGrade::Partial,
            format!(
                "handles present but console capability unproven (stdin={:?} stdout={:?})",
                report.stdin_file_type, report.stdout_file_type
            ),
        )
        .with_observations(vec![obs.observation()])
    } else {
        InvariantResult::fail(
            inv,
            EvidenceGrade::Partial,
            "engine client missing required console-capable std handles",
        )
        .with_observations(vec![obs.observation()])
    }
}

/// Judge: product ResizePseudoConsole observed inside client.
pub fn judge_engine_resize_visible(
    before: (u16, u16),
    after: (u16, u16),
    child_observed: bool,
) -> InvariantResult {
    let inv = InvariantId::WindowsEngineConPtyResizeVisible;
    let size_changed = before != after;
    if size_changed && child_observed {
        InvariantResult::pass(
            inv,
            EvidenceGrade::Strong,
            format!("product resize {before:?} -> {after:?} observed by fixture"),
        )
    } else if size_changed {
        InvariantResult::fail(
            inv,
            EvidenceGrade::Partial,
            format!("product resize {before:?} -> {after:?} not observed by fixture"),
        )
    } else {
        InvariantResult::inconclusive(
            inv,
            EvidenceGrade::Partial,
            "product resize did not change observed dimensions",
        )
    }
}

/// Judge: product Job Object contains descendants (both live then both gone).
pub fn judge_engine_job_contains(
    parent_alive_before: bool,
    descendant_alive_before: bool,
    parent_alive_after: bool,
    descendant_alive_after: bool,
) -> InvariantResult {
    let inv = InvariantId::WindowsEngineJobContainsDescendants;
    if parent_alive_before
        && descendant_alive_before
        && !parent_alive_after
        && !descendant_alive_after
    {
        InvariantResult::pass(
            inv,
            EvidenceGrade::Strong,
            "parent+descendant live before terminate; both gone after product terminate",
        )
    } else if !descendant_alive_before {
        InvariantResult::inconclusive(
            inv,
            EvidenceGrade::Partial,
            "descendant not observed alive before terminate; containment unproven",
        )
    } else if descendant_alive_after {
        InvariantResult::fail(
            inv,
            EvidenceGrade::Strong,
            "descendant still alive after product terminate (containment ineffective)",
        )
    } else {
        InvariantResult::inconclusive(
            inv,
            EvidenceGrade::Partial,
            format!(
                "unexpected liveness pattern before=({parent_alive_before},{descendant_alive_before}) \
                 after=({parent_alive_after},{descendant_alive_after})"
            ),
        )
    }
}

/// Judge: product terminate removes observed parent + descendant within bound.
pub fn judge_engine_terminate_kills(
    both_live_before: bool,
    parent_gone: bool,
    descendant_gone: bool,
) -> InvariantResult {
    let inv = InvariantId::WindowsEngineTerminateKillsDescendants;
    if both_live_before && parent_gone && descendant_gone {
        InvariantResult::pass(
            inv,
            EvidenceGrade::Strong,
            "product terminate removed parent and descendant within bound",
        )
    } else if !both_live_before {
        InvariantResult::inconclusive(
            inv,
            EvidenceGrade::Partial,
            "parent+descendant not both live before terminate",
        )
    } else {
        InvariantResult::fail(
            inv,
            EvidenceGrade::Strong,
            format!("after terminate parent_gone={parent_gone} descendant_gone={descendant_gone}"),
        )
    }
}

/// Judge: product exit preserves controlled raw DWORD bits (not cause proof).
pub fn judge_engine_exit_preserves_raw_bits(
    observed: &WindowsExitObservation,
    expected: u32,
) -> InvariantResult {
    let inv = InvariantId::WindowsEngineExitStatusPreservesRawBits;
    if observed.preserves_bits(expected) {
        InvariantResult::pass(
            inv,
            EvidenceGrade::Strong,
            format!(
                "raw status preserves bits {expected:#x} (raw={:?})",
                observed.raw_status
            ),
        )
        .with_observations(vec![observed.observation()])
    } else {
        InvariantResult::fail(
            inv,
            EvidenceGrade::Partial,
            format!(
                "expected raw {expected:#x}, observed raw={:?} product_code={:?} \
                 (cause context is separate: {})",
                observed.raw_status,
                observed.product_code,
                observed.cause_context.stable_id()
            ),
        )
        .with_observations(vec![observed.observation()])
    }
}

/// Judge: engine shutdown completed within declared bound.
pub fn judge_engine_shutdown_bounded(elapsed: Duration, limit: Duration) -> InvariantResult {
    let inv = InvariantId::WindowsEngineShutdownBounded;
    if elapsed <= limit {
        InvariantResult::pass(
            inv,
            EvidenceGrade::Strong,
            format!(
                "shutdown elapsed {}ms within {}ms",
                elapsed.as_millis(),
                limit.as_millis()
            ),
        )
    } else {
        InvariantResult::fail(
            inv,
            EvidenceGrade::Strong,
            format!(
                "shutdown elapsed {}ms exceeds bound {}ms",
                elapsed.as_millis(),
                limit.as_millis()
            ),
        )
    }
}

/// Judge helper re-export used by tests for std-handle observation construction.
pub fn std_handle_observation_from_report(report: &WindowsReport) -> WindowsStdHandleObservation {
    report.std_handle_observation()
}
