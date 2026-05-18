//! Lossy normalization helpers used both by the fingerprint logic (for dedup)
//! and by the discovery sources themselves (for cross-OS comparability).

use std::path::Path;
use std::path::PathBuf;

use url::Url;

/// Normalize a command string for content-based deduplication.
///
/// The goal is to make `python`, `/usr/bin/python`, and `C:\\Python\\python.exe`
/// hash to the same value when they refer to the same executable. We:
///
/// - lowercase on Windows-style paths,
/// - strip a trailing `.exe`/`.cmd`/`.bat` extension,
/// - drop the directory portion (basename only).
///
/// This is intentionally permissive: it favors fewer false-negative duplicates
/// over rare false-positives.
pub fn normalize_exe(command: &str) -> String {
    let path = Path::new(command);
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(command);
    let lowered = file_name.to_ascii_lowercase();
    let stripped = lowered
        .strip_suffix(".exe")
        .or_else(|| lowered.strip_suffix(".cmd"))
        .or_else(|| lowered.strip_suffix(".bat"))
        .unwrap_or(&lowered);
    stripped.to_string()
}

/// Normalize a working directory for content-based deduplication. Returns the
/// absolute, lower-cased form when possible. Falls back to the raw string on
/// platforms where canonicalization fails (e.g. relative test fixtures).
pub fn normalize_cwd(cwd: &Path) -> String {
    let canonical: PathBuf = cwd.canonicalize().unwrap_or_else(|_| cwd.to_path_buf());
    canonical.to_string_lossy().to_ascii_lowercase()
}

/// Normalize an HTTP URL by collapsing scheme/host casing, dropping default
/// ports, and trimming a trailing `/` from the path. Query strings and fragments
/// are dropped because they typically carry per-session state.
pub fn normalize_url(raw: &str) -> String {
    let Ok(mut url) = Url::parse(raw) else {
        return raw.trim().to_ascii_lowercase();
    };
    url.set_fragment(None);
    url.set_query(None);

    let scheme = url.scheme().to_ascii_lowercase();
    let host = url.host_str().unwrap_or("").to_ascii_lowercase();
    let default_port_for_scheme = matches!(
        (scheme.as_str(), url.port()),
        ("http", Some(80)) | ("https", Some(443))
    );
    let port_segment = match (default_port_for_scheme, url.port()) {
        (true, _) | (_, None) => String::new(),
        (false, Some(port)) => format!(":{port}"),
    };
    let mut path = url.path().to_string();
    if path.len() > 1 && path.ends_with('/') {
        path.pop();
    }
    format!("{scheme}://{host}{port_segment}{path}")
}

/// Substitute `${workspaceFolder}` and `${env:VAR}` placeholders in a VS Code
/// config string. Missing env vars resolve to the empty string to match the
/// VS Code behavior our users expect.
pub fn expand_vscode_vars(value: &str, cwd: &Path, env: impl Fn(&str) -> Option<String>) -> String {
    let cwd_str = cwd.to_string_lossy().to_string();
    let mut output = String::with_capacity(value.len());
    let mut rest = value;
    while let Some(start) = rest.find("${") {
        output.push_str(&rest[..start]);
        let after_open = &rest[start + 2..];
        let Some(end) = after_open.find('}') else {
            output.push_str("${");
            rest = after_open;
            continue;
        };
        let token = &after_open[..end];
        let replacement = if token == "workspaceFolder" {
            cwd_str.clone()
        } else if let Some(name) = token.strip_prefix("env:") {
            env(name).unwrap_or_default()
        } else {
            format!("${{{token}}}")
        };
        output.push_str(&replacement);
        rest = &after_open[end + 1..];
    }
    output.push_str(rest);
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn normalize_exe_handles_windows_extensions_and_paths() {
        assert_eq!(normalize_exe("python"), "python");
        assert_eq!(normalize_exe("/usr/bin/python"), "python");
        assert_eq!(normalize_exe("C:\\Python\\python.exe"), "python");
        assert_eq!(normalize_exe("C:/Users/me/agency.CMD"), "agency");
    }

    #[test]
    fn normalize_url_strips_defaults_and_lowercases() {
        assert_eq!(
            normalize_url("HTTPS://Example.com:443/Foo/?bar=1"),
            "https://example.com/Foo"
        );
        assert_eq!(
            normalize_url("http://api.local:8080/v1/"),
            "http://api.local:8080/v1"
        );
        assert_eq!(normalize_url("not a url"), "not a url");
    }

    #[test]
    fn expand_vscode_vars_substitutes_workspace_and_env() {
        let cwd = Path::new("/home/me/project");
        let env = |name: &str| {
            if name == "TOKEN" {
                Some("abc".to_string())
            } else {
                None
            }
        };
        assert_eq!(
            expand_vscode_vars("${workspaceFolder}/bin/${env:TOKEN}", cwd, env),
            "/home/me/project/bin/abc"
        );
    }

    #[test]
    fn expand_vscode_vars_leaves_unknown_tokens_intact() {
        let env = |_: &str| None;
        assert_eq!(
            expand_vscode_vars("${unknownToken}/x", Path::new("/cwd"), env),
            "${unknownToken}/x"
        );
    }
}
