use omen_core::{CoreError, InteractiveSessionId};
use omen_knowledge::{Database, ExecutionHistory};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiLaneOutput {
    pub configured: bool,
    pub query: String,
    pub response_text: String,
    pub suggested_commands: Vec<String>,
}

pub struct AiLaneDispatcher;

impl AiLaneDispatcher {
    pub fn dispatch(
        query: &str,
        session_id: &InteractiveSessionId,
        db: Option<&Database>,
    ) -> Result<AiLaneOutput, CoreError> {
        let mut suggestions = vec![":status".to_string()];

        let has_failed = if let Some(db_ref) = db {
            matches!(
                ExecutionHistory::get_last_failed_execution(db_ref, session_id),
                Ok(Some(_))
            )
        } else {
            false
        };

        if has_failed {
            suggestions.insert(0, ":show @failed".to_string());
            suggestions.insert(1, ":why @last".to_string());
            suggestions.insert(2, ":rerun @failed".to_string());
        } else {
            suggestions.insert(0, ":inspect @last".to_string());
            suggestions.insert(1, ":history".to_string());
        }

        let response_text = format!(
            "AI reasoning lane is not configured.\n\nQuery received: \"{}\"\n\nDeterministic alternatives available directly from machine state:",
            query
        );

        Ok(AiLaneOutput {
            configured: false,
            query: query.to_string(),
            response_text,
            suggested_commands: suggestions,
        })
    }
}
