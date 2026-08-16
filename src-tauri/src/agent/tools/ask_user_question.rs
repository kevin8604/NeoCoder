use super::{PostExecuteAction, Tool, ToolContext};
use crate::chat::QuestionItem;

pub struct AskUserQuestion;

#[async_trait::async_trait]
impl Tool for AskUserQuestion {
    fn name(&self) -> &str {
        "ask_user_question"
    }

    async fn execute(&self, _args: serde_json::Value, _ctx: &ToolContext) -> String {
        // The actual handling is done by AgentLoop via post_execute_action
        "Asking user for input...".to_string()
    }

    fn post_execute_action(&self, args: &serde_json::Value) -> PostExecuteAction {
        let questions: Vec<QuestionItem> =
            serde_json::from_value(args["questions"].clone()).unwrap_or_default();
        PostExecuteAction::AskUser(questions)
    }
}
