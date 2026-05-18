use chrono::DateTime;
use chrono::Utc;
use serde::Deserialize;
use serde::Serialize;
use std::fs::File;
use std::fs::OpenOptions;
use std::io::Read;
use std::io::Write;
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use std::path::PathBuf;

const GITHUB_COPILOT_AUTH_FILE: &str = "github-copilot-auth.json";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GitHubCopilotAuth {
    pub github_access_token: String,
    pub copilot_access_token: String,
    pub copilot_token_expires_at: DateTime<Utc>,
    pub api_base_url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enterprise_domain: Option<String>,
    pub saved_at: DateTime<Utc>,
}

impl GitHubCopilotAuth {
    pub fn new(
        github_access_token: String,
        copilot_access_token: String,
        copilot_token_expires_at: DateTime<Utc>,
        api_base_url: String,
        enterprise_domain: Option<String>,
    ) -> Self {
        Self {
            github_access_token,
            copilot_access_token,
            copilot_token_expires_at,
            api_base_url,
            enterprise_domain,
            saved_at: Utc::now(),
        }
    }
}

pub fn github_copilot_auth_file(codex_home: &Path) -> PathBuf {
    codex_home.join(GITHUB_COPILOT_AUTH_FILE)
}

pub fn load_github_copilot_auth(codex_home: &Path) -> std::io::Result<Option<GitHubCopilotAuth>> {
    let path = github_copilot_auth_file(codex_home);
    let mut file = match File::open(&path) {
        Ok(file) => file,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err),
    };
    let mut contents = String::new();
    file.read_to_string(&mut contents)?;
    serde_json::from_str(&contents)
        .map(Some)
        .map_err(std::io::Error::other)
}

pub fn save_github_copilot_auth(
    codex_home: &Path,
    auth: &GitHubCopilotAuth,
) -> std::io::Result<()> {
    let path = github_copilot_auth_file(codex_home);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let json = serde_json::to_string_pretty(auth).map_err(std::io::Error::other)?;
    let mut options = OpenOptions::new();
    options.create(true).truncate(true).write(true);
    #[cfg(unix)]
    {
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(json.as_bytes())?;
    file.flush()
}

pub fn delete_github_copilot_auth(codex_home: &Path) -> std::io::Result<bool> {
    let path = github_copilot_auth_file(codex_home);
    match std::fs::remove_file(path) {
        Ok(()) => Ok(true),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(err) => Err(err),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeDelta;
    use pretty_assertions::assert_eq;

    #[test]
    fn save_load_and_delete_github_copilot_auth() {
        let temp_dir = tempfile::tempdir().expect("temp dir should be created");
        let auth = GitHubCopilotAuth::new(
            "github-token".to_string(),
            "copilot-token".to_string(),
            Utc::now() + TimeDelta::minutes(30),
            "https://api.githubcopilot.com".to_string(),
            Some("github.example.com".to_string()),
        );

        save_github_copilot_auth(temp_dir.path(), &auth).expect("auth should save");
        assert_eq!(
            load_github_copilot_auth(temp_dir.path()).expect("auth should load"),
            Some(auth)
        );
        assert!(delete_github_copilot_auth(temp_dir.path()).expect("auth should delete"));
        assert_eq!(
            load_github_copilot_auth(temp_dir.path()).expect("missing auth should load"),
            None
        );
        assert!(!delete_github_copilot_auth(temp_dir.path()).expect("delete should be idempotent"));
    }
}
