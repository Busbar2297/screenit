//! `screenit-cli`: run any engine command from the shell. Same registry as the GUI.
//!
//!   screenit-cli commands [--ids]
//!   screenit-cli exec <command> [json-params]
//!   screenit-cli serve            (JSON lines on stdin/stdout; see docs/control-protocol.md)

use clap::{Parser, Subcommand};
#[cfg(not(windows))]
use screenit_engine::Services;
use screenit_engine::{Session, control};
use std::io::{BufRead, Write};

#[derive(Parser)]
#[command(name = "screenit-cli", version, about = "screenIt command line")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// List every engine command.
    Commands {
        /// Print only the ids, one per line.
        #[arg(long)]
        ids: bool,
    },
    /// Execute one command with optional JSON params and print the JSON result.
    Exec {
        command: String,
        params: Option<String>,
    },
    /// Run a JSON array of [command, params] steps in one session (or `@file.json`).
    /// Steps are executed in order; stops at the first error.
    Run { steps: String },
    /// Serve the control protocol over stdin/stdout.
    Serve,
    /// Run as an MCP (Model Context Protocol) server over stdio; every engine command is a tool.
    Mcp,
}

fn session() -> Session {
    #[cfg(windows)]
    let services = screenit_platform::services();
    #[cfg(not(windows))]
    let services = Services::default();
    Session::new(services)
}

fn main() {
    let cli = Cli::parse();
    let mut s = session();
    let code = match cli.cmd {
        Cmd::Commands { ids } => {
            if ids {
                for c in s.registry() {
                    println!("{}", c.id);
                }
            } else {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&s.command_list()).unwrap_or_default()
                );
            }
            0
        }
        Cmd::Exec { command, params } => {
            let params = match params
                .as_deref()
                .map(serde_json::from_str::<serde_json::Value>)
            {
                None => serde_json::Value::Null,
                Some(Ok(v)) => v,
                Some(Err(e)) => {
                    eprintln!("error: params are not valid JSON: {e}");
                    std::process::exit(2);
                }
            };
            match s.dispatch(&command, &params) {
                Ok(v) => {
                    println!("{v}");
                    0
                }
                Err(e) => {
                    eprintln!("error: {e}");
                    1
                }
            }
        }
        Cmd::Run { steps } => run_steps(&mut s, &steps),
        Cmd::Mcp => {
            let stdin = std::io::stdin();
            let mut out = std::io::stdout();
            for line in stdin.lock().lines().map_while(Result::ok) {
                if line.trim().is_empty() {
                    continue;
                }
                if let Some(resp) = screenit_engine::mcp::handle_mcp_line(&mut s, &line)
                    && writeln!(out, "{resp}").and_then(|()| out.flush()).is_err()
                {
                    break;
                }
            }
            0
        }
        Cmd::Serve => {
            let stdin = std::io::stdin();
            let mut out = std::io::stdout();
            for line in stdin.lock().lines().map_while(Result::ok) {
                if line.trim().is_empty() {
                    continue;
                }
                if writeln!(out, "{}", control::handle_line(&mut s, &line))
                    .and_then(|()| out.flush())
                    .is_err()
                {
                    break;
                }
            }
            0
        }
    };
    std::process::exit(code);
}

fn run_steps(s: &mut Session, steps: &str) -> i32 {
    // `@steps.json` reads the steps from a file (avoids shell quoting problems).
    let text = match steps.strip_prefix('@') {
        Some(path) => std::fs::read_to_string(path).unwrap_or_default(),
        None => steps.to_string(),
    };
    let list: Vec<serde_json::Value> = match serde_json::from_str(&text) {
        Ok(serde_json::Value::Array(a)) => a,
        _ => {
            eprintln!("error: steps must be a JSON array of [command, params]");
            return 2;
        }
    };
    for (i, step) in list.iter().enumerate() {
        let Some(cmd) = step.get(0).and_then(|v| v.as_str()) else {
            eprintln!("error: step {i} needs a command name");
            return 2;
        };
        let params = step.get(1).cloned().unwrap_or(serde_json::Value::Null);
        match s.dispatch(cmd, &params) {
            Ok(v) => println!("{v}"),
            Err(e) => {
                eprintln!("error in step {i} ({cmd}): {e}");
                return 1;
            }
        }
    }
    0
}
