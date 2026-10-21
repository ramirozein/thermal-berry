use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use tauri::{AppHandle, Manager};

use crate::thermal::ThermalError;

type CmdResult<T> = Result<T, ThermalError>;

const REPO: &str = "ramirozein/thermal-berry";
const INSTALLER: &str = include_str!("../../install.sh");
static UPDATE_IN_PROGRESS: AtomicBool = AtomicBool::new(false);

struct UpdateGuard;

impl UpdateGuard {
    fn acquire() -> CmdResult<Self> {
        UPDATE_IN_PROGRESS
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .map_err(|_| ThermalError::InvalidValue("an update is already running".into()))?;
        Ok(Self)
    }
}

impl Drop for UpdateGuard {
    fn drop(&mut self) {
        UPDATE_IN_PROGRESS.store(false, Ordering::SeqCst);
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateInfo {
    pub available: bool,
    pub current_version: String,
    pub latest_version: String,
    pub release_url: String,
}

#[derive(Debug, Deserialize)]
struct GithubRelease {
    tag_name: String,
    html_url: String,
}

#[tauri::command]
pub async fn check_for_update(app: AppHandle) -> CmdResult<UpdateInfo> {
    let current_version = app.package_info().version.to_string();
    let release = latest_release().await?;
    Ok(update_info(current_version, release))
}

async fn latest_release() -> CmdResult<GithubRelease> {
    let url = format!("https://api.github.com/repos/{REPO}/releases/latest");
    let client = reqwest::Client::builder()
        .user_agent("thermal-berry-update-checker")
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .map_err(|e| ThermalError::Network(e.to_string()))?;

    client
        .get(url)
        .send()
        .await
        .map_err(|e| ThermalError::Network(e.to_string()))?
        .error_for_status()
        .map_err(|e| ThermalError::Network(e.to_string()))?
        .json()
        .await
        .map_err(|e| ThermalError::Network(e.to_string()))
}

fn update_info(current_version: String, release: GithubRelease) -> UpdateInfo {
    let latest_version = release.tag_name.trim_start_matches('v').to_string();
    let available = match (
        parse_version(&latest_version),
        parse_version(&current_version),
    ) {
        (Some(latest), Some(current)) => latest > current,
        _ => false,
    };

    UpdateInfo {
        available,
        current_version,
        latest_version,
        release_url: release.html_url,
    }
}

#[tauri::command]
pub async fn install_update(app: AppHandle) -> CmdResult<()> {
    let guard = UpdateGuard::acquire()?;
    if cfg!(debug_assertions) {
        return Err(ThermalError::InvalidValue(
            "updates are only supported in installed release builds".into(),
        ));
    }

    let release = latest_release().await?;
    let tag = release.tag_name.clone();
    if !update_info(app.package_info().version.to_string(), release).available {
        return Err(ThermalError::InvalidValue(
            "no newer stable version is available".into(),
        ));
    }
    let appimage = app.env().appimage.map(PathBuf::from);

    tauri::async_runtime::spawn_blocking(move || {
        let _guard = guard;
        install_update_blocking(appimage, &tag)
    })
    .await
    .map_err(|e| ThermalError::InvalidValue(format!("update task failed: {e}")))??;

    app.request_restart();
    Ok(())
}

fn install_update_blocking(appimage: Option<PathBuf>, tag: &str) -> CmdResult<()> {
    let mut command = Command::new("bash");
    command.args(["-c", INSTALLER, "thermal-berry-installer"]);
    if let Some(path) = appimage {
        command.arg("--update-appimage").arg(path).arg(tag);
    } else {
        let executable = std::env::current_exe()
            .map_err(|e| ThermalError::from_io(std::path::Path::new("/proc/self/exe"), e))?;
        let owner = Command::new("dpkg-query")
            .arg("--search")
            .arg(&executable)
            .output()
            .map_err(|_| {
                ThermalError::InvalidValue(
                    "automatic updates support AppImage and .deb installations only".into(),
                )
            })?;
        if !owner.status.success()
            || !is_deb_owner(&String::from_utf8_lossy(&owner.stdout), &executable)
        {
            return Err(ThermalError::InvalidValue(
                "this executable is not installed by the thermal-berry .deb package".into(),
            ));
        }
        command.args(["--update-deb", tag]);
    }

    let output = command
        .output()
        .map_err(|e| ThermalError::from_io(std::path::Path::new("bash"), e))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let message = stderr
            .lines()
            .rev()
            .find_map(|line| line.strip_prefix("Error: "))
            .unwrap_or("the installer failed; check curl, bash and system package tools");
        return Err(ThermalError::InvalidValue(message.into()));
    }
    Ok(())
}

fn is_deb_owner(output: &str, executable: &std::path::Path) -> bool {
    output.lines().any(|line| {
        line.split_once(": ").is_some_and(|(package, path)| {
            (package == "thermal-berry" || package == "thermal-berry:amd64")
                && std::path::Path::new(path) == executable
        })
    })
}

fn parse_version(v: &str) -> Option<(u32, u32, u32)> {
    let mut parts = v.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch = parts.next()?.parse().ok()?;
    if parts.next().is_some() {
        return None;
    }
    Some((major, minor, patch))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_well_formed_versions() {
        assert_eq!(parse_version("1.2.3"), Some((1, 2, 3)));
        assert_eq!(parse_version("0.1.0"), Some((0, 1, 0)));
    }

    #[test]
    fn rejects_malformed_versions() {
        assert_eq!(parse_version("1.2"), None);
        assert_eq!(parse_version("abc"), None);
        assert_eq!(parse_version(""), None);
        assert_eq!(parse_version("1.2.3.4"), None);
        assert_eq!(parse_version("1.2.3-beta"), None);
    }

    #[test]
    fn compares_versions_numerically_not_lexically() {
        assert!(parse_version("1.10.0") > parse_version("1.9.0"));
        assert!(parse_version("2.0.0") > parse_version("1.99.99"));
        assert!(parse_version("1.0.0") == parse_version("1.0.0"));
    }

    #[test]
    fn update_is_available_only_for_a_newer_stable_release() {
        for (tag, available) in [
            ("v1.10.0", true),
            ("v1.9.0", false),
            ("v1.8.0", false),
            ("v1.10.0-beta", false),
            ("v1.10.0.1", false),
        ] {
            let info = update_info(
                "1.9.0".into(),
                GithubRelease {
                    tag_name: tag.into(),
                    html_url: "https://github.com/ramirozein/thermal-berry/releases".into(),
                },
            );
            assert_eq!(info.available, available, "{tag}");
        }
    }

    #[test]
    fn deb_detection_requires_this_executable_to_belong_to_our_package() {
        let executable = std::path::Path::new("/usr/bin/thermal-berry");
        assert!(is_deb_owner(
            "thermal-berry: /usr/bin/thermal-berry\n",
            executable
        ));
        assert!(is_deb_owner(
            "thermal-berry:amd64: /usr/bin/thermal-berry\n",
            executable
        ));
        assert!(!is_deb_owner(
            "other-package: /usr/bin/thermal-berry\n",
            executable
        ));
        assert!(!is_deb_owner("thermal-berry: /usr/bin/other\n", executable));
    }

    #[test]
    fn concurrent_updates_are_rejected_and_failure_allows_retry() {
        let guard = UpdateGuard::acquire().unwrap();
        assert!(UpdateGuard::acquire().is_err());
        drop(guard);
        assert!(UpdateGuard::acquire().is_ok());
    }
}
