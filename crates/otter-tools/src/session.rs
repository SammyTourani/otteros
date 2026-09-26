use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;
use core::cmp::Ordering;
use otter_json::{parse_str, Value};

use crate::format::validate_schema;
use crate::os::Os;
use crate::policy::{error_message, is_binary, is_protected, is_writable_location, should_show_as_text};
use crate::{Approval, Content, LogEntry, Outcome, ToolResult};
use crate::normalize;
use crate::spec::TOOLS;

/// A tool-call session with an Os, optional secret redaction, and an action log.
pub struct Session<O: Os> {
    os: O,
    secret: Vec<u8>,
    log: Vec<LogEntry>,
}

impl<O: Os> Session<O> {
    /// Create a new session.
    pub fn new(os: O) -> Self {
        Session {
            os,
            secret: Vec::new(),
            log: Vec::new(),
        }
    }

    /// Set the secret for redaction throughout the session.
    pub fn with_secret(mut self, secret: &[u8]) -> Self {
        self.secret = secret.to_vec();
        self
    }

    /// Process a tool call: validate, check policy, ask for approval if needed, execute, format output, log.
    pub fn call(&mut self, tool_name: &str, input_json: &str) -> ToolResult {
        let spec = TOOLS.iter().find(|t| t.name == tool_name);

        // Unknown tool is invalid.
        if spec.is_none() {
            let outcome = Outcome::Invalid;
            let error_text = "invalid input: unknown tool";
            self.log.push(LogEntry {
                tool: tool_name.to_string(),
                summary: tool_name.to_string(),
                outcome: outcome.clone(),
            });
            return ToolResult {
                content: vec![Content::Text(self.redact_str(error_text))],
                is_error: true,
            };
        }
        let spec = spec.unwrap();

        // Parse input as JSON.
        let value = match parse_str(input_json) {
            Ok(v) => v,
            Err(_) => {
                let outcome = Outcome::Invalid;
                let error_text = "invalid input: not valid JSON";
                self.log.push(LogEntry {
                    tool: tool_name.to_string(),
                    summary: tool_name.to_string(),
                    outcome,
                });
                return ToolResult {
                    content: vec![Content::Text(self.redact_str(error_text))],
                    is_error: true,
                };
            }
        };

        // Validate against schema.
        if let Err(e) = validate_schema(&value, spec.input_schema) {
            let outcome = Outcome::Invalid;
            self.log.push(LogEntry {
                tool: tool_name.to_string(),
                summary: tool_name.to_string(),
                outcome,
            });
            return ToolResult {
                content: vec![Content::Text(self.redact_str(&format!("invalid input: {e}")))],
                is_error: true,
            };
        }

        // Dispatch to the appropriate tool handler.
        match tool_name {
            "list_dir" => self.handle_list_dir(spec, &value),
            "read_file" => self.handle_read_file(spec, &value),
            "write_file" => self.handle_write_file(spec, &value),
            "run_command" => self.handle_run_command(spec, &value),
            "open_app" => self.handle_open_app(spec, &value),
            "fetch_url" => self.handle_fetch_url(spec, &value),
            "screenshot" => self.handle_screenshot(spec, &value),
            _ => unreachable!(),
        }
    }

    pub fn log(&self) -> &[LogEntry] {
        &self.log
    }

    pub fn os(&self) -> &O {
        &self.os
    }

    pub fn os_mut(&mut self) -> &mut O {
        &mut self.os
    }

    /// Redact the secret from text.
    fn redact_str(&self, text: &str) -> String {
        if self.secret.is_empty() {
            return text.to_string();
        }
        let secret_str = match alloc::string::String::from_utf8(self.secret.clone()) {
            Ok(s) => s,
            Err(_) => return text.to_string(),
        };
        text.replace(&secret_str, "[REDACTED]")
    }

    /// Redact the secret from bytes.
    fn redact_bytes(&self, bytes: &[u8]) -> Vec<u8> {
        if self.secret.is_empty() {
            return bytes.to_vec();
        }
        let mut result = bytes.to_vec();
        while let Some(pos) = result.windows(self.secret.len()).position(|w| w == self.secret.as_slice()) {
            result.splice(pos..pos + self.secret.len(), b"[REDACTED]".iter().copied());
        }
        result
    }

