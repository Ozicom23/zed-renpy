//! `renpy-language-server dap` — a Debug Adapter Protocol server that runs
//! Ren'Py games from the editor's debugger UI.
//!
//! This is a launch adapter: it starts the game through the Ren'Py SDK
//! (optionally warped to a file:line), streams the game's output into the
//! debug console, and stops it on terminate/disconnect. Breakpoints are
//! acknowledged but reported unverified — script-level debugging is served by
//! later tiers.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};

use crate::renpy_cli;

/// Shared stdout with the DAP sequence counter, so events emitted by the
/// output-streaming threads interleave safely with responses.
#[derive(Clone)]
struct Writer(Arc<Mutex<WriterState>>);

struct WriterState {
    out: std::io::Stdout,
    seq: i64,
}

impl Writer {
    fn new() -> Self {
        Writer(Arc::new(Mutex::new(WriterState { out: std::io::stdout(), seq: 0 })))
    }

    fn send(&self, mut message: Value) {
        let mut state = self.0.lock().unwrap();
        state.seq += 1;
        message["seq"] = json!(state.seq);
        let payload = message.to_string();
        let _ = write!(state.out, "Content-Length: {}\r\n\r\n{payload}", payload.len());
        let _ = state.out.flush();
    }

    fn event(&self, event: &str, body: Value) {
        self.send(json!({ "type": "event", "event": event, "body": body }));
    }

    fn output(&self, category: &str, text: String) {
        self.event("output", json!({ "category": category, "output": text }));
    }

    fn respond(&self, request: &Value, body: Value) {
        self.send(json!({
            "type": "response",
            "request_seq": request["seq"],
            "command": request["command"],
            "success": true,
            "body": body,
        }));
    }

    fn fail(&self, request: &Value, message: String) {
        self.send(json!({
            "type": "response",
            "request_seq": request["seq"],
            "command": request["command"],
            "success": false,
            "message": message,
        }));
    }
}

/// One Content-Length framed message, or None on EOF.
fn read_message(reader: &mut impl BufRead) -> std::io::Result<Option<Value>> {
    let mut content_length: Option<usize> = None;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line)? == 0 {
            return Ok(None);
        }
        let line = line.trim_end();
        if line.is_empty() {
            if content_length.is_some() {
                break;
            }
            continue; // stray blank line before any header
        }
        if let Some(value) = line.strip_prefix("Content-Length:") {
            content_length = value.trim().parse().ok();
        }
    }
    let length = content_length
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidData, "missing Content-Length"))?;
    let mut buf = vec![0u8; length];
    reader.read_exact(&mut buf)?;
    let value = serde_json::from_slice(&buf)
        .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidData, err))?;
    Ok(Some(value))
}

/// The launch configuration from the editor's debug.json entry. Unknown fields
/// (label, adapter, editor-internal keys) are ignored.
#[derive(Default, serde::Deserialize)]
#[serde(default)]
struct LaunchConfig {
    sdk: Option<String>,
    project: Option<String>,
    warp: Option<String>,
    /// Ren'Py command to execute; defaults to "run". ("lint", "compile", ...)
    command: Option<String>,
    args: Vec<String>,
    env: HashMap<String, String>,
}

#[derive(Debug)]
struct ResolvedLaunch {
    program: PathBuf,
    args: Vec<String>,
    env: HashMap<String, String>,
    project: PathBuf,
}

