use crate::color::ColorRoles;
use omen_core::ValidityState;
use omen_knowledge::{ExecutionRecord, FactProvenance};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagnosticLevel {
    /// Level 0: Minimal (one-line summary)
    Level0,
    /// Level 1: Compact (failure context, primary error lines, affected resources)
    Level1,
    /// Level 2: Detailed (full causal breakdown, provenance dependencies, artifacts)
    Level2,
    /// Level 3: Machine / Full (raw JSON / full structured inspection)
    Level3,
}

pub struct DiagnosticRenderer;

impl DiagnosticRenderer {
    pub fn render_execution(
        record: &ExecutionRecord,
        level: DiagnosticLevel,
        roles: &ColorRoles,
    ) -> String {
        let is_ok = record.exit_code == Some(0);
        let status_symbol = if is_ok {
            roles.success.paint("✓").to_string()
        } else {
            roles.error.paint("✗").to_string()
        };

        match level {
            DiagnosticLevel::Level0 => {
                let code_str = match record.exit_code {
                    Some(c) => format!("exit {c}"),
                    None => "running".into(),
                };
                let dur_str = format!("{}ms", record.duration_ms.unwrap_or(0));
                format!(
                    "{} {} ({}, {})",
                    status_symbol,
                    roles.normal.paint(&record.command),
                    roles.subtle.paint(&code_str),
                    roles.subtle.paint(&dur_str)
                )
            }
            DiagnosticLevel::Level1 => {
                let mut out = String::new();
                let code_str = match record.exit_code {
                    Some(c) => format!("exit code {c}"),
                    None => "running".into(),
                };
                out.push_str(&format!(
                    "{} Command: {}\n",
                    status_symbol,
                    roles.normal.paint(&record.command)
                ));
                out.push_str(&format!(
                    "  Outcome: {}\n",
                    if is_ok {
                        roles.success.paint("SUCCESS").to_string()
                    } else {
                        roles.error.paint(&code_str).to_string()
                    }
                ));
                if let Some(ref err) = record.stderr_artifact {
                    out.push_str(&format!("  Errors: {}\n", roles.reference.paint(err)));
                }
                out
            }
            DiagnosticLevel::Level2 => {
                let mut out = String::new();
                out.push_str(&format!(
                    "--- Execution Diagnostic: {} ---\n",
                    record.execution_id.as_str()
                ));
                out.push_str(&format!("  Session:   {}\n", record.session_id.as_str()));
                out.push_str(&format!("  Command:   {}\n", record.command));
                out.push_str(&format!("  Exit Code: {:?}\n", record.exit_code));
                out.push_str(&format!(
                    "  Duration:  {} ms\n",
                    record.duration_ms.unwrap_or(0)
                ));
                if let Some(ref stdout) = record.stdout_artifact {
                    out.push_str(&format!("  Stdout CAS: {}\n", stdout));
                }
                if let Some(ref stderr) = record.stderr_artifact {
                    out.push_str(&format!("  Stderr CAS: {}\n", stderr));
                }
                out.push_str(&format!("  Recorded:  {}\n", record.created_at));
                out
            }
            DiagnosticLevel::Level3 => {
                serde_json::to_string_pretty(record).unwrap_or_else(|_| format!("{record:?}"))
            }
        }
    }

    pub fn render_fact_provenance(
        prov: &FactProvenance,
        level: DiagnosticLevel,
        roles: &ColorRoles,
    ) -> String {
        let is_current = prov.fact.validity == ValidityState::Current;
        let validity_str = format!("{:?}", prov.fact.validity).to_uppercase();

        let validity_badge = if is_current {
            roles.success.paint(&validity_str).to_string()
        } else {
            roles.warning.paint(&validity_str).to_string()
        };

        match level {
            DiagnosticLevel::Level0 => {
                format!(
                    "{} {} = {}",
                    validity_badge,
                    roles.reference.paint(prov.fact.resource_uri.as_str()),
                    roles.normal.paint(&prov.fact.value)
                )
            }
            DiagnosticLevel::Level1 | DiagnosticLevel::Level2 => {
                let mut out = String::new();
                out.push_str(&format!(
                    "Fact:       {}\n",
                    roles.reference.paint(prov.fact.resource_uri.as_str())
                ));
                out.push_str(&format!("Value:      {}\n", prov.fact.value));
                out.push_str(&format!("Validity:   {}\n", validity_badge));
                out.push_str(&format!("Assurance:  {:?}\n", prov.fact.assurance));
                out.push_str(&format!("Producer:   {}\n", prov.fact.producer));
                out.push_str(&format!("Witness:    {:?}\n", prov.fact.witness));

                if !prov.dependencies.is_empty() {
                    out.push_str("Dependencies:\n");
                    for dep in &prov.dependencies {
                        let state = if dep.is_dirty {
                            roles.warning.paint("DIRTY").to_string()
                        } else {
                            roles.success.paint("OK").to_string()
                        };
                        out.push_str(&format!(
                            "  - {} [{}]: recorded={}, current={}\n",
                            dep.generation_name,
                            state,
                            dep.recorded_generation,
                            dep.current_generation
                        ));
                    }
                }

                if !prov.artifacts.is_empty() {
                    out.push_str("Artifacts:\n");
                    for art in &prov.artifacts {
                        out.push_str(&format!("  - {}\n", art.as_str()));
                    }
                }
                out
            }
            DiagnosticLevel::Level3 => {
                serde_json::to_string_pretty(prov).unwrap_or_else(|_| format!("{prov:?}"))
            }
        }
    }
}