    fn handle_list_dir(&mut self, _spec: &crate::spec::ToolSpec, value: &Value) -> ToolResult {
        let path = match value.get("path").and_then(|v| v.as_str()) {
            Some(p) => p,
            None => {
                self.log.push(LogEntry {
                    tool: "list_dir".to_string(),
                    summary: "List".to_string(),
                    outcome: Outcome::Invalid,
                });
                return ToolResult {
                    content: vec![Content::Text("invalid input: missing path".to_string())],
                    is_error: true,
                };
            }
        };

        let norm_path = match normalize(path) {
            Ok(p) => p,
            Err(_) => {
                self.log.push(LogEntry {
                    tool: "list_dir".to_string(),
                    summary: "List".to_string(),
                    outcome: Outcome::Invalid,
                });
                return ToolResult {
                    content: vec![Content::Text("invalid input: invalid path".to_string())],
                    is_error: true,
                };
            }
        };

        // Check protected paths (case-insensitive on FAT).
        if is_protected(&norm_path) {
            let outcome = Outcome::Refused;
            self.log.push(LogEntry {
                tool: "list_dir".to_string(),
                summary: format!("List {}", norm_path),
                outcome,
            });
            return ToolResult {
                content: vec![Content::Text("refused: protected path".to_string())],
                is_error: true,
            };
        }

        let summary = format!("List {}", norm_path);
        match self.os.list_dir(&norm_path) {
            Ok(mut entries) => {
                // Sort entries: case-insensitive, then by byte order for tie-breaking.
                entries.sort_by(|a, b| {
                    let a_name_lower = a.name.to_lowercase();
                    let b_name_lower = b.name.to_lowercase();
                    match a_name_lower.cmp(&b_name_lower) {
                        Ordering::Equal => a.name.as_bytes().cmp(b.name.as_bytes()),
                        other => other,
                    }
                });

                let mut output = String::new();
                if entries.is_empty() {
                    output.push_str("(empty directory)");
                } else {
                    for entry in entries {
                        if !output.is_empty() {
                            output.push('\n');
                        }
                        if entry.is_dir {
                            output.push_str(&format!("{}/", entry.name));
                        } else {
                            output.push_str(&format!("{} ({} bytes)", entry.name, entry.size));
                        }
                    }
                }
                self.log.push(LogEntry {
                    tool: "list_dir".to_string(),
                    summary,
                    outcome: Outcome::AutoRun,
                });
                ToolResult {
                    content: vec![Content::Text(output)],
                    is_error: false,
                }
            }
            Err(e) => {
                let error_text = format!("failed: {}", error_message(&e));
                self.log.push(LogEntry {
                    tool: "list_dir".to_string(),
                    summary,
                    outcome: Outcome::Failed,
                });
                ToolResult {
                    content: vec![Content::Text(error_text)],
                    is_error: true,
                }
            }
        }
    }

