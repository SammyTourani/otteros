#![no_std]
#![no_main]

extern crate alloc;

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use libotter::{exit, spawn, wait};

// Access the libotter macros
#[macro_use]
extern crate libotter;

const PROMPT: &str = "\u{1b}[1mother> \u{1b}[0m"; // Bold prompt
const HISTORY_SIZE: usize = 64;

struct Shell {
    history: Vec<String>,
    test_mode: bool,
}

impl Shell {
    fn new(test_mode: bool) -> Self {
        Shell { history: Vec::new(), test_mode }
    }

    fn print_prompt(&self) {
        println!("{}", PROMPT);
    }

    fn readline(&self) -> String {
        let mut line = String::new();
        let mut buf = [0u8; 4096];
        let n = libotter::io::read_line(&mut buf);
        if n > 0 {
            // Convert bytes to string
            line = String::from_utf8_lossy(&buf[..n]).to_string();
        }
        line
    }

    fn execute(&mut self, line: &str) {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            return;
        }

        // Add to history (keep last 64)
        if self.history.len() >= HISTORY_SIZE {
            self.history.remove(0);
        }
        self.history.push(trimmed.to_string());

        // Parse command and arguments
        let parts: Vec<&str> = trimmed.split_whitespace().collect();
        if parts.is_empty() {
            return;
        }

        match parts[0] {
            "help" => self.cmd_help(),
            "echo" => self.cmd_echo(&parts[1..]),
            "clear" => println!("\x1b[2J"),
            "ps" => self.cmd_ps(),
            "mem" => self.cmd_mem(),
            "uptime" => self.cmd_uptime(),
            "kill" => self.cmd_kill(&parts[1..]),
            "run" => self.cmd_run(&parts[1..]),
            "exit" => self.cmd_exit(&parts[1..]),
            "reboot" => self.cmd_reboot(),
            "history" => self.cmd_history(),
            cmd => {
                // Try to run as external command from /bin/
                let path = format!("/bin/{}", cmd);
                let args: Vec<&str> = parts.iter().skip(1).map(|&s| s).collect();
                self.run_external(&path, &args);
            }
        }
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
        // Would need syscall 13 (proc_list) - not implemented in shell yet
        println!("[ps: not implemented]");
    }

    fn cmd_mem(&self) {
        // Would need syscall 14 (sysinfo) - not implemented in shell yet
        println!("[mem: not implemented]");
    }

    fn cmd_uptime(&self) {
        // Would need syscall 14 (sysinfo) - not implemented in shell yet
        println!("[uptime: not implemented]");
    }

    fn cmd_kill(&self, args: &[&str]) {
        if args.is_empty() {
            eprintln!("kill: missing pid");
            return;
        }
        eprintln!("kill: not implemented");
    }

    fn cmd_run(&mut self, args: &[&str]) {
        if args.is_empty() {
            eprintln!("run: missing program name");
            return;
        }
        let prog = args[0];
        let path = format!("/bin/{}", prog);
        self.run_external(&path, &args[1..]);
    }

    fn cmd_exit(&self, args: &[&str]) {
        let code = if args.is_empty() {
            0
        } else {
            args[0].parse::<i32>().unwrap_or(0)
        };
        exit(code);
    }

    fn cmd_reboot(&self) {
        println!("Rebooting...");
        // Would need syscall 15 (reboot) - not implemented yet
    }

    fn cmd_history(&self) {
        for (i, cmd) in self.history.iter().enumerate() {
            println!("  {} {}", i + 1, cmd);
        }
    }

    fn run_external(&mut self, path: &str, args: &[&str]) {
        match spawn(path, args) {
            Ok(pid) => {
                match wait(pid) {
                    Ok(code) => {
                        if code != 0 {
                            println!("[exited with code {}]", code);
                        }
                    }
                    Err(_) => {
                        println!("[wait failed]");
                    }
                }
            }
            Err(_) => {
                println!("otsh: {}: command not found", path);
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
        println!("[sh] ready");
    }

    let mut shell = Shell::new(test_mode);

    loop {
        if !test_mode {
            print!("{}", PROMPT);
        }
        let line = shell.readline();
        if test_mode && !line.is_empty() {
            // Echo the command in test mode
            println!("{}", line);
        }
        shell.execute(&line);
    }
}

