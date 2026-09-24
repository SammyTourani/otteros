#![no_std]
#![no_main]

extern crate alloc;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use libotter::{exit, spawn, wait};
use libotter::lineeditor::LineEditor;

#[macro_use]
extern crate libotter;

const PROMPT: &str = "\u{1b}[1motter> \u{1b}[0m"; // Bold prompt

struct Shell {
    editor: LineEditor,
    test_mode: bool,
}

impl Shell {
    fn new(test_mode: bool) -> Self {
        Shell { editor: LineEditor::new(), test_mode }
    }

    fn process_input(&mut self, line: &str) -> bool {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            return true;
        }

        // Add to history before executing
        self.editor.add_to_history(trimmed);

        // Parse command and arguments
        let parts: Vec<&str> = trimmed.split_whitespace().collect();
        if parts.is_empty() {
            return true;
        }

        match parts[0] {
            "help" => self.cmd_help(),
            "echo" => self.cmd_echo(&parts[1..]),
            "clear" => print!("\x1b[2J\x1b[H"),
            "ps" => self.cmd_ps(),
            "mem" => self.cmd_mem(),
            "uptime" => self.cmd_uptime(),
            "kill" => self.cmd_kill(&parts[1..]),
            "run" => self.cmd_run(&parts[1..]),
            "exit" => {
                let code = if parts.len() > 1 {
                    parts[1].parse::<i32>().unwrap_or(0)
                } else {
                    0
                };
                if self.test_mode {
                    let _ = libotter::test_exit(code);
                }
                exit(code);
            }
            "reboot" => {
                println!("Rebooting...");
                libotter::reboot();
            }
            "history" => self.cmd_history(),
            cmd => {
                // Try to run as external command from /bin/
                let path = format!("/bin/{}", cmd);
                let args: Vec<&str> = parts.iter().skip(1).map(|&s| s).collect();
                self.run_external(&path, &args);
            }
        }
        true
    }

    fn cmd_help(&self) {
        println!("Built-in commands:");
        println!("  help         Show this help");
        println!("  echo <args>  Print arguments");
        println!("  clear        Clear screen");
        println!("  ps           List processes");
        println!("  mem          Show memory info");
        println!("  uptime       Show system uptime");
        println!("  kill <pid>   Kill a process");
        println!("  run <prog>   Run a program from /bin");
        println!("  exit [code]  Exit the shell");
        println!("  reboot       Reboot the system");
        println!("  history      Show command history");
    }

    fn cmd_echo(&self, args: &[&str]) {
        for (i, arg) in args.iter().enumerate() {
            if i > 0 {
                print!(" ");
            }
            print!("{}", arg);
        }
        println!();
    }

    fn cmd_ps(&self) {
        // Syscall 13: proc_list
        let mut buf = [0u8; 4096];
        match libotter::proc_list(&mut buf) {
            Ok(bytes_written) => {
                println!("PID   PPID  STATE NAME");
                let entry_size = 56;
                let num_entries = bytes_written / entry_size;
                for i in 0..num_entries {
                    let offset = i * entry_size;
                    if offset + entry_size > buf.len() {
                        break;
                    }

                    let pid = u64::from_le_bytes([
                        buf[offset], buf[offset + 1], buf[offset + 2], buf[offset + 3],
                        buf[offset + 4], buf[offset + 5], buf[offset + 6], buf[offset + 7],
                    ]);
                    let ppid = u64::from_le_bytes([
                        buf[offset + 8], buf[offset + 9], buf[offset + 10], buf[offset + 11],
                        buf[offset + 12], buf[offset + 13], buf[offset + 14], buf[offset + 15],
                    ]);
                    let state = u32::from_le_bytes([
                        buf[offset + 16], buf[offset + 17], buf[offset + 18], buf[offset + 19],
                    ]);
                    let name_bytes = &buf[offset + 24..offset + 40]; // after pid, ppid, state and 4 bytes of padding
                    let name_str = alloc::string::String::from_utf8_lossy(
                        &name_bytes[..name_bytes.iter().position(|&b| b == 0).unwrap_or(16)]
                    );

                    let state_str = match state {
                        0 => "READY",
                        1 => "RUNNING",
                        2 => "BLOCKED",
                        3 => "DEAD",
                        _ => "?",
                    };

                    println!("{:<5} {:<5} {:<7} {}", pid, ppid, state_str, name_str);
                }
            }
            Err(e) => eprintln!("ps: {e:?}"),
        }
    }

    fn cmd_mem(&self) {
        // Syscall 14: sysinfo
        let mut buf = [0u8; 64];
        match libotter::sysinfo(&mut buf) {
            Ok(()) => {
                let total_frames = u64::from_le_bytes([
                    buf[8], buf[9], buf[10], buf[11],
                    buf[12], buf[13], buf[14], buf[15],
                ]);
                let free_frames = u64::from_le_bytes([
                    buf[16], buf[17], buf[18], buf[19],
                    buf[20], buf[21], buf[22], buf[23],
                ]);
                let heap_bytes = u64::from_le_bytes([
                    buf[24], buf[25], buf[26], buf[27],
                    buf[28], buf[29], buf[30], buf[31],
                ]);

                let total_bytes = total_frames * 4096;
                let free_bytes = free_frames * 4096;
                let used_bytes = total_bytes - free_bytes;

                println!("Memory: {} bytes used / {} bytes total", used_bytes, total_bytes);
                println!("Heap: {} bytes", heap_bytes);
            }
            Err(e) => eprintln!("mem: {e:?}"),
        }
    }

    fn cmd_uptime(&self) {
        // Syscall 14: sysinfo
        let mut buf = [0u8; 64];
        match libotter::sysinfo(&mut buf) {
            Ok(()) => {
                let uptime_ms = u64::from_le_bytes([
                    buf[0], buf[1], buf[2], buf[3],
                    buf[4], buf[5], buf[6], buf[7],
                ]);

                let seconds = uptime_ms / 1000;
                let minutes = seconds / 60;
                let hours = minutes / 60;
                let days = hours / 24;

                println!("Uptime: {}d {}h {}m {}s", days, hours % 24, minutes % 60, seconds % 60);
            }
            Err(e) => eprintln!("uptime: {e:?}"),
        }
    }

    fn cmd_kill(&self, args: &[&str]) {
        if args.is_empty() {
            eprintln!("kill: missing pid");
            return;
        }
        match args[0].parse::<u64>() {
            Ok(pid) => match libotter::kill(pid) {
                Ok(()) => {},
                Err(e) => eprintln!("kill: {e:?}"),
            },
            Err(_) => eprintln!("kill: invalid pid"),
        }
    }

    fn cmd_run(&self, args: &[&str]) {
        if args.is_empty() {
            eprintln!("run: missing program name");
            return;
        }
        let prog = args[0];
        let path = format!("/bin/{}", prog);
        self.run_external(&path, &args[1..]);
    }

    fn cmd_history(&self) {
        for (i, cmd) in self.editor.history.iter().enumerate() {
            println!("{:3} {}", i + 1, cmd);
        }
    }

    fn run_external(&self, path: &str, args: &[&str]) {
        match spawn(path, args) {
            Ok(pid) => {
                match wait(pid) {
                    Ok(code) => {
                        if code >= 128 {
                            // Process was killed (exit code 128 + signal number)
                            println!("killed");
                        } else if code != 0 {
                            println!("exited with code {}", code);
                        }
                    }
                    Err(_) => {
                        eprintln!("wait failed");
                    }
                }
            }
            Err(_) => {
                println!("otsh: {}: command not found", path.trim_start_matches("/bin/"));
            }
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn main() {
    // Get command-line arguments to check for --test mode
    let mut test_mode = false;
    for arg in libotter::args() {
        if *arg == String::from("--test") {
            test_mode = true;
            break;
        }
    }

    if test_mode {
        println!("[kbd] ready");
    }

    let mut shell = Shell::new(test_mode);

    loop {
        // Print prompt (bold in all modes per brief)
        print!("{}", PROMPT);
        libotter::io::flush();

        // Read a line using the line editor
        // One editor for the whole session, so submitted lines are in the history Up/Down walk.
        let line = read_line_with_editor(&mut shell.editor, test_mode);

        if test_mode && !line.is_empty() {
            println!("{}", line);
        }

        shell.process_input(&line);
    }
}

/// Read a line with the line editor, handling ANSI escape sequences.
fn read_line_with_editor(editor: &mut LineEditor, _test_mode: bool) -> String {
    use libotter::lineeditor::EditorAction;

    // Start an empty line; ctrl_u clears the line and cursor and resets the history position.
    editor.ctrl_u();

    loop {
        // Read one byte at a time from stdin
        let byte = libotter::io::read_byte();

        match byte {
            b'\n' => {
                // Enter: complete the line
                println!();
                return editor.line();
            }
            0x7F => {
                // Backspace
                editor.backspace();
                redraw_line(editor, PROMPT);
            }
            0x01..=0x1A => {
                // Ctrl+letter: Ctrl-A/E/U/L
                match byte {
                    0x01 => {
                        // Ctrl-A: Home
                        editor.cursor_home();
                        redraw_line(editor, PROMPT);
                    }
                    0x05 => {
                        // Ctrl-E: End
                        editor.cursor_end();
                        redraw_line(editor, PROMPT);
                    }
                    0x15 => {
                        // Ctrl-U: Clear line
                        editor.ctrl_u();
                        redraw_line(editor, PROMPT);
                    }
                    0x0C => {
                        // Ctrl-L: Clear screen
                        print!("\x1b[2J\x1b[H");
                        libotter::io::flush();
                        print!("{}", PROMPT);
                        libotter::io::flush();
                        redraw_line(editor, PROMPT);
                    }
                    _ => {
                        // Other control characters: ignore
                    }
                }
            }
            _ => {
                // Feed the byte to the line editor's escape sequence state machine
                if let Some(action) = editor.feed_byte(byte) {
                    match action {
                        EditorAction::Byte(ch) => {
                            // Regular character to insert
                            editor.insert_char(ch);
                            redraw_line(editor, PROMPT);
                        }
                        EditorAction::HistoryUp => {
                            editor.history_up();
                            redraw_line(editor, PROMPT);
                        }
                        EditorAction::HistoryDown => {
                            editor.history_down();
                            redraw_line(editor, PROMPT);
                        }
                        EditorAction::CursorRight => {
                            editor.cursor_right();
                            redraw_line(editor, PROMPT);
                        }
                        EditorAction::CursorLeft => {
                            editor.cursor_left();
                            redraw_line(editor, PROMPT);
                        }
                        EditorAction::CursorHome => {
                            editor.cursor_home();
                            redraw_line(editor, PROMPT);
                        }
                        EditorAction::CursorEnd => {
                            editor.cursor_end();
                            redraw_line(editor, PROMPT);
                        }
                        EditorAction::Delete => {
                            editor.delete();
                            redraw_line(editor, PROMPT);
                        }
                    }
                }
                // If feed_byte returns None, it's part of an incomplete escape sequence
            }
        }
    }
}

/// Redraw the current line after an edit.
fn redraw_line(editor: &LineEditor, prompt: &str) {
    let line_text = editor.line();
    let cursor_pos = editor.cursor_pos();

    // Move to start of line, clear to end, print prompt + line
    print!("\r{}", prompt);
    print!("{}", line_text);
    // Clear to end of line
    print!("\x1b[K");

    // Move cursor to correct position (after prompt + text up to cursor)
    let prompt_stripped = prompt
        .replace("\u{1b}[1m", "")
        .replace("\u{1b}[0m", "");
    let moves_needed = (prompt_stripped.len() + cursor_pos) as i32 - (prompt_stripped.len() + line_text.len()) as i32;

    if moves_needed < 0 {
        let left_moves = (-moves_needed) as usize;
        print!("\x1b[{}D", left_moves);
    } else if moves_needed > 0 {
        let right_moves = moves_needed as usize;
        print!("\x1b[{}C", right_moves);
    }

    libotter::io::flush();
}
