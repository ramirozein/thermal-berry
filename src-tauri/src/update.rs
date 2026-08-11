use serde::{Deserialize, Serialize};
use tauri::AppHandle;

use crate::thermal::ThermalError;

type CmdResult<T> = Result<T, ThermalError>;

const REPO: &str = "ramirozein/thermal-berry";

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
    let url = format!("https://api.github.com/repos/{REPO}/releases/latest");
    let client = reqwest::Client::builder()
        .user_agent("thermal-berry-update-checker")
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .map_err(|e| ThermalError::Network(e.to_string()))?;

    let release: GithubRelease = client
        .get(url)
        .send()
        .await
        .map_err(|e| ThermalError::Network(e.to_string()))?
        .error_for_status()
        .map_err(|e| ThermalError::Network(e.to_string()))?
        .json()
        .await
        .map_err(|e| ThermalError::Network(e.to_string()))?;

    let latest_version = release.tag_name.trim_start_matches('v').to_string();
    let available = match (parse_version(&latest_version), parse_version(&current_version)) {
        (Some(latest), Some(current)) => latest > current,
        _ => false,
    };

    Ok(UpdateInfo {
        available,
        current_version,
        latest_version,
        release_url: release.html_url,
    })
}

fn parse_version(v: &str) -> Option<(u32, u32, u32)> {
    let mut parts = v.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch = parts.next()?.parse().ok()?;
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
    }

    #[test]
    fn compares_versions_numerically_not_lexically() {
        assert!(parse_version("1.10.0") > parse_version("1.9.0"));
        assert!(parse_version("2.0.0") > parse_version("1.99.99"));
        assert!(parse_version("1.0.0") == parse_version("1.0.0"));
    }
}