/// Turn a launch config into a concrete command line, or a user-facing error.
fn resolve_launch(config: &LaunchConfig) -> Result<ResolvedLaunch, String> {
    let sdk = match &config.sdk {
        Some(path) => {
            let path = PathBuf::from(path);
            if !renpy_cli::is_sdk_dir(&path) {
                return Err(format!(
                    "'{}' does not look like a Ren'Py SDK (no renpy.py inside)",
                    path.display()
                ));
            }
            path
        }
        None => renpy_cli::find_sdk().ok_or_else(|| {
            "No Ren'Py SDK found. Set \"sdk\" in the debug configuration to your SDK directory, \
             or set the RENPY_SDK environment variable, or keep the SDK (renpy-*-sdk) in your \
             home, Documents, or Downloads folder."
                .to_string()
        })?,
    };
    let invocation = renpy_cli::sdk_invocation(&sdk)
        .ok_or_else(|| format!("Ren'Py SDK at '{}' has no launcher for this platform", sdk.display()))?;

    let start = match &config.project {
        Some(path) => PathBuf::from(path),
        None => std::env::current_dir().map_err(|err| err.to_string())?,
    };
    let project = renpy_cli::find_project(&start).ok_or_else(|| {
        format!(
            "No Ren'Py project found at '{}' (no game/ directory). \
             Set \"project\" in the debug configuration.",
            start.display()
        )
    })?;

    let mut args = invocation.prefix_args;
    args.push(project.to_string_lossy().into_owned());
    args.push(config.command.clone().unwrap_or_else(|| "run".to_string()));
    if let Some(warp) = &config.warp {
        args.push("--warp".to_string());
        args.push(renpy_cli::normalize_warp(warp, &project));
    }
    args.extend(config.args.iter().cloned());

    Ok(ResolvedLaunch { program: invocation.program, args, env: config.env.clone(), project })
}

fn kill_pid(pid: u32, force: bool) {
    #[cfg(unix)]
    {
        let signal = if force { "-KILL" } else { "-TERM" };
        let _ = Command::new("kill").arg(signal).arg(pid.to_string()).status();
    }
    #[cfg(windows)]
    {
        let mut cmd = Command::new("taskkill");
        cmd.args(["/T", "/PID", &pid.to_string()]);
        if force {
            cmd.arg("/F");
        }
        let _ = cmd.status();
    }
}

