use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use semver::Version;
use zed::settings::LspSettings;
use zed_extension_api::{self as zed, Result};

const SERVER_NAME: &str = "sanemark";
const GITHUB_REPOSITORY: &str = "nkitsaini/sanemark";

struct SanemarkExtension {
    cached_binary: Option<PathBuf>,
}

struct Platform {
    target: String,
    archive_type: zed::DownloadedFileType,
    archive_extension: &'static str,
    executable_name: &'static str,
    is_windows: bool,
}

impl SanemarkExtension {
    fn resolve_binary(
        &mut self,
        language_server_id: &zed::LanguageServerId,
        worktree: &zed::Worktree,
    ) -> Result<PathBuf> {
        if let Some(path) = worktree.which(SERVER_NAME) {
            clear_installation_status(language_server_id);
            return Ok(path.into());
        }

        if let Some(path) = self.cached_binary.as_ref().filter(|path| path.is_file()) {
            clear_installation_status(language_server_id);
            return Ok(path.clone());
        }

        let platform = Platform::current().inspect_err(|error| {
            set_installation_failed(language_server_id, error);
        })?;
        let cached_binary = find_cached_binary(Path::new("."), &platform);

        zed::set_language_server_installation_status(
            language_server_id,
            &zed::LanguageServerInstallationStatus::CheckingForUpdate,
        );

        match self.download_latest_binary(language_server_id, &platform) {
            Ok(path) => {
                self.cached_binary = Some(path.clone());
                clear_installation_status(language_server_id);
                Ok(path)
            }
            Err(update_error) => {
                if let Some(path) = cached_binary {
                    if let Err(cache_error) = make_executable_if_needed(&path, &platform) {
                        let error = format!(
                            "failed to update Sanemark ({update_error}); the cached binary is unusable: {cache_error}"
                        );
                        set_installation_failed(language_server_id, &error);
                        return Err(error);
                    }

                    eprintln!(
                        "failed to update Sanemark; using cached binary {}: {update_error}",
                        path.display()
                    );
                    self.cached_binary = Some(path.clone());
                    clear_installation_status(language_server_id);
                    Ok(path)
                } else {
                    set_installation_failed(language_server_id, &update_error);
                    Err(update_error)
                }
            }
        }
    }

    fn download_latest_binary(
        &self,
        language_server_id: &zed::LanguageServerId,
        platform: &Platform,
    ) -> Result<PathBuf> {
        let release = zed::latest_github_release(
            GITHUB_REPOSITORY,
            zed::GithubReleaseOptions {
                require_assets: true,
                pre_release: false,
            },
        )
        .map_err(|error| format!("failed to find the latest Sanemark release: {error}"))?;

        let package_name = format!("sanemark-{}-{}", release.version, platform.target);
        let asset_name = format!("{package_name}.{}", platform.archive_extension);
        let asset = release
            .assets
            .iter()
            .find(|asset| asset.name == asset_name)
            .ok_or_else(|| {
                format!(
                    "release {} has no asset named {asset_name}",
                    release.version
                )
            })?;

        let install_dir = PathBuf::from(format!("sanemark-{}", release.version));
        let binary_path = install_dir
            .join(&package_name)
            .join(platform.executable_name);

        if !binary_path.is_file() {
            zed::set_language_server_installation_status(
                language_server_id,
                &zed::LanguageServerInstallationStatus::Downloading,
            );

            if install_dir.exists() {
                fs::remove_dir_all(&install_dir).map_err(|error| {
                    format!("failed to remove an incomplete Sanemark download: {error}")
                })?;
            }

            zed::download_file(
                &asset.download_url,
                path_as_str(&install_dir)?,
                platform.archive_type,
            )
            .map_err(|error| format!("failed to download {asset_name}: {error}"))?;

            if !binary_path.is_file() {
                return Err(format!(
                    "{asset_name} did not contain the expected executable at {}",
                    binary_path.display()
                ));
            }
        }

        // Reapply this even for an existing download. A previous Zed session may
        // have been interrupted after extraction but before chmod completed.
        make_executable_if_needed(&binary_path, platform)?;
        Ok(binary_path)
    }
}

impl Platform {
    fn current() -> Result<Self> {
        let (os, architecture) = zed::current_platform();
        let architecture = match architecture {
            zed::Architecture::Aarch64 => "aarch64",
            zed::Architecture::X8664 => "x86_64",
            zed::Architecture::X86 => {
                return Err("Sanemark does not provide 32-bit release binaries".into());
            }
        };

        Ok(match os {
            zed::Os::Linux => Self {
                target: format!("{architecture}-unknown-linux-musl"),
                archive_type: zed::DownloadedFileType::GzipTar,
                archive_extension: "tar.gz",
                executable_name: SERVER_NAME,
                is_windows: false,
            },
            zed::Os::Mac => Self {
                target: format!("{architecture}-apple-darwin"),
                archive_type: zed::DownloadedFileType::GzipTar,
                archive_extension: "tar.gz",
                executable_name: SERVER_NAME,
                is_windows: false,
            },
            zed::Os::Windows => Self {
                target: format!("{architecture}-pc-windows-msvc"),
                archive_type: zed::DownloadedFileType::Zip,
                archive_extension: "zip",
                executable_name: "sanemark.exe",
                is_windows: true,
            },
        })
    }
}

