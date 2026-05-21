use clap::Parser;
use std::path::PathBuf;

#[derive(Debug, Parser)]
pub struct AppCommand {
    /// Workspace path to open in Codex Desktop.
    #[arg(value_name = "PATH", default_value = ".")]
    pub path: PathBuf,

    /// Override the app installer download URL (advanced).
    #[arg(long = "download-url")]
    pub download_url_override: Option<String>,
}

pub async fn run_app(cmd: AppCommand, codepilot_cli_path: Option<PathBuf>) -> anyhow::Result<()> {
    #[cfg(not(target_os = "windows"))]
    let _ = codepilot_cli_path;

    let workspace = std::fs::canonicalize(&cmd.path).unwrap_or(cmd.path);
    #[cfg(target_os = "macos")]
    {
        crate::desktop_app::run_app_open_or_install(workspace, cmd.download_url_override).await
    }
    #[cfg(target_os = "windows")]
    {
        crate::desktop_app::run_app_open_or_install(
            workspace,
            cmd.download_url_override,
            codepilot_cli_path,
        )
        .await
    }
}
