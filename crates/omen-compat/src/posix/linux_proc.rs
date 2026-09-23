//! Linux `/proc` process observer.
//!
//! Treated explicitly as Linux evidence. Never claimed on non-Linux platforms.

use crate::posix::observe::PosixProcessIdentity;

/// Parse `/proc/<pid>/stat` into identity facts. `comm` may contain spaces
/// and parentheses; parse from the last `)` onward.
pub fn read_process_identity(pid: u32) -> Option<PosixProcessIdentity> {
    let path = format!("/proc/{pid}/stat");
    let text = std::fs::read_to_string(&path).ok()?;
    let rparen = text.rfind(')')?;
    let after = text[rparen + 1..].trim_start();
    // fields after comm: state ppid pgrp session ...
    let mut it = after.split_whitespace();
    let _state = it.next()?;
    let ppid = it.next()?.parse::<u32>().ok()?;
    let pgrp = it.next()?.parse::<u32>().ok()?;
    let session_id = it.next()?.parse::<u32>().ok()?;
    Some(PosixProcessIdentity {
        pid,
        ppid: Some(ppid),
        pgrp: Some(pgrp),
        session_id: Some(session_id),
        source: "linux_proc_stat".into(),
    })
}

/// Process state letter from `/proc/<pid>/stat` (`T` = stopped, `Z` = zombie).
pub fn read_process_state_letter(pid: u32) -> Option<char> {
    let path = format!("/proc/{pid}/stat");
    let text = std::fs::read_to_string(&path).ok()?;
    let rparen = text.rfind(')')?;
    let after = text[rparen + 1..].trim_start();
    after.chars().next()
}

/// True when the pid is a zombie (state `Z`).
pub fn is_zombie(pid: u32) -> bool {
    read_process_state_letter(pid) == Some('Z')
}

/// True when the pid is stopped (`T` or `t`).
pub fn is_stopped(pid: u32) -> bool {
    matches!(read_process_state_letter(pid), Some('T') | Some('t'))
}

/// Direct children of `pid` via `/proc/<pid>/task/<pid>/children` when present.
pub fn direct_children(pid: u32) -> Vec<u32> {
    let path = format!("/proc/{pid}/task/{pid}/children");
    std::fs::read_to_string(&path)
        .ok()
        .map(|text| {
            text.split_whitespace()
                .filter_map(|s| s.parse::<u32>().ok())
                .collect()
        })
        .unwrap_or_default()
}

/// Direct children of `shell_pid` that are currently zombies.
pub fn zombie_direct_children(shell_pid: u32) -> Vec<u32> {
    direct_children(shell_pid)
        .into_iter()
        .filter(|c| is_zombie(*c))
        .collect()
}

/// Platform honesty: `/proc` zombie observation is Linux-only here.
pub fn zombie_evidence_available() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn self_identity_is_coherent() {
        let pid = std::process::id();
        let id = read_process_identity(pid).expect("self /proc identity");
        assert_eq!(id.pid, pid);
        assert!(id.ppid.is_some());
        assert!(id.pgrp.is_some());
        assert!(id.session_id.is_some());
        assert_eq!(id.source, "linux_proc_stat");
    }

    #[test]
    fn self_is_not_zombie_or_stopped() {
        let pid = std::process::id();
        assert!(!is_zombie(pid));
        assert!(!is_stopped(pid));
    }
}
