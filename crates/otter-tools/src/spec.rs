/// A tool's specification: name, description, JSON schema, and approval requirement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolSpec {
    pub name: &'static str,
    pub description: &'static str,
    pub input_schema: &'static str,
    pub needs_approval: bool,
}

/// The seven agent tools, in policy order.
pub const TOOLS: [ToolSpec; 7] = [
    ToolSpec {
        name: "list_dir",
        description: "List files and directories in a folder. Returns up to 65,536 bytes of \
            directory listing (names and sizes). Case-insensitive on FAT.",
        input_schema: r#"{"type":"object","properties":{"path":{"type":"string"}},"required":["path"],"additionalProperties":false}"#,
        needs_approval: false,
    },
    ToolSpec {
        name: "read_file",
        description: "Read a file. Returns up to 64,000 bytes of UTF-8 text (binary files are \
            marked as such); truncation is noted in the output.",
        input_schema: r#"{"type":"object","properties":{"path":{"type":"string"}},"required":["path"],"additionalProperties":false}"#,
        needs_approval: false,
    },
    ToolSpec {
        name: "write_file",
        description: "Write to a file in /data or /tmp. The human must approve. Creates or \
            replaces the file. Up to 16 MiB total.",
        input_schema: r#"{"type":"object","properties":{"path":{"type":"string"},"content":{"type":"string"}},"required":["path","content"],"additionalProperties":false}"#,
        needs_approval: true,
    },
    ToolSpec {
        name: "run_command",
        description: "Run a shell command with a 30-second timeout. The human must approve. \
            Returns exit status and up to 16 KiB of output; truncation is noted.",
        input_schema: r#"{"type":"object","properties":{"command":{"type":"string"}},"required":["command"],"additionalProperties":false}"#,
        needs_approval: true,
    },
    ToolSpec {
        name: "open_app",
        description: "Open an application (Terminal, Editor, Files, Otter) optionally with a file \
            path. The human must approve. Used to launch and interact with desktop apps.",
        input_schema: r#"{"type":"object","properties":{"app":{"enum":["terminal","editor","files","otter"]},"path":{"type":"string"}},"required":["app"],"additionalProperties":false}"#,
        needs_approval: true,
    },
    ToolSpec {
        name: "fetch_url",
        description: "Fetch a URL with GET. The human must approve: fetched pages are a common \
            prompt-injection channel. Returns status, content type, and up to 64 KiB of body; \
            binary is marked as such.",
        input_schema: r#"{"type":"object","properties":{"url":{"type":"string"}},"required":["url"],"additionalProperties":false}"#,
        needs_approval: true,
    },
    ToolSpec {
        name: "screenshot",
        description: "Take a screenshot of the desktop as PNG. Auto-run, no approval needed.",
        input_schema: r#"{"type":"object","properties":{},"required":[],"additionalProperties":false}"#,
        needs_approval: false,
    },
];
