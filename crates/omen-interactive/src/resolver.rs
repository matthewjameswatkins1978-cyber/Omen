use crate::grammar::TypedReference;
use omen_core::{CoreError, InteractiveSessionId};
use omen_knowledge::{Database, ExecutionHistory};

pub struct ReferenceResolver;

impl ReferenceResolver {
    pub fn resolve(
        handle: &str,
        session_id: &InteractiveSessionId,
        db: &Database,
    ) -> Result<String, CoreError> {
        let typed_ref = match TypedReference::parse(handle) {
            Some(r) => r,
            None => return Ok(handle.to_string()),
        };

        match typed_ref {
            TypedReference::Last => {
                if let Some(last) = ExecutionHistory::get_last_execution(db, session_id)? {
                    Ok(last.command)
                } else {
                    Err(CoreError::NotFound(
                        "No previous execution in this session".into(),
                    ))
                }
            }
            TypedReference::LastFailed | TypedReference::Failed => {
                if let Some(failed) = ExecutionHistory::get_last_failed_execution(db, session_id)? {
                    Ok(failed.command)
                } else {
                    Err(CoreError::NotFound(
                        "No failed execution found in this session".into(),
                    ))
                }
            }
            TypedReference::LastArtifact => {
                if let Some(last) = ExecutionHistory::get_last_execution(db, session_id)? {
                    if let Some(art) = last.stdout_artifact {
                        return Ok(art);
                    }
                    let arts = ExecutionHistory::get_execution_artifacts(db, &last.execution_id)?;
                    if let Some(first) = arts.first() {
                        return Ok(first.as_str().to_string());
                    }
                    Err(CoreError::NotFound(
                        "No artifact produced by last execution".into(),
                    ))
                } else {
                    Err(CoreError::NotFound(
                        "No previous execution in this session".into(),
                    ))
                }
            }
            TypedReference::LastChanged => {
                if let Some(last) = ExecutionHistory::get_last_execution(db, session_id)? {
                    let res = ExecutionHistory::get_execution_resources(db, &last.execution_id)?;
                    let changed: Vec<String> = res
                        .into_iter()
                        .filter(|(_, role)| role == "modified" || role == "created")
                        .map(|(uri, _)| uri.as_str().to_string())
                        .collect();
                    if changed.is_empty() {
                        Ok(String::new())
                    } else {
                        Ok(changed.join(" "))
                    }
                } else {
                    Err(CoreError::NotFound(
                        "No previous execution in this session".into(),
                    ))
                }
            }
            TypedReference::LastOutput => {
                if let Some(last) = ExecutionHistory::get_last_execution(db, session_id)? {
                    Ok(last.stdout_artifact.unwrap_or_default())
                } else {
                    Err(CoreError::NotFound(
                        "No previous execution in this session".into(),
                    ))
                }
            }
            TypedReference::Errors => {
                if let Some(failed) = ExecutionHistory::get_last_failed_execution(db, session_id)? {
                    Ok(failed
                        .stderr_artifact
                        .unwrap_or_else(|| "Exit with error".into()))
                } else {
                    Err(CoreError::NotFound(
                        "No previous failed execution with errors".into(),
                    ))
                }
            }
            TypedReference::Fact(fact_name) => Ok(format!("fact://{fact_name}")),
            TypedReference::Service(svc_name) => Ok(format!("proc://{svc_name}")),
            TypedReference::Other(other) => Ok(format!("@{other}")),
        }
    }

    /// Resolves all typed references found in an argv list.
    pub fn resolve_argv(
        argv: &[String],
        session_id: &InteractiveSessionId,
        db: Option<&Database>,
    ) -> Vec<String> {
        let db_ref = match db {
            Some(d) => d,
            None => return argv.to_vec(),
        };

        argv.iter()
            .map(|arg| {
                if arg.starts_with('@') {
                    Self::resolve(arg, session_id, db_ref).unwrap_or_else(|_| arg.clone())
                } else {
                    arg.clone()
                }
            })
            .collect()
    }
}