    fn handle_read_file(&mut self, _spec: &crate::spec::ToolSpec, value: &Value) -> ToolResult {
        let path = match value.get("path").and_then(|v| v.as_str()) {
            Some(p) => p,
            None => {
                self.log.push(LogEntry {
                    tool: "read_file".to_string(),
                    summary: "Read".to_string(),
                    outcome: Outcome::Invalid,
                });
                return ToolResult {
                    content: vec![Content::Text("invalid input: missing path".to_string())],
                    is_error: true,
                };
            }
        };

        let norm_path = match normalize(path) {
            Ok(p) => p,
            Err(_) => {
                self.log.push(LogEntry {
                    tool: "read_file".to_string(),
                    summary: "Read".to_string(),
                    outcome: Outcome::Invalid,
                });
                return ToolResult {
                    content: vec![Content::Text("invalid input: invalid path".to_string())],
                    is_error: true,
                };
            }
        };

        // Check protected paths.
        if is_protected(&norm_path) {
            let outcome = Outcome::Refused;
            self.log.push(LogEntry {
                tool: "read_file".to_string(),
                summary: format!("Read {}", norm_path),
                outcome,
            });
            return ToolResult {
                content: vec![Content::Text("refused: protected path".to_string())],
                is_error: true,
            };
        }

        let summary = format!("Read {}", norm_path);
        const LIMIT: usize = 65_536;
        match self.os.read_file(&norm_path, LIMIT) {
            Ok((data, total_size)) => {
                let output = if is_binary(&data) {
                    format!("[binary file, {} bytes]", total_size)
                } else {
                    // Try to decode as UTF-8, cutting at character boundaries.
                    match alloc::string::String::from_utf8(data.clone()) {
                        Ok(text) => {
                            if total_size > LIMIT as u64 {
                                let note = format!("\n[truncated: showing {} of {} bytes]", text.len(), total_size);
                                format!("{}{}", text, note)
                            } else {
                                text
                            }
                        }
                        Err(_) => format!("[binary file, {} bytes]", total_size),
                    }
                };

                self.log.push(LogEntry {
                    tool: "read_file".to_string(),
                    summary,
                    outcome: Outcome::AutoRun,
                });
                ToolResult {
                    content: vec![Content::Text(self.redact_str(&output))],
                    is_error: false,
                }
            }
            Err(e) => {
                let error_text = format!("failed: {}", error_message(&e));
                self.log.push(LogEntry {
                    tool: "read_file".to_string(),
                    summary,
                    outcome: Outcome::Failed,
                });
                ToolResult {
                    content: vec![Content::Text(error_text)],
                    is_error: true,
                }
            }
        }
    }

    fn handle_write_file(&mut self, _spec: &crate::spec::ToolSpec, value: &Value) -> ToolResult {
        let path = match value.get("path").and_then(|v| v.as_str()) {
            Some(p) => p,
            None => {
                self.log.push(LogEntry {
                    tool: "write_file".to_string(),
                    summary: "Write".to_string(),
                    outcome: Outcome::Invalid,
                });
                return ToolResult {
                    content: vec![Content::Text("invalid input: missing path".to_string())],
                    is_error: true,
                };
            }
        };

        let content = match value.get("content").and_then(|v| v.as_str()) {
            Some(c) => c,
            None => {
                self.log.push(LogEntry {
                    tool: "write_file".to_string(),
                    summary: "Write".to_string(),
                    outcome: Outcome::Invalid,
                });
                return ToolResult {
                    content: vec![Content::Text("invalid input: missing content".to_string())],
                    is_error: true,
                };
            }
        };

        let norm_path = match normalize(path) {
            Ok(p) => p,
            Err(_) => {
                self.log.push(LogEntry {
                    tool: "write_file".to_string(),
                    summary: "Write".to_string(),
                    outcome: Outcome::Invalid,
                });
                return ToolResult {
                    content: vec![Content::Text("invalid input: invalid path".to_string())],
                    is_error: true,
                };
            }
        };

        // Check policy: protected paths and write locations.
        if is_protected(&norm_path) || !is_writable_location(&norm_path) {
            let outcome = Outcome::Refused;
            let summary = format!("Write {} bytes to {}", content.len(), norm_path);
            self.log.push(LogEntry {
                tool: "write_file".to_string(),
                summary,
                outcome,
            });
            return ToolResult {
                content: vec![Content::Text("refused: can only write to /data and /tmp".to_string())],
                is_error: true,
            };
        }

        // Check if file exists for proper approval message.
        let existing_size = self.os.file_size(&norm_path).ok().flatten();
        let summary = if let Some(size) = existing_size {
            format!("Replace {} ({} bytes) with {} bytes", norm_path, size, content.len())
        } else {
            format!("Write {} bytes to {}", content.len(), norm_path)
        };

        // Prepare detail (bounded to 2 KiB).
        let detail = if content.len() > 2048 {
            format!("{}\n[... {} more bytes]", &content[..2048], content.len() - 2048)
        } else {
            content.to_string()
        };

        let approval = Approval {
            tool: "write_file".to_string(),
            summary: summary.clone(),
            detail: self.redact_str(&detail),
        };

        if !self.os.ask(&approval) {
            self.log.push(LogEntry {
                tool: "write_file".to_string(),
                summary,
                outcome: Outcome::Denied,
            });
            return ToolResult {
                content: vec![Content::Text("The user denied this action.".to_string())],
                is_error: true,
            };
        }

        match self.os.write_file(&norm_path, content.as_bytes()) {
            Ok(()) => {
                self.log.push(LogEntry {
                    tool: "write_file".to_string(),
                    summary,
                    outcome: Outcome::Allowed,
                });
                ToolResult {
                    content: vec![Content::Text(format!("Wrote {} bytes to {}", content.len(), norm_path))],
                    is_error: false,
                }
            }
            Err(e) => {
                let error_text = format!("failed: {}", error_message(&e));
                self.log.push(LogEntry {
                    tool: "write_file".to_string(),
                    summary,
                    outcome: Outcome::Failed,
                });
                ToolResult {
                    content: vec![Content::Text(error_text)],
                    is_error: true,
                }
            }
        }
    }

