# 8. Running things

Execution is where Omen tries hardest to be boring.

That is a compliment.

## argv before shell strings

When Omen controls an execution contract, it prefers explicit argv values over constructing a shell command string.

This removes one layer of quoting and interpretation.

## Closed stdin by default

Automated/background work should not sit forever waiting for invisible input.

Closed stdin is a useful default for non-interactive execution.

Programs that genuinely require terminal ownership follow the PTY path.

## Time is bounded

Nothing important is allowed to wait forever.

Process startup, execution, IPC, provider initialization, shutdown and test harnesses need deadlines or cancellation paths.

A timeout should return control and evidence.

## Output is bounded

A build can produce tens of thousands of lines.

Omen does not need to push all of them through the human or model surface.

The useful shape is a bounded summary plus artifact reference.

Full output remains addressable through CAS.

## Interactive programs

Some invocations genuinely need a terminal, including editors, interactive REPLs, SSH and full-screen terminal programs.

Omen 0.6 added real PTY support and attachable/resumable physical sessions.

The important distinction is invocation-specific.

    python script.py

is not the same terminal problem as:

    python

## Backends and assurance

Different platforms can enforce different constraints.

Omen reports that difference instead of manufacturing one universal green shield.

Examples include ENFORCED, MEDIATED, OBSERVED, BEST_EFFORT and UNSUPPORTED.

## Services

Long-running work should become a named process/service resource rather than shell-job archaeology.

A stable proc:// identity is easier for a human and agent to discuss than "job two from whichever terminal launched it."