pub fn run() -> Result<(), Box<dyn std::error::Error + Sync + Send>> {
    let stdin = std::io::stdin();
    let mut reader = BufReader::new(stdin.lock());
    let writer = Writer::new();
    // Pid of the running game, if any; cleared by the exit-watcher thread.
    let game_pid: Arc<Mutex<Option<u32>>> = Arc::new(Mutex::new(None));

    while let Some(request) = read_message(&mut reader)? {
        if request["type"] != "request" {
            continue;
        }
        match request["command"].as_str().unwrap_or("") {
            "initialize" => {
                writer.respond(
                    &request,
                    json!({
                        "supportsConfigurationDoneRequest": true,
                        "supportsTerminateRequest": true,
                    }),
                );
                writer.event("initialized", json!({}));
            }
            "launch" => {
                let config: LaunchConfig =
                    serde_json::from_value(request["arguments"].clone()).unwrap_or_default();
                let resolved = match resolve_launch(&config) {
                    Ok(resolved) => resolved,
                    Err(message) => {
                        writer.fail(&request, message);
                        continue;
                    }
                };
                let mut command = Command::new(&resolved.program);
                command
                    .args(&resolved.args)
                    .envs(&resolved.env)
                    .current_dir(&resolved.project)
                    .stdin(Stdio::null())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped());
                let mut child = match command.spawn() {
                    Ok(child) => child,
                    Err(err) => {
                        writer.fail(
                            &request,
                            format!("failed to start '{}': {err}", resolved.program.display()),
                        );
                        continue;
                    }
                };
                let pid = child.id();
                *game_pid.lock().unwrap() = Some(pid);
                writer.output(
                    "console",
                    format!("Launching: {} {}\n", resolved.program.display(), resolved.args.join(" ")),
                );
                writer.respond(&request, json!({}));
                writer.event(
                    "process",
                    json!({
                        "name": resolved.project.to_string_lossy(),
                        "systemProcessId": pid,
                        "isLocalProcess": true,
                        "startMethod": "launch",
                    }),
                );

                for (stream, category) in [
                    (child.stdout.take().map(|s| Box::new(s) as Box<dyn Read + Send>), "stdout"),
                    (child.stderr.take().map(|s| Box::new(s) as Box<dyn Read + Send>), "stderr"),
                ] {
                    let Some(stream) = stream else { continue };
                    let writer = writer.clone();
                    std::thread::spawn(move || {
                        for line in BufReader::new(stream).lines() {
                            let Ok(line) = line else { break };
                            writer.output(category, format!("{line}\n"));
                        }
                    });
                }
                let writer_for_exit = writer.clone();
                let game_pid_for_exit = game_pid.clone();
                std::thread::spawn(move || {
                    let code = child
                        .wait()
                        .ok()
                        .and_then(|status| status.code())
                        .unwrap_or(-1);
                    *game_pid_for_exit.lock().unwrap() = None;
                    writer_for_exit.event("exited", json!({ "exitCode": code }));
                    writer_for_exit.event("terminated", json!({}));
                });
            }
            "attach" => {
                writer.fail(
                    &request,
                    "Attaching to a running Ren'Py game is not supported yet — use a launch \
                     configuration."
                        .to_string(),
                );
            }
            "setBreakpoints" => {
                let requested = request["arguments"]["breakpoints"]
                    .as_array()
                    .map(|list| list.len())
                    .unwrap_or(0);
                let breakpoints: Vec<Value> = (0..requested)
                    .map(|_| {
                        json!({
                            "verified": false,
                            "message": "Breakpoints are not supported yet for Ren'Py scripts \
                                        (planned for a future release)."
                        })
                    })
                    .collect();
                writer.respond(&request, json!({ "breakpoints": breakpoints }));
            }
            "setFunctionBreakpoints" => {
                writer.respond(&request, json!({ "breakpoints": [] }));
            }
            "setExceptionBreakpoints" => {
                writer.respond(&request, json!({ "breakpoints": [] }));
            }
            "configurationDone" => writer.respond(&request, json!({})),
            "threads" => {
                writer.respond(&request, json!({ "threads": [{ "id": 1, "name": "Ren'Py" }] }));
            }
            "stackTrace" => {
                writer.respond(&request, json!({ "stackFrames": [], "totalFrames": 0 }));
            }
            "scopes" => writer.respond(&request, json!({ "scopes": [] })),
            "variables" => writer.respond(&request, json!({ "variables": [] })),
            "continue" => writer.respond(&request, json!({ "allThreadsContinued": true })),
            "terminate" => {
                if let Some(pid) = *game_pid.lock().unwrap() {
                    kill_pid(pid, false);
                }
                writer.respond(&request, json!({}));
            }
            "disconnect" => {
                if let Some(pid) = *game_pid.lock().unwrap() {
                    kill_pid(pid, true);
                }
                writer.respond(&request, json!({}));
                break;
            }
            other => {
                writer.fail(&request, format!("unsupported request '{other}'"));
            }
        }
    }

    // Editor went away (or disconnected): never leave a game process behind.
    if let Some(pid) = *game_pid.lock().unwrap() {
        kill_pid(pid, true);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launch_config_ignores_unknown_fields_and_defaults() {
        let config: LaunchConfig = serde_json::from_value(json!({
            "request": "launch",
            "label": "Run game",
            "adapter": "renpy",
            "warp": "game/script.rpy:12",
            "env": { "RENPY_SKIP_SPLASHSCREEN": "1" },
        }))
        .unwrap();
        assert_eq!(config.warp.as_deref(), Some("game/script.rpy:12"));
        assert_eq!(config.env["RENPY_SKIP_SPLASHSCREEN"], "1");
        assert!(config.sdk.is_none());
        assert!(config.args.is_empty());
    }

    #[test]
    fn resolve_launch_reports_bad_sdk() {
        let config = LaunchConfig {
            sdk: Some("/definitely/not/an/sdk".into()),
            ..Default::default()
        };
        let err = resolve_launch(&config).unwrap_err();
        assert!(err.contains("does not look like a Ren'Py SDK"), "{err}");
    }

    #[test]
    fn framing_reads_messages_and_eof() {
        let body = r#"{"type":"request","command":"initialize","seq":1}"#;
        let framed = format!("Content-Length: {}\r\n\r\n{body}", body.len());
        let two = format!("{framed}{framed}");
        let mut reader = BufReader::new(two.as_bytes());
        let first = read_message(&mut reader).unwrap().unwrap();
        assert_eq!(first["command"], "initialize");
        let second = read_message(&mut reader).unwrap().unwrap();
        assert_eq!(second["seq"], 1);
        assert!(read_message(&mut reader).unwrap().is_none());
    }
}