    fn handle_run_command(&mut self, _spec: &crate::spec::ToolSpec, value: &Value) -> ToolResult {
        let command = match value.get("command").and_then(|v| v.as_str()) {
            Some(c) => c,
            None => {
                self.log.push(LogEntry {
                    tool: "run_command".to_string(),
                    summary: "Run".to_string(),
                    outcome: Outcome::Invalid,
                });
                return ToolResult {
                    content: vec![Content::Text("invalid input: missing command".to_string())],
                    is_error: true,
                };
            }
        };

        // Check policy: forbid commands with "anthropic.key" or "/config" (case-insensitive).
        let command_lower = command.to_lowercase();
        if command_lower.contains("anthropic.key") || command_lower.contains("/config") {
            let outcome = Outcome::Refused;
            let summary = if command.len() > 200 {
                format!("Run: {}…", &command[..200])
            } else {
                format!("Run: {}", command)
            };
            self.log.push(LogEntry {
                tool: "run_command".to_string(),
                summary,
                outcome,
            });
            return ToolResult {
                content: vec![Content::Text("refused: access to /config is blocked".to_string())],
                is_error: true,
            };
        }

        let summary = if command.len() > 200 {
            format!("Run: {}…", &command[..200])
        } else {
            format!("Run: {}", command)
        };

        let approval = Approval {
            tool: "run_command".to_string(),
            summary: summary.clone(),
            detail: command.to_string(),
        };

        if !self.os.ask(&approval) {
            self.log.push(LogEntry {
                tool: "run_command".to_string(),
                summary,
                outcome: Outcome::Denied,
            });
            return ToolResult {
                content: vec![Content::Text("The user denied this action.".to_string())],
                is_error: true,
            };
        }

        match self.os.run_command(command, 30_000, 16_384) {
            Ok(output) => {
                let redacted_output = self.redact_bytes(&output.output);
                let output_text = if let Ok(s) = alloc::string::String::from_utf8(redacted_output) {
                    s
                } else {
                    "[binary output]".to_string()
                };

                let mut result = format!("exit status {}\n", output.status);
                if output.truncated {
                    result.push_str(&format!("{}\n[output truncated at {} bytes]", output_text, output.output.len()));
                } else {
                    result.push_str(&output_text);
                }

                self.log.push(LogEntry {
                    tool: "run_command".to_string(),
                    summary,
                    outcome: Outcome::Allowed,
                });
                ToolResult {
                    content: vec![Content::Text(result)],
                    is_error: false,
                }
            }
            Err(e) => {
                let error_text = format!("failed: {}", error_message(&e));
                self.log.push(LogEntry {
                    tool: "run_command".to_string(),
                    summary,
                    outcome: Outcome::Failed,
                });
                ToolResult {
                    content: vec![Content::Text(error_text)],
                    is_error: true,
                }
            }
        }
    }

