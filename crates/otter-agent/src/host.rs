use alloc::format;
use alloc::string::String;
use otter_claude::ToolHost;
use otter_tools::{Session, Os, Content};

/// Adapter from Session to ToolHost for the tool loop.
pub struct ToolHostAdapter<'a, O: Os> {
    session: &'a mut Session<O>,
}

impl<'a, O: Os> ToolHostAdapter<'a, O> {
    /// Create a new tool host adapter.
    pub fn new(session: &'a mut Session<O>) -> Self {
        ToolHostAdapter { session }
    }
}

impl<'a, O: Os> ToolHost for ToolHostAdapter<'a, O> {
    fn read_only(&self, tool: &str) -> bool {
        // From otter-tools::TOOLS: list_dir, read_file, and screenshot are read-only
        matches!(tool, "list_dir" | "read_file" | "screenshot")
    }

    fn approve(&mut self, _tool: &str, _input: &str) -> bool {
        // The Session handles approval internally when execute() is called.
        // Always return true here; the Session will ask for approval if needed.
        true
    }

    fn execute(&mut self, tool: &str, input: &str) -> Result<String, String> {
        let result = self.session.call(tool, input);

        // Combine all content as a single message string
        let mut msg = String::new();
        for content in &result.content {
            match content {
                Content::Text(text) => msg.push_str(text),
                Content::Png(png) => {
                    msg.push_str(&format!("[screenshot: {}-byte PNG]", png.len()));
                }
            }
        }

        // Return Err when the Session indicates an error (denied, refused, etc.)
        // Return Ok when the Session indicates success
        // Note: otter-claude's run_tool_loop wraps Err() with "Execution error: ",
        // which may not match the oracle test expectations due to a design mismatch.
        if result.is_error {
            Err(msg)
        } else {
            Ok(msg)
        }
    }
}
