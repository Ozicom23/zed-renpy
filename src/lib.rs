use std::fs;

use zed_extension_api::{
    self as zed, serde_json, settings::LspSettings, DebugAdapterBinary, DebugConfig, DebugRequest,
    DebugScenario, DebugTaskDefinition, LanguageServerId, Result, StartDebuggingRequestArguments,
    StartDebuggingRequestArgumentsRequest,
};

const GITHUB_REPO: &str = "Ozicom23/zed-renpy";
const BINARY_NAME: &str = "renpy-language-server";

struct RenpyExtension {
    cached_binary_path: Option<String>,
}

/// The newest previously-downloaded server binary in the extension work dir,
/// if any (version dirs are named `renpy-language-server-<tag>`).
fn newest_cached_binary(binary_name: &str) -> Option<String> {
    let mut candidates: Vec<String> = fs::read_dir(".")
        .ok()?
        .flatten()
        .filter_map(|entry| {
            let dir = entry.file_name().to_string_lossy().to_string();
            if !dir.starts_with(BINARY_NAME) {
                return None;
            }
            let path = format!("{dir}/{binary_name}");
            fs::metadata(&path).is_ok_and(|m| m.is_file()).then_some(path)
        })
        .collect();
    candidates.sort();
    candidates.pop()
}

impl RenpyExtension {
    /// Resolution order: explicit path in Zed settings, then PATH, then a
    /// binary auto-downloaded from this repo's GitHub releases (cached in the
    /// extension's work directory, one directory per released version).
    ///
    /// `language_server_id` is only used for progress reporting; the debug
    /// adapter resolves the same binary without one.
    fn server_binary(
        &mut self,
        language_server_id: Option<&LanguageServerId>,
        worktree: &zed::Worktree,
    ) -> Result<zed::Command> {
        let binary_settings = LspSettings::for_worktree(BINARY_NAME, worktree)
            .ok()
            .and_then(|settings| settings.binary);
        if let Some(binary) = binary_settings {
            if let Some(path) = binary.path {
                return Ok(zed::Command {
                    command: path,
                    args: binary.arguments.unwrap_or_default(),
                    env: Default::default(),
                });
            }
        }

        if let Some(path) = worktree.which(BINARY_NAME) {
            return Ok(zed::Command { command: path, args: Vec::new(), env: Default::default() });
        }

        if let Some(path) = &self.cached_binary_path {
            if fs::metadata(path).is_ok_and(|meta| meta.is_file()) {
                return Ok(zed::Command {
                    command: path.clone(),
                    args: Vec::new(),
                    env: Default::default(),
                });
            }
        }

        let (platform, arch) = zed::current_platform();
        let binary_name = match platform {
            zed::Os::Windows => format!("{BINARY_NAME}.exe"),
            _ => BINARY_NAME.to_string(),
        };

        if let Some(id) = language_server_id {
            zed::set_language_server_installation_status(
                id,
                &zed::LanguageServerInstallationStatus::CheckingForUpdate,
            );
        }
        let release = match zed::latest_github_release(
            GITHUB_REPO,
            zed::GithubReleaseOptions { require_assets: true, pre_release: false },
        ) {
            Ok(release) => release,
            Err(err) => {
                // Offline or rate-limited: fall back to the newest binary we
                // downloaded on an earlier run, if any.
                if let Some(path) = newest_cached_binary(&binary_name) {
                    if let Some(id) = language_server_id {
                        zed::set_language_server_installation_status(
                            id,
                            &zed::LanguageServerInstallationStatus::None,
                        );
                    }
                    self.cached_binary_path = Some(path.clone());
                    return Ok(zed::Command { command: path, args: Vec::new(), env: Default::default() });
                }
                return Err(format!("failed to check for a renpy-language-server release: {err}"));
            }
        };
        let target = match (platform, arch) {
            (zed::Os::Mac, zed::Architecture::Aarch64) => "aarch64-apple-darwin",
            (zed::Os::Mac, _) => "x86_64-apple-darwin",
            (zed::Os::Linux, zed::Architecture::Aarch64) => "aarch64-unknown-linux-gnu",
            (zed::Os::Linux, _) => "x86_64-unknown-linux-gnu",
            (zed::Os::Windows, _) => "x86_64-pc-windows-msvc",
        };
        let (asset_ext, file_type) = match platform {
            zed::Os::Windows => ("zip", zed::DownloadedFileType::Zip),
            _ => ("tar.gz", zed::DownloadedFileType::GzipTar),
        };
        let asset_name = format!("{BINARY_NAME}-{}-{target}.{asset_ext}", release.version);
        let asset = release
            .assets
            .iter()
            .find(|asset| asset.name == asset_name)
            .ok_or_else(|| format!("release {} has no asset {asset_name}", release.version))?;

        let version_dir = format!("{BINARY_NAME}-{}", release.version);
        let binary_path = format!("{version_dir}/{binary_name}");

        if !fs::metadata(&binary_path).is_ok_and(|meta| meta.is_file()) {
            if let Some(id) = language_server_id {
                zed::set_language_server_installation_status(
                    id,
                    &zed::LanguageServerInstallationStatus::Downloading,
                );
            }
            zed::download_file(&asset.download_url, &version_dir, file_type)
                .map_err(|err| format!("failed to download {asset_name}: {err}"))?;
            zed::make_file_executable(&binary_path)?;

            // Prune caches of older releases.
            if let Ok(entries) = fs::read_dir(".") {
                for entry in entries.flatten() {
                    let name = entry.file_name().to_string_lossy().to_string();
                    if name.starts_with(BINARY_NAME) && name != version_dir {
                        let _ = fs::remove_dir_all(entry.path());
                    }
                }
            }
        }

        if let Some(id) = language_server_id {
            zed::set_language_server_installation_status(
                id,
                &zed::LanguageServerInstallationStatus::None,
            );
        }
        self.cached_binary_path = Some(binary_path.clone());
        Ok(zed::Command { command: binary_path, args: Vec::new(), env: Default::default() })
    }
}