    fn handle_open_app(&mut self, _spec: &crate::spec::ToolSpec, value: &Value) -> ToolResult {
        let app_str = match value.get("app").and_then(|v| v.as_str()) {
            Some(a) => a,
            None => {
                self.log.push(LogEntry {
                    tool: "open_app".to_string(),
                    summary: "Open".to_string(),
                    outcome: Outcome::Invalid,
                });
                return ToolResult {
                    content: vec![Content::Text("invalid input: missing app".to_string())],
                    is_error: true,
                };
            }
        };

        let app = match app_str {
            "terminal" => crate::os::App::Terminal,
            "editor" => crate::os::App::Editor,
            "files" => crate::os::App::Files,
            "otter" => crate::os::App::Otter,
            _ => {
                self.log.push(LogEntry {
                    tool: "open_app".to_string(),
                    summary: "Open".to_string(),
                    outcome: Outcome::Invalid,
                });
                return ToolResult {
                    content: vec![Content::Text("invalid input: unknown app".to_string())],
                    is_error: true,
                };
            }
        };

        // Get optional path (if present, must be a string).
        let path = value.get("path").and_then(|v| v.as_str());

        // Normalize and check path if provided.
        let norm_path = if let Some(p) = path {
            match normalize(p) {
                Ok(np) => Some(np),
                Err(_) => {
                    self.log.push(LogEntry {
                        tool: "open_app".to_string(),
                        summary: "Open".to_string(),
                        outcome: Outcome::Invalid,
                    });
                    return ToolResult {
                        content: vec![Content::Text("invalid input: invalid path".to_string())],
                        is_error: true,
                    };
                }
            }
        } else {
            None
        };

        // Check protected paths.
        if let Some(ref p) = norm_path
            && is_protected(p)
        {
            let outcome = Outcome::Refused;
            let summary = format!("Open {} in {}", p, app_str);
            self.log.push(LogEntry {
                tool: "open_app".to_string(),
                summary,
                outcome,
            });
            return ToolResult {
                content: vec![Content::Text("refused: cannot open protected paths".to_string())],
                is_error: true,
            };
        }

        let summary = match &norm_path {
            Some(p) => format!("Open {} in {}", p, match app {
                crate::os::App::Terminal => "Terminal",
                crate::os::App::Editor => "Editor",
                crate::os::App::Files => "Files",
                crate::os::App::Otter => "Otter",
            }),
            None => format!("Open {}", match app {
                crate::os::App::Terminal => "Terminal",
                crate::os::App::Editor => "Editor",
                crate::os::App::Files => "Files",
                crate::os::App::Otter => "Otter",
            }),
        };

        let approval = Approval {
            tool: "open_app".to_string(),
            summary: summary.clone(),
            detail: summary.clone(),
        };

        if !self.os.ask(&approval) {
            self.log.push(LogEntry {
                tool: "open_app".to_string(),
                summary,
                outcome: Outcome::Denied,
            });
            return ToolResult {
                content: vec![Content::Text("The user denied this action.".to_string())],
                is_error: true,
            };
        }

        match self.os.open_app(app, norm_path.as_deref()) {
            Ok(()) => {
                let result = match &norm_path {
                    Some(p) => format!("Opened {} in {}", p, match app {
                        crate::os::App::Terminal => "Terminal",
                        crate::os::App::Editor => "Editor",
                        crate::os::App::Files => "Files",
                        crate::os::App::Otter => "Otter",
                    }),
                    None => format!("Opened {}", match app {
                        crate::os::App::Terminal => "Terminal",
                        crate::os::App::Editor => "Editor",
                        crate::os::App::Files => "Files",
                        crate::os::App::Otter => "Otter",
                    }),
                };
                self.log.push(LogEntry {
                    tool: "open_app".to_string(),
                    summary,
                    outcome: Outcome::Allowed,
                });
                ToolResult {
                    content: vec![Content::Text(result)],
                    is_error: false,
                }
            }
            Err(e) => {
                let error_text = format!("failed: {}", error_message(&e));
                self.log.push(LogEntry {
                    tool: "open_app".to_string(),
                    summary,
                    outcome: Outcome::Failed,
                });
                ToolResult {
                    content: vec![Content::Text(error_text)],
                    is_error: true,
                }
            }
        }
    }

