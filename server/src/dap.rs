//! `renpy-language-server dap` — a Debug Adapter Protocol server that runs
//! Ren'Py games from the editor's debugger UI.
//!
//! Launching: spawns the game through the Ren'Py SDK (optionally warped to a
//! file:line), streams the game's output into the debug console, and stops it
//! on terminate/disconnect.
//!
//! Debugging: unless the client asks for `noDebug`, an in-game agent
//! (assets/zed_renpy_debug.rpe.py, embedded in this binary) is written into
//! the project's game/ directory and the game connects back to us over
//! loopback with a one-shot token. The agent pauses on breakpoints in both
//! python lines (sys.settrace) and Ren'Py statements (statement callbacks),
//! and serves stack/scopes/variables/evaluate while paused; this side
//! translates DAP requests into the agent's newline-delimited JSON protocol
//! and editor-absolute paths into the project-relative ("elided") paths
//! Ren'Py uses internally.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use serde_json::{json, Value};

use crate::renpy_cli;

/// The in-game debug agent, injected into the project at launch.
static DEBUG_AGENT_PY: &str = include_str!("../assets/zed_renpy_debug.rpe.py");
const AGENT_FILE_NAME: &str = "zed_debug.rpe.py";

/// Shared stdout with the DAP sequence counter, so events emitted by the
/// output-streaming and agent threads interleave safely with responses.
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
    #[serde(rename = "noDebug")]
    no_debug: bool,
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

/// Editor-absolute path -> Ren'Py's project-relative ("elided") form.
fn elide(project: &Path, absolute: &str) -> Option<String> {
    let relative = Path::new(absolute).strip_prefix(project).ok()?;
    Some(relative.to_string_lossy().replace('\\', "/"))
}