fn find_cached_binary(work_dir: &Path, platform: &Platform) -> Option<PathBuf> {
    fs::read_dir(work_dir)
        .ok()?
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| {
            let file_name = entry.file_name();
            let release = file_name.to_str()?.strip_prefix("sanemark-")?;
            let version = Version::parse(release.strip_prefix('v').unwrap_or(release)).ok()?;
            let package_name = format!("sanemark-{release}-{}", platform.target);
            let binary_path = entry
                .path()
                .join(package_name)
                .join(platform.executable_name);
            binary_path.is_file().then_some((version, binary_path))
        })
        .max_by(|(left, _), (right, _)| left.cmp(right))
        .map(|(_, path)| path)
}

fn make_executable_if_needed(path: &Path, platform: &Platform) -> Result<()> {
    if !platform.is_windows {
        zed::make_file_executable(path_as_str(path)?)
            .map_err(|error| format!("failed to make the Sanemark binary executable: {error}"))?;
    }
    Ok(())
}

fn clear_installation_status(language_server_id: &zed::LanguageServerId) {
    zed::set_language_server_installation_status(
        language_server_id,
        &zed::LanguageServerInstallationStatus::None,
    );
}

fn set_installation_failed(language_server_id: &zed::LanguageServerId, error: &str) {
    zed::set_language_server_installation_status(
        language_server_id,
        &zed::LanguageServerInstallationStatus::Failed(error.to_owned()),
    );
}

impl zed::Extension for SanemarkExtension {
    fn new() -> Self {
        Self {
            cached_binary: None,
        }
    }

    fn language_server_command(
        &mut self,
        language_server_id: &zed::LanguageServerId,
        worktree: &zed::Worktree,
    ) -> Result<zed::Command> {
        let settings = LspSettings::for_worktree(language_server_id.as_ref(), worktree)?;
        let configured_binary = settings.binary;

        let command = match configured_binary
            .as_ref()
            .and_then(|binary| binary.path.clone())
        {
            Some(path) => {
                clear_installation_status(language_server_id);
                path
            }
            None => path_as_str(&self.resolve_binary(language_server_id, worktree)?)?.to_owned(),
        };
        let args = configured_binary
            .as_ref()
            .and_then(|binary| binary.arguments.clone())
            .unwrap_or_default();
        let mut env: BTreeMap<_, _> = worktree.shell_env().into_iter().collect();
        if let Some(configured_env) = configured_binary.and_then(|binary| binary.env) {
            env.extend(configured_env);
        }

        Ok(zed::Command {
            command,
            args,
            env: env.into_iter().collect(),
        })
    }

    fn language_server_initialization_options(
        &mut self,
        language_server_id: &zed::LanguageServerId,
        worktree: &zed::Worktree,
    ) -> Result<Option<zed::serde_json::Value>> {
        Ok(
            LspSettings::for_worktree(language_server_id.as_ref(), worktree)?
                .initialization_options,
        )
    }

    fn language_server_workspace_configuration(
        &mut self,
        language_server_id: &zed::LanguageServerId,
        worktree: &zed::Worktree,
    ) -> Result<Option<zed::serde_json::Value>> {
        Ok(LspSettings::for_worktree(language_server_id.as_ref(), worktree)?.settings)
    }
}

fn path_as_str(path: &Path) -> Result<&str> {
    path.to_str()
        .ok_or_else(|| format!("path is not valid UTF-8: {}", path.display()))
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;

    static NEXT_TEST_ID: AtomicU64 = AtomicU64::new(0);

    #[test]
    fn finds_newest_compatible_cached_binary() {
        let root = test_dir();
        let platform = test_platform();
        let older = create_cached_binary(&root, "v0.9.0", &platform);
        let newer = create_cached_binary(&root, "v0.10.0", &platform);

        // These must not be considered usable cache entries.
        fs::create_dir_all(root.join("sanemark-v99.0.0")).unwrap();
        fs::create_dir_all(root.join("sanemark-not-semver")).unwrap();
        let wrong_target = root
            .join("sanemark-v100.0.0")
            .join("sanemark-v100.0.0-aarch64-apple-darwin");
        fs::create_dir_all(&wrong_target).unwrap();
        fs::write(wrong_target.join(SERVER_NAME), []).unwrap();

        assert_ne!(older, newer);
        assert_eq!(find_cached_binary(&root, &platform), Some(newer));

        fs::remove_dir_all(root).unwrap();
    }

    fn test_platform() -> Platform {
        Platform {
            target: "x86_64-unknown-linux-musl".to_owned(),
            archive_type: zed::DownloadedFileType::GzipTar,
            archive_extension: "tar.gz",
            executable_name: SERVER_NAME,
            is_windows: false,
        }
    }

    fn test_dir() -> PathBuf {
        let id = NEXT_TEST_ID.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("sanemark-zed-test-{}-{id}", std::process::id()));
        fs::create_dir(&path).unwrap();
        path
    }

    fn create_cached_binary(root: &Path, release: &str, platform: &Platform) -> PathBuf {
        let path = root
            .join(format!("sanemark-{release}"))
            .join(format!("sanemark-{release}-{}", platform.target))
            .join(platform.executable_name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, []).unwrap();
        path
    }
}

zed::register_extension!(SanemarkExtension);
