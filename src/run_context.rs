use serde_json::Value;

use crate::tools::Tools;

/// Reserved compatibility Agent used by the current single-agent runtime.
pub const MAIN_AGENT_ID: &str = "main";

/// Per-run execution contract handed to the engine.
///
/// The tool advertisement and the matching executor are derived from the same
/// `Tools` instance, so the model cannot be told about tools that this run
/// cannot execute.
pub struct RunContext<'a> {
    run_id: u64,
    agent_id: &'a str,
    tool_schemas: Value,
    tools: &'a Tools,
}

impl<'a> RunContext<'a> {
    pub fn new(run_id: u64, agent_id: &'a str, tools: &'a Tools) -> Self {
        Self {
            run_id,
            agent_id,
            tool_schemas: tools.schemas(),
            tools,
        }
    }

    pub fn run_id(&self) -> u64 {
        self.run_id
    }

    pub fn agent_id(&self) -> &str {
        self.agent_id
    }

    pub fn tool_schemas(&self) -> &Value {
        &self.tool_schemas
    }

    pub fn tools(&self) -> &Tools {
        self.tools
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn carries_run_identity_and_the_executor_schemas() {
        let paths = crate::instance::InstancePaths::default();
        let tools = Tools::new_in(&paths.workspace, &paths.memory).unwrap();
        let context = RunContext::new(42, MAIN_AGENT_ID, &tools);

        assert_eq!(context.run_id(), 42);
        assert_eq!(context.agent_id(), MAIN_AGENT_ID);
        assert_eq!(context.tool_schemas(), &tools.schemas());
    }
}