    fn handle_fetch_url(&mut self, _spec: &crate::spec::ToolSpec, value: &Value) -> ToolResult {
        let url = match value.get("url").and_then(|v| v.as_str()) {
            Some(u) => u,
            None => {
                self.log.push(LogEntry {
                    tool: "fetch_url".to_string(),
                    summary: "Fetch".to_string(),
                    outcome: Outcome::Invalid,
                });
                return ToolResult {
                    content: vec![Content::Text("invalid input: missing url".to_string())],
                    is_error: true,
                };
            }
        };

        // Validate URL: must be http:// or https://, must have a host, max 2048 bytes.
        if url.len() > 2048 || (!url.starts_with("http://") && !url.starts_with("https://")) {
            self.log.push(LogEntry {
                tool: "fetch_url".to_string(),
                summary: "Fetch".to_string(),
                outcome: Outcome::Invalid,
            });
            return ToolResult {
                content: vec![Content::Text("invalid input: URL must be http:// or https://, at most 2048 bytes".to_string())],
                is_error: true,
            };
        }

        // Check for a host.
        let after_scheme: &str = url.strip_prefix("https://").or_else(|| url.strip_prefix("http://")).unwrap_or_default();

        if after_scheme.is_empty() || after_scheme.starts_with('/') {
            self.log.push(LogEntry {
                tool: "fetch_url".to_string(),
                summary: "Fetch".to_string(),
                outcome: Outcome::Invalid,
            });
            return ToolResult {
                content: vec![Content::Text("invalid input: URL must have a host".to_string())],
                is_error: true,
            };
        }

        let summary = format!("Fetch {}", url);
        let approval = Approval {
            tool: "fetch_url".to_string(),
            summary: summary.clone(),
            detail: url.to_string(),
        };

        if !self.os.ask(&approval) {
            self.log.push(LogEntry {
                tool: "fetch_url".to_string(),
                summary,
                outcome: Outcome::Denied,
            });
            return ToolResult {
                content: vec![Content::Text("The user denied this action.".to_string())],
                is_error: true,
            };
        }

        match self.os.fetch(url, 65_536) {
            Ok(output) => {
                let body_text = if should_show_as_text(&output.content_type) {
                    match alloc::string::String::from_utf8(output.body.clone()) {
                        Ok(s) => s,
                        Err(_) => "[binary body]".to_string(),
                    }
                } else {
                    "[binary body]".to_string()
                };

                let redacted = self.redact_str(&body_text);
                let mut result = format!(
                    "HTTP {}, {}, {} bytes\n\n{}",
                    output.status, output.content_type, output.body.len(), redacted
                );

                if output.truncated {
                    result.push_str(&format!("\n[truncated at {} bytes]", output.body.len()));
                }

                self.log.push(LogEntry {
                    tool: "fetch_url".to_string(),
                    summary,
                    outcome: Outcome::Allowed,
                });
                ToolResult {
                    content: vec![Content::Text(result)],
                    is_error: false,
                }
            }
            Err(e) => {
                let error_text = format!("failed: {}", error_message(&e));
                self.log.push(LogEntry {
                    tool: "fetch_url".to_string(),
                    summary,
                    outcome: Outcome::Failed,
                });
                ToolResult {
                    content: vec![Content::Text(error_text)],
                    is_error: true,
                }
            }
        }
    }

    fn handle_screenshot(&mut self, _spec: &crate::spec::ToolSpec, _value: &Value) -> ToolResult {
        match self.os.screenshot() {
            Ok(png_data) => {
                self.log.push(LogEntry {
                    tool: "screenshot".to_string(),
                    summary: "Screenshot".to_string(),
                    outcome: Outcome::AutoRun,
                });
                ToolResult {
                    content: vec![Content::Png(png_data)],
                    is_error: false,
                }
            }
            Err(e) => {
                let error_text = format!("failed: {}", error_message(&e));
                self.log.push(LogEntry {
                    tool: "screenshot".to_string(),
                    summary: "Screenshot".to_string(),
                    outcome: Outcome::Failed,
                });
                ToolResult {
                    content: vec![Content::Text(error_text)],
                    is_error: true,
                }
            }
        }
    }
}
