//! Environment abstraction so discovery can be driven from real filesystem
//! state in production and from in-memory fixtures in tests.

use std::path::Path;
use std::path::PathBuf;

/// Filesystem and environment accessor used by every discovery source.
///
/// Implementations must be cheap to clone or borrow; the discovery pipeline
/// holds a `&dyn ExternalMcpEnv` and never tries to mutate state through this
/// trait. Tests should implement this trait against a `TempDir` rather than
/// shelling out or reading the real user home.
pub trait ExternalMcpEnv: Send + Sync {
    /// Current working directory used as the starting point for upward `.mcp.json`
    /// walks and for `${workspaceFolder}` expansion.
    fn cwd(&self) -> &Path;

    /// Home directory of the invoking user. May return `None` on hosts where
    /// the home directory cannot be resolved.
    fn home_dir(&self) -> Option<PathBuf>;

    /// Codex configuration directory, mirroring [`codex_utils_home_dir::find_codex_home`].
    /// Used to locate the `mcp-discovery/own/` overrides and the consent store.
    fn codex_home(&self) -> Option<PathBuf>;

    /// Resolve an environment variable. Returns `None` for unset or invalid
    /// values; callers should treat both cases the same.
    fn env_var(&self, key: &str) -> Option<String>;

    /// True when the path exists on disk (file or directory).
    fn path_exists(&self, path: &Path) -> bool;

    /// Read a file as UTF-8. Returns the same error kinds as [`std::fs::read_to_string`].
    fn read_to_string(&self, path: &Path) -> std::io::Result<String>;

    /// Enumerate immediate child entries of `dir`. Order is unspecified; callers
    /// should sort if they need determinism.
    fn read_dir(&self, dir: &Path) -> std::io::Result<Vec<PathBuf>>;

    /// True when the path resolves to a directory.
    fn is_dir(&self, path: &Path) -> bool;

    /// Resolve an executable on `PATH`. Used to detect Agency or self-referential
    /// commands. The default implementation defers to [`which::which`].
    fn which(&self, program: &str) -> Option<PathBuf> {
        which::which(program).ok()
    }
}

/// Production [`ExternalMcpEnv`] backed by the real filesystem and process env.
#[derive(Debug, Clone)]
pub struct RealExternalMcpEnv {
    cwd: PathBuf,
    home_dir: Option<PathBuf>,
    codex_home: Option<PathBuf>,
}

impl RealExternalMcpEnv {
    /// Capture the current process state. The constructor is cheap: it only
    /// reads `cwd`, the home directory, and the resolved `CODEX_HOME`.
    pub fn new() -> std::io::Result<Self> {
        let cwd = std::env::current_dir()?;
        let home_dir = dirs::home_dir();
        let codex_home = codex_utils_home_dir::find_codex_home()
            .ok()
            .map(|abs| abs.as_path().to_path_buf());
        Ok(Self {
            cwd,
            home_dir,
            codex_home,
        })
    }

    /// Construct directly from already-resolved paths. Helpful when the
    /// embedder wants to override `cwd` (e.g. for a per-session working
    /// directory) without disturbing the discovery contract.
    pub fn from_parts(
        cwd: PathBuf,
        home_dir: Option<PathBuf>,
        codex_home: Option<PathBuf>,
    ) -> Self {
        Self {
            cwd,
            home_dir,
            codex_home,
        }
    }
}

impl ExternalMcpEnv for RealExternalMcpEnv {
    fn cwd(&self) -> &Path {
        &self.cwd
    }

    fn home_dir(&self) -> Option<PathBuf> {
        self.home_dir.clone()
    }

    fn codex_home(&self) -> Option<PathBuf> {
        self.codex_home.clone()
    }

    fn env_var(&self, key: &str) -> Option<String> {
        std::env::var(key).ok().filter(|val| !val.is_empty())
    }

    fn path_exists(&self, path: &Path) -> bool {
        path.exists()
    }

    fn read_to_string(&self, path: &Path) -> std::io::Result<String> {
        std::fs::read_to_string(path)
    }

    fn read_dir(&self, dir: &Path) -> std::io::Result<Vec<PathBuf>> {
        let mut entries = Vec::new();
        for entry in std::fs::read_dir(dir)? {
            entries.push(entry?.path());
        }
        Ok(entries)
    }

    fn is_dir(&self, path: &Path) -> bool {
        path.is_dir()
    }
}