/// The DAP request kind a debug configuration asks for. Only launch exists
/// until the in-game debug agent lands.
fn request_kind(config: &serde_json::Value) -> Result<StartDebuggingRequestArgumentsRequest> {
    match config.get("request").and_then(|v| v.as_str()) {
        None | Some("launch") => Ok(StartDebuggingRequestArgumentsRequest::Launch),
        Some("attach") => {
            Err("attaching to a running Ren'Py game is not supported yet".to_string())
        }
        Some(other) => Err(format!("unknown request kind '{other}' (expected \"launch\")")),
    }
}

impl zed::Extension for RenpyExtension {
    fn new() -> Self {
        RenpyExtension { cached_binary_path: None }
    }

    fn language_server_command(
        &mut self,
        language_server_id: &LanguageServerId,
        worktree: &zed::Worktree,
    ) -> Result<zed::Command> {
        self.server_binary(Some(language_server_id), worktree)
    }

    fn get_dap_binary(
        &mut self,
        adapter_name: String,
        config: DebugTaskDefinition,
        user_provided_debug_adapter_path: Option<String>,
        worktree: &zed::Worktree,
    ) -> Result<DebugAdapterBinary, String> {
        if adapter_name != "renpy" {
            return Err(format!("unknown debug adapter '{adapter_name}'"));
        }
        let command = match user_provided_debug_adapter_path {
            Some(path) => path,
            None => self.server_binary(None, worktree)?.command,
        };
        let configuration: serde_json::Value = serde_json::from_str(&config.config)
            .map_err(|err| format!("invalid debug configuration: {err}"))?;
        Ok(DebugAdapterBinary {
            command: Some(command),
            arguments: vec!["dap".to_string()],
            envs: Vec::new(),
            cwd: Some(worktree.root_path()),
            connection: None,
            request_args: StartDebuggingRequestArguments {
                configuration: config.config,
                request: request_kind(&configuration)?,
            },
        })
    }

    fn dap_request_kind(
        &mut self,
        _adapter_name: String,
        config: serde_json::Value,
    ) -> Result<StartDebuggingRequestArgumentsRequest, String> {
        request_kind(&config)
    }

    /// Maps the generic new-session modal (program/cwd/args/env) onto our
    /// config: for Ren'Py "the program" is the project directory.
    fn dap_config_to_scenario(&mut self, config: DebugConfig) -> Result<DebugScenario, String> {
        match config.request {
            DebugRequest::Launch(launch) => {
                let mut scenario = serde_json::json!({ "request": "launch" });
                if !launch.program.is_empty() {
                    scenario["project"] = launch.program.into();
                } else if let Some(cwd) = launch.cwd {
                    scenario["project"] = cwd.into();
                }
                if !launch.args.is_empty() {
                    scenario["args"] = launch.args.into();
                }
                if !launch.envs.is_empty() {
                    scenario["env"] = launch
                        .envs
                        .into_iter()
                        .map(|(key, value)| (key, serde_json::Value::String(value)))
                        .collect::<serde_json::Map<String, serde_json::Value>>()
                        .into();
                }
                Ok(DebugScenario {
                    adapter: config.adapter,
                    label: config.label,
                    config: scenario.to_string(),
                    build: None,
                    tcp_connection: None,
                })
            }
            DebugRequest::Attach(_) => {
                Err("attaching to a running Ren'Py game is not supported yet".to_string())
            }
        }
    }
}

zed::register_extension!(RenpyExtension);
