use std::fs;

use zed_extension_api::{self as zed, settings::LspSettings, LanguageServerId, Result};

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
    fn server_binary(
        &mut self,
        language_server_id: &LanguageServerId,
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

        zed::set_language_server_installation_status(
            language_server_id,
            &zed::LanguageServerInstallationStatus::CheckingForUpdate,
        );
        let release = match zed::latest_github_release(
            GITHUB_REPO,
            zed::GithubReleaseOptions { require_assets: true, pre_release: false },
        ) {
            Ok(release) => release,
            Err(err) => {
                // Offline or rate-limited: fall back to the newest binary we
                // downloaded on an earlier run, if any.
                if let Some(path) = newest_cached_binary(&binary_name) {
                    zed::set_language_server_installation_status(
                        language_server_id,
                        &zed::LanguageServerInstallationStatus::None,
                    );
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
            zed::set_language_server_installation_status(
                language_server_id,
                &zed::LanguageServerInstallationStatus::Downloading,
            );
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

        zed::set_language_server_installation_status(
            language_server_id,
            &zed::LanguageServerInstallationStatus::None,
        );
        self.cached_binary_path = Some(binary_path.clone());
        Ok(zed::Command { command: binary_path, args: Vec::new(), env: Default::default() })
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
        self.server_binary(language_server_id, worktree)
    }
}

zed::register_extension!(RenpyExtension);
