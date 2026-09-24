# Windows ConPTY / console-control invariants (M0-W)

Stable machine-readable IDs. Exact Rust enum spelling may follow existing
style; serialized identity must match these IDs or clearly preserve meaning.

Windows console control events are **not** POSIX signals (D2-019). Raw exit
DWORDs are **not** cause proof (D2-021). Compat's independent ConPTY harness
and Omen engine ConPTY are separate authorities (D2-020).

| ID | Meaning | Evidence expectation |
| --- | --- | --- |
| `WINDOWS_INTERACTIVE_CHILD_CONSOLE_HANDLES_VALID` | Interactive child under Omen has usable Windows console semantics on required standard handles. | Strong: fixture `GetFileType` = CHAR on stdin/stdout **and** `GetConsoleMode` succeeds. Non-null alone is not enough. |
| `WINDOWS_INTERACTIVE_CHILD_HAS_TARGETABLE_CONTROL_GROUP` | A distinct Windows control group exists and can be targeted as such. | Strong only from **behavioral** targeted control-event receipt. `CREATE_NEW_PROCESS_GROUP` configuration alone is not targeting proof → INCONCLUSIVE. |
| `WINDOWS_TERMINAL_CTRL_C_REACHES_INTERACTIVE_CHILD` | User-like Ctrl-C through outer Compat ConPTY input is independently observed by the interactive child (`CTRL_C_EVENT` receipt marker). | Strong: fixture handler records `CTRL_C_EVENT` after ConPTY input `0x03`. Input written alone is not receipt. |
| `WINDOWS_SHELL_SURVIVES_INTERACTIVE_CHILD_CTRL_C` | Omen shell remains alive (and returns to a usable prompt) after child Ctrl-C. | Strong: shell process still alive; independent of child receipt. |
| `WINDOWS_CONSOLE_MODE_RESTORED_AFTER_ABNORMAL_CHILD_EXIT` | Verified child console-mode mutation is restored to the shell's observed pre-child snapshot after abnormal exit. | Strong: pre observed + dirty verified + post consequential modes == pre. Mutation not observed → INCONCLUSIVE (not PASS). |
| `WINDOWS_OUTER_CONPTY_RESIZE_VISIBLE_TO_INTERACTIVE_CHILD` | Outer Compat ConPTY `ResizePseudoConsole` is observed by an interactive Omen child. | Strong: fixture `GetConsoleScreenBufferInfo` shows new dimensions after harness resize. API success alone is not enough. |
| `WINDOWS_ENGINE_CONPTY_CLIENT_CONSOLE_VALID` | Omen engine `NativePtyHandle` creates a genuine usable ConPTY client console. | Strong: fixture under product ConPTY reports console-capable std handles. |
| `WINDOWS_ENGINE_CONPTY_RESIZE_VISIBLE` | Product `NativePtyHandle::resize` is observed inside its client process. | Strong: fixture observes new dimensions after product `ResizePseudoConsole`. |
| `WINDOWS_ENGINE_JOB_CONTAINS_DESCENDANTS` | A descendant created by the ConPTY client is included in effective product containment. | Strong: parent+descendant live before product terminate; both gone after. Source inspection alone is never PASS. |
| `WINDOWS_ENGINE_TERMINATE_KILLS_DESCENDANTS` | Product PTY termination removes observed parent + descendant within bound. | Strong: both live before terminate; both gone within harness bound after. |
| `WINDOWS_ENGINE_EXIT_STATUS_PRESERVES_RAW_BITS` | Observed product exit value preserves the controlled Windows exit-status bit pattern. | Strong: raw DWORD equality (or i32 bit-pattern reinterpretation). Cause context is a **separate** fact — never inferred from the integer alone. |
| `WINDOWS_ENGINE_SHUTDOWN_BOUNDED` | Normal close / terminate complete within declared harness bounds. | Strong: elapsed ≤ declared bound. |

Optional (only if evidence supports cleanly):

| ID | Meaning | Evidence expectation |
| --- | --- | --- |
| `WINDOWS_TARGETED_CTRL_BREAK_REACHES_CONTROL_GROUP` | Programmatic `CTRL_BREAK_EVENT` reaches a proven distinct control group. | Strong: targeted group receipt. Do not add solely because the API name can be spelled. |

## Evidence grades

- Direct Win32 observation (`GetConsoleMode`, `GetConsoleScreenBufferInfo`, `GetExitCodeProcess`, Toolhelp liveness): STRONG
- Fixture Ctrl receipt marker after user-like ConPTY input: STRONG for delivery
- Control-event API success without observed receipt: PARTIAL (not delivery)
- Source-code expectation / prompt text (`\O/` readiness only): WITNESS / PARTIAL
- Configured `CREATE_NEW_PROCESS_GROUP` without behavioral targeting: PARTIAL configuration fact only
- Raw exit integer without cause context: bits only — cause UNAVAILABLE/INCONCLUSIVE

## Tiers

- TIER WINDOWS CONTROL — Compat-owned ConPTY calibration without Omen
- TIER WINDOWS OMEN INTERACTIVE — real Omen shell under Compat ConPTY
- TIER WINDOWS ENGINE CONPTY — Omen `NativePtyHandle` as product under test

Judgment lives in `omen-compat`. Fixtures report facts only.