/// Not cryptographic — just makes the loopback port unusable by an unrelated
/// local process that races to connect before the game does.
fn session_token() -> String {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    std::process::id().hash(&mut hasher);
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos()
        .hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

/// Everything the DAP loop and the agent threads share.
#[derive(Default)]
struct BridgeState {
    /// Write half of the agent connection, once the game has called back.
    agent: Option<TcpStream>,
    next_id: i64,
    waiting: HashMap<i64, mpsc::Sender<Value>>,
    /// Editor-absolute path -> breakpoint lines, as last set by the client.
    breakpoints: HashMap<String, Vec<u32>>,
    project: Option<PathBuf>,
    configuration_done: bool,
    injected_file: Option<PathBuf>,
}

#[derive(Clone, Default)]
struct Bridge(Arc<Mutex<BridgeState>>);

impl Bridge {
    /// Send one JSON line to the agent, if connected.
    fn agent_send(&self, mut message: Value, with_id: bool) -> Option<i64> {
        let mut state = self.0.lock().unwrap();
        let id = if with_id {
            state.next_id += 1;
            message["id"] = json!(state.next_id);
            Some(state.next_id)
        } else {
            None
        };
        let stream = state.agent.as_ref()?;
        let mut stream = stream;
        let line = message.to_string() + "\n";
        if stream.write_all(line.as_bytes()).is_err() {
            return None;
        }
        id
    }

    /// Round-trip a command to the agent. The reply channel is registered
    /// before the write so even an instant reply cannot be lost. Fails fast
    /// when no game is connected; times out defensively otherwise.
    fn agent_request(&self, mut message: Value) -> Result<Value, String> {
        let (tx, rx) = mpsc::channel();
        let id = {
            let mut state = self.0.lock().unwrap();
            if state.agent.is_none() {
                return Err("the game is not connected for debugging".to_string());
            }
            state.next_id += 1;
            let id = state.next_id;
            message["id"] = json!(id);
            state.waiting.insert(id, tx);
            let line = message.to_string() + "\n";
            let write_ok = {
                let mut stream = state.agent.as_ref().unwrap();
                stream.write_all(line.as_bytes()).is_ok()
            };
            if !write_ok {
                state.waiting.remove(&id);
                return Err("failed to talk to the game".to_string());
            }
            id
        };
        let reply = rx.recv_timeout(Duration::from_secs(5));
        self.0.lock().unwrap().waiting.remove(&id);
        let reply = reply.map_err(|_| "the game did not answer in time".to_string())?;
        if let Some(error) = reply.get("error").and_then(|e| e.as_str()) {
            return Err(error.to_string());
        }
        Ok(reply)
    }

    /// Push the client's current breakpoints to the agent, elided.
    fn flush_breakpoints(&self) {
        let (files, project) = {
            let state = self.0.lock().unwrap();
            let Some(project) = state.project.clone() else { return };
            (state.breakpoints.clone(), project)
        };
        for (absolute, lines) in files {
            if let Some(elided) = elide(&project, &absolute) {
                self.agent_send(
                    json!({ "cmd": "set_breakpoints", "file": elided, "lines": lines }),
                    false,
                );
            }
        }
    }

    fn remove_injected_file(&self) {
        let path = self.0.lock().unwrap().injected_file.take();
        if let Some(path) = path {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// Reads agent messages: replies are routed to waiting DAP requests, stopped
/// events are forwarded to the client.
fn agent_reader(stream: TcpStream, bridge: Bridge, writer: Writer, expected_token: String) {
    let reader = BufReader::new(match stream.try_clone() {
        Ok(clone) => clone,
        Err(_) => return,
    });
    let mut lines = reader.lines();

    // First line must be the hello with our session token.
    match lines.next() {
        Some(Ok(line)) => {
            let hello: Value = match serde_json::from_str(&line) {
                Ok(value) => value,
                Err(_) => return,
            };
            if hello["event"] != "hello" || hello["token"].as_str() != Some(&expected_token) {
                return;
            }
        }
        _ => return,
    }

    let configuration_done = {
        let mut state = bridge.0.lock().unwrap();
        state.agent = Some(stream);
        state.configuration_done
    };
    bridge.flush_breakpoints();
    if configuration_done {
        bridge.agent_send(json!({ "cmd": "configuration_done" }), false);
    }
    writer.output("console", "Ren'Py debug agent connected — breakpoints active.\n".into());

    for line in lines {
        let Ok(line) = line else { break };
        let Ok(message) = serde_json::from_str::<Value>(&line) else { continue };
        if let Some(reply_to) = message.get("reply_to").and_then(|v| v.as_i64()) {
            let tx = bridge.0.lock().unwrap().waiting.remove(&reply_to);
            if let Some(tx) = tx {
                let _ = tx.send(message);
            }
            continue;
        }
        if message["event"] == "stopped" {
            writer.event(
                "stopped",
                json!({
                    "reason": message["reason"],
                    "threadId": 1,
                    "allThreadsStopped": true,
                }),
            );
        }
    }

    let mut state = bridge.0.lock().unwrap();
    state.agent = None;
    state.waiting.clear();
}

/// Map one agent stack frame to a DAP frame; `frame` doubles as the id.
fn dap_frame(project: Option<&Path>, frame: &Value) -> Value {
    let file = frame["file"].as_str().unwrap_or("unknown");
    let mut out = json!({
        "id": frame["frame"],
        "name": frame["name"],
        "line": frame["line"],
        "column": 1,
    });
    if file != "unknown" {
        let absolute = match project {
            Some(project) => project.join(file).to_string_lossy().into_owned(),
            None => file.to_string(),
        };
        out["source"] = json!({
            "name": Path::new(file).file_name().map(|n| n.to_string_lossy().into_owned()),
            "path": absolute,
        });
    }
    out
}

pub fn run() -> Result<(), Box<dyn std::error::Error + Sync + Send>> {
    let stdin = std::io::stdin();
    let mut reader = BufReader::new(stdin.lock());
    let writer = Writer::new();
    let bridge = Bridge::default();
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
                        "supportsSetVariable": true,
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

                // Debug plumbing: loopback listener + agent injection. Plain
                // launch when the client asked to run without debugging.
                let mut extra_env: Vec<(String, String)> = Vec::new();
                if !config.no_debug {
                    match TcpListener::bind("127.0.0.1:0") {
                        Ok(listener) => {
                            let token = session_token();
                            let port = listener.local_addr().map(|a| a.port()).unwrap_or(0);
                            let agent_path = resolved.project.join("game").join(AGENT_FILE_NAME);
                            match std::fs::write(&agent_path, DEBUG_AGENT_PY) {
                                Ok(()) => {
                                    {
                                        let mut state = bridge.0.lock().unwrap();
                                        state.project = Some(resolved.project.clone());
                                        state.injected_file = Some(agent_path);
                                    }
                                    extra_env.push(("ZED_RENPY_DEBUG_PORT".into(), port.to_string()));
                                    extra_env.push(("ZED_RENPY_DEBUG_TOKEN".into(), token.clone()));
                                    let bridge = bridge.clone();
                                    let writer_for_agent = writer.clone();
                                    std::thread::spawn(move || {
                                        let _ = listener.set_nonblocking(true);
                                        let deadline = Instant::now() + Duration::from_secs(60);
                                        loop {
                                            match listener.accept() {
                                                Ok((stream, _)) => {
                                                    let _ = stream.set_nodelay(true);
                                                    let _ = stream.set_nonblocking(false);
                                                    agent_reader(
                                                        stream,
                                                        bridge,
                                                        writer_for_agent,
                                                        token,
                                                    );
                                                    break;
                                                }
                                                Err(err)
                                                    if err.kind()
                                                        == std::io::ErrorKind::WouldBlock =>
                                                {
                                                    if Instant::now() > deadline {
                                                        writer_for_agent.output(
                                                            "console",
                                                            "Ren'Py debug agent did not connect \
                                                             (needs Ren'Py 8.2+); running without \
                                                             breakpoints.\n"
                                                                .into(),
                                                        );
                                                        break;
                                                    }
                                                    std::thread::sleep(Duration::from_millis(100));
                                                }
                                                Err(_) => break,
                                            }
                                        }
                                    });
                                }
                                Err(err) => writer.output(
                                    "console",
                                    format!(
                                        "Could not install the debug agent ({err}); running \
                                         without breakpoints.\n"
                                    ),
                                ),
                            }
                        }
                        Err(err) => writer.output(
                            "console",
                            format!("Debugger port unavailable ({err}); running without breakpoints.\n"),
                        ),
                    }
                }

                let mut command = Command::new(&resolved.program);
                command
                    .args(&resolved.args)
                    .envs(&resolved.env)
                    .envs(extra_env)
                    .current_dir(&resolved.project)
                    .stdin(Stdio::null())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped());
                let mut child = match command.spawn() {
                    Ok(child) => child,
                    Err(err) => {
                        bridge.remove_injected_file();
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
                let bridge_for_exit = bridge.clone();
                std::thread::spawn(move || {
                    let code = child
                        .wait()
                        .ok()
                        .and_then(|status| status.code())
                        .unwrap_or(-1);
                    *game_pid_for_exit.lock().unwrap() = None;
                    bridge_for_exit.remove_injected_file();
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
                let path = request["arguments"]["source"]["path"]
                    .as_str()
                    .unwrap_or("")
                    .to_string();
                let lines: Vec<u32> = request["arguments"]["breakpoints"]
                    .as_array()
                    .map(|list| {
                        list.iter()
                            .filter_map(|bp| bp["line"].as_u64().map(|l| l as u32))
                            .collect()
                    })
                    .unwrap_or_default();
                let elided = {
                    let mut state = bridge.0.lock().unwrap();
                    if lines.is_empty() {
                        state.breakpoints.remove(&path);
                    } else {
                        state.breakpoints.insert(path.clone(), lines.clone());
                    }
                    state
                        .project
                        .as_ref()
                        .and_then(|project| elide(project, &path))
                };
                if let Some(elided) = elided {
                    bridge.agent_send(
                        json!({ "cmd": "set_breakpoints", "file": elided, "lines": lines }),
                        false,
                    );
                }
                let breakpoints: Vec<Value> = lines
                    .iter()
                    .map(|line| json!({ "verified": true, "line": line }))
                    .collect();
                writer.respond(&request, json!({ "breakpoints": breakpoints }));
            }
            "setFunctionBreakpoints" => {
                writer.respond(&request, json!({ "breakpoints": [] }));
            }
            "setExceptionBreakpoints" => {
                writer.respond(&request, json!({ "breakpoints": [] }));
            }
            "configurationDone" => {
                bridge.0.lock().unwrap().configuration_done = true;
                bridge.agent_send(json!({ "cmd": "configuration_done" }), false);
                writer.respond(&request, json!({}));
            }
            "threads" => {
                writer.respond(&request, json!({ "threads": [{ "id": 1, "name": "Ren'Py" }] }));
            }
            "stackTrace" => match bridge.agent_request(json!({ "cmd": "stack" })) {
                Ok(reply) => {
                    let project = bridge.0.lock().unwrap().project.clone();
                    let frames: Vec<Value> = reply["frames"]
                        .as_array()
                        .map(|frames| {
                            frames.iter().map(|f| dap_frame(project.as_deref(), f)).collect()
                        })
                        .unwrap_or_default();
                    let total = frames.len();
                    writer.respond(
                        &request,
                        json!({ "stackFrames": frames, "totalFrames": total }),
                    );
                }
                Err(_) => {
                    writer.respond(&request, json!({ "stackFrames": [], "totalFrames": 0 }));
                }
            },
            "scopes" => {
                let frame = request["arguments"]["frameId"].as_i64().unwrap_or(0);
                match bridge.agent_request(json!({ "cmd": "scopes", "frame": frame })) {
                    Ok(reply) => {
                        let scopes: Vec<Value> = reply["scopes"]
                            .as_array()
                            .map(|scopes| {
                                scopes
                                    .iter()
                                    .map(|s| {
                                        json!({
                                            "name": s["name"],
                                            "variablesReference": s["ref"],
                                            "expensive": false,
                                        })
                                    })
                                    .collect()
                            })
                            .unwrap_or_default();
                        writer.respond(&request, json!({ "scopes": scopes }));
                    }
                    Err(_) => writer.respond(&request, json!({ "scopes": [] })),
                }
            }
            "variables" => {
                let reference = request["arguments"]["variablesReference"].as_i64().unwrap_or(0);
                match bridge.agent_request(json!({ "cmd": "variables", "ref": reference })) {
                    Ok(reply) => {
                        let variables: Vec<Value> = reply["variables"]
                            .as_array()
                            .map(|vars| {
                                vars.iter()
                                    .map(|v| {
                                        json!({
                                            "name": v["name"],
                                            "value": v["value"],
                                            "type": v["type"],
                                            "variablesReference": v["ref"],
                                        })
                                    })
                                    .collect()
                            })
                            .unwrap_or_default();
                        writer.respond(&request, json!({ "variables": variables }));
                    }
                    Err(_) => writer.respond(&request, json!({ "variables": [] })),
                }
            }
            "setVariable" => {
                let arguments = &request["arguments"];
                match bridge.agent_request(json!({
                    "cmd": "set_variable",
                    "ref": arguments["variablesReference"],
                    "name": arguments["name"],
                    "value": arguments["value"],
                })) {
                    Ok(reply) => {
                        let variable = &reply["variable"];
                        writer.respond(
                            &request,
                            json!({
                                "value": variable["value"],
                                "type": variable["type"],
                                "variablesReference": variable["ref"],
                            }),
                        );
                    }
                    Err(message) => writer.fail(&request, message),
                }
            }
            "evaluate" => {
                let expression = request["arguments"]["expression"].as_str().unwrap_or("");
                match bridge.agent_request(json!({ "cmd": "evaluate", "expr": expression })) {
                    Ok(reply) => writer.respond(
                        &request,
                        json!({
                            "result": reply["value"],
                            "type": reply["type"],
                            "variablesReference": reply["ref"],
                        }),
                    ),
                    Err(message) => writer.fail(&request, message),
                }
            }
            "continue" => {
                bridge.agent_send(json!({ "cmd": "continue" }), false);
                writer.respond(&request, json!({ "allThreadsContinued": true }));
            }
            "next" => {
                bridge.agent_send(json!({ "cmd": "next" }), false);
                writer.respond(&request, json!({}));
            }
            "stepIn" => {
                bridge.agent_send(json!({ "cmd": "step_in" }), false);
                writer.respond(&request, json!({}));
            }
            "stepOut" => {
                bridge.agent_send(json!({ "cmd": "step_out" }), false);
                writer.respond(&request, json!({}));
            }
            "pause" => {
                let has_agent = bridge.0.lock().unwrap().agent.is_some();
                if has_agent {
                    bridge.agent_send(json!({ "cmd": "pause" }), false);
                    writer.respond(&request, json!({}));
                } else {
                    writer.fail(&request, "the game is not connected for debugging".to_string());
                }
            }
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
                bridge.remove_injected_file();
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
    bridge.remove_injected_file();
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
        assert!(!config.no_debug);
    }

    #[test]
    fn no_debug_flag_is_honored() {
        let config: LaunchConfig =
            serde_json::from_value(json!({ "noDebug": true })).unwrap();
        assert!(config.no_debug);
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
    fn elide_maps_absolute_paths_into_the_project() {
        let project = if cfg!(windows) { Path::new("C:\\proj") } else { Path::new("/proj") };
        let absolute = if cfg!(windows) { "C:\\proj\\game\\script.rpy" } else { "/proj/game/script.rpy" };
        assert_eq!(elide(project, absolute).as_deref(), Some("game/script.rpy"));
        assert_eq!(elide(project, "/elsewhere/x.rpy"), None);
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
