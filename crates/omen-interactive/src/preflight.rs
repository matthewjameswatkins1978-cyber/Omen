#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlastSeverity {
    Low,
    Medium,
    High,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlastRadiusReport {
    pub command: String,
    pub severity: BlastSeverity,
    pub summary: String,
    pub targets: Vec<String>,
}

pub struct BlastPreflight;

impl BlastPreflight {
    /// Evaluates whether a command has high/destructive blast radius.
    pub fn assess(argv: &[String]) -> Option<BlastRadiusReport> {
        if argv.is_empty() {
            return None;
        }

        let cmd = &argv[0];
        match cmd.as_str() {
            "rm" => {
                let has_recursive = argv.iter().any(|a| a.starts_with('-') && a.contains('r'));
                let has_force = argv.iter().any(|a| a.starts_with('-') && a.contains('f'));
                let severity = if has_recursive || has_force {
                    BlastSeverity::High
                } else {
                    BlastSeverity::Medium
                };
                Some(BlastRadiusReport {
                    command: argv.join(" "),
                    severity,
                    summary: "File deletion from disk".into(),
                    targets: argv[1..].to_vec(),
                })
            }
            "cargo" => {
                if argv.get(1).map(|s| s.as_str()) == Some("clean") {
                    Some(BlastRadiusReport {
                        command: argv.join(" "),
                        severity: BlastSeverity::Medium,
                        summary: "Purging Cargo target build artifacts".into(),
                        targets: vec!["target/".into()],
                    })
                } else {
                    None
                }
            }
            "git" => {
                if let Some(sub) = argv.get(1) {
                    if sub == "reset" && argv.iter().any(|a| a == "--hard") {
                        Some(BlastRadiusReport {
                            command: argv.join(" "),
                            severity: BlastSeverity::High,
                            summary: "Hard reset: discards uncommitted changes in working tree"
                                .into(),
                            targets: vec!["working tree".into()],
                        })
                    } else if sub == "clean" && argv.iter().any(|a| a.contains('f')) {
                        Some(BlastRadiusReport {
                            command: argv.join(" "),
                            severity: BlastSeverity::High,
                            summary: "Removing untracked files from working directory".into(),
                            targets: vec!["untracked files".into()],
                        })
                    } else {
                        None
                    }
                } else {
                    None
                }
            }
            "threadmoth" => {
                if let Some(sub) = argv.get(1) {
                    if sub == "mutate" || sub == "apply-plan" {
                        Some(BlastRadiusReport {
                            command: argv.join(" "),
                            severity: BlastSeverity::Medium,
                            summary: "Structural mutation of workspace files via ThreadMoth".into(),
                            targets: argv[2..].to_vec(),
                        })
                    } else {
                        None
                    }
                } else {
                    None
                }
            }
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PasteReview {
    pub line_count: usize,
    pub lines: Vec<String>,
}

pub struct PasteGuard;

impl PasteGuard {
    /// Intercepts multiline paste buffers before individual execution.
    pub fn inspect(buffer: &str) -> Option<PasteReview> {
        let lines: Vec<String> = buffer
            .lines()
            .map(|l| l.trim().to_string())
            .filter(|l| !l.is_empty())
            .collect();

        if lines.len() > 1 {
            Some(PasteReview {
                line_count: lines.len(),
                lines,
            })
        } else {
            None
        }
    }
}
