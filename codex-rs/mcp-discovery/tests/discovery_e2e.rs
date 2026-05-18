//! End-to-end discovery tests against an in-process fake [`ExternalMcpEnv`]
//! backed by a temporary directory tree. These tests cover the full
//! orchestrator: per-source scanning, name/content dedup, and consent
//! decisions.

use std::collections::HashMap;
use std::path::Path;
use std::path::PathBuf;

use codex_mcp_discovery::ConsentDecision;
use codex_mcp_discovery::ConsentRecord;
use codex_mcp_discovery::ConsentStore;
use codex_mcp_discovery::ExternalMcpEnv;
use codex_mcp_discovery::ExternalMcpSource;
use codex_mcp_discovery::ReservedNames;
use codex_mcp_discovery::SelfReferenceConfig;
use codex_mcp_discovery::ShadowReason;
use codex_mcp_discovery::discover_all;
use pretty_assertions::assert_eq;
use tempfile::TempDir;

#[derive(Default)]
struct FakeEnv {
    cwd: PathBuf,
    home_dir: Option<PathBuf>,
    codex_home: Option<PathBuf>,
    vars: HashMap<String, String>,
    which: HashMap<String, PathBuf>,
}

impl ExternalMcpEnv for FakeEnv {
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
        self.vars.get(key).cloned()
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

    fn which(&self, program: &str) -> Option<PathBuf> {
        self.which.get(program).cloned()
    }
}

fn write_file(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap_or_else(|err| panic!("create parent: {err}"));
    }
    std::fs::write(path, contents).unwrap_or_else(|err| panic!("write fixture: {err}"));
}

#[test]
fn discovers_servers_from_every_source_with_expected_precedence() {
    let temp = TempDir::new().expect("temp dir");
    let root = temp.path();
    let project = root.join("project");
    let home = root.join("home");
    let codex_home = home.join(".codex");
    let agency_exe = home.join("bin").join("agency");

    // Own override wins over CopilotCli for `github`.
    write_file(
        &codex_home
            .join("mcp-discovery")
            .join("own")
            .join("mcp.json"),
        r#"{
  "mcpServers": {
    "github": { "command": "python", "args": ["-m", "github_own"] },
    "legacy": false
  }
}
"#,
    );

    // Claude project file at cwd defines `notes`, and explicitly disables `legacy`.
    write_file(
        &project.join(".mcp.json"),
        r#"{
  "mcpServers": {
    "notes": { "command": "node", "args": ["notes.js"] },
    "legacy": false
  }
}
"#,
    );

    // Copilot CLI: `github` should be shadowed by the Own entry above.
    // `linker` defines an entry that is a content duplicate of the VS Code one.
    write_file(
        &home.join(".copilot").join("mcp-config.json"),
        r#"{
  "mcpServers": {
    "github": { "command": "python", "args": ["-m", "github_cli"] },
    "linker": { "command": "python", "args": ["-m", "linker"] }
  }
}
"#,
    );

    // Copilot plugin: provides one new server.
    write_file(
        &home
            .join(".copilot")
            .join("installed-plugins")
            .join("copilot-plugins")
            .join("workiq")
            .join(".mcp.json"),
        r#"{
  "mcpServers": {
    "workiq": { "command": "python", "args": ["-m", "workiq"] }
  }
}
"#,
    );

    // VS Code config: workspace+env expansion, duplicates `linker` by content.
    write_file(
        &project.join(".vscode").join("mcp.json"),
        r#"{
  "servers": {
    "vsc-server": {
      "type": "stdio",
      "command": "${workspaceFolder}/bin/server",
      "args": ["--token", "${env:VSC_TOKEN}"]
    },
    "linker-alt": {
      "type": "stdio",
      "command": "python",
      "args": ["-m", "linker"]
    }
  }
}
"#,
    );

    // Agency: builtin entry that resolves to `agency mcp kusto --transport stdio`.
    write_file(
        &home.join(".agency").join("agency.toml"),
        r#"
[mcps.builtins]
kusto = true
m365 = { database = "msft", type = "graph" }
"#,
    );
    write_file(&agency_exe, "");

    let mut which = HashMap::new();
    which.insert("agency".to_string(), agency_exe);
    let mut vars = HashMap::new();
    vars.insert("VSC_TOKEN".to_string(), "secret".to_string());
    let env = FakeEnv {
        cwd: project,
        home_dir: Some(home),
        codex_home: Some(codex_home),
        vars,
        which,
    };

    let reserved = ReservedNames::from_entries([codex_mcp_discovery::ReservedName {
        name: "notes",
        owner: "config.toml",
    }]);
    let report = discover_all(&env, &reserved, &SelfReferenceConfig::default());

    let names: Vec<&str> = report.servers.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(
        names,
        vec!["github", "linker", "workiq", "vsc-server", "kusto", "m365"]
    );

    let github = report
        .servers
        .iter()
        .find(|s| s.name == "github")
        .expect("github");
    assert_eq!(github.source, ExternalMcpSource::Own);

    let shadows_by_name: HashMap<String, ShadowReason> = report
        .shadows
        .iter()
        .map(|s| (s.name.clone(), s.reason.clone()))
        .collect();
    assert_eq!(
        shadows_by_name.get("github"),
        Some(&ShadowReason::NameCollision {
            winner_source: ExternalMcpSource::Own.label().to_string(),
        })
    );
    assert_eq!(
        shadows_by_name.get("linker-alt"),
        Some(&ShadowReason::ContentDuplicate {
            winner_name: "linker".to_string(),
        })
    );
    assert_eq!(
        shadows_by_name.get("notes"),
        Some(&ShadowReason::NameCollision {
            winner_source: "config.toml".to_string(),
        })
    );
    assert_eq!(
        shadows_by_name.get("legacy"),
        None,
        "explicitly disabled name should not appear since no lower-priority entry tried to define it"
    );
}

#[test]
fn consent_store_treats_own_as_trusted_and_persists_external() {
    let temp = TempDir::new().expect("temp dir");
    let codex_home = temp.path().to_path_buf();
    let project = temp.path().join("project");

    write_file(
        &codex_home
            .join("mcp-discovery")
            .join("own")
            .join("mcp.json"),
        r#"{
  "mcpServers": {
    "own-server": { "command": "python", "args": ["-m", "own"] }
  }
}
"#,
    );
    write_file(
        &project.join(".mcp.json"),
        r#"{
  "mcpServers": {
    "external-server": { "command": "python", "args": ["-m", "external"] }
  }
}
"#,
    );

    let env = FakeEnv {
        cwd: project,
        home_dir: Some(temp.path().to_path_buf()),
        codex_home: Some(codex_home.clone()),
        vars: HashMap::new(),
        which: HashMap::new(),
    };
    let report = discover_all(
        &env,
        &ReservedNames::default(),
        &SelfReferenceConfig::default(),
    );
    let own = report
        .servers
        .iter()
        .find(|s| s.name == "own-server")
        .expect("own server");
    let external = report
        .servers
        .iter()
        .find(|s| s.name == "external-server")
        .expect("external server");

    let mut store = ConsentStore::with_record(
        codex_home.join("mcp-consent.json"),
        ConsentRecord::default(),
    );
    assert_eq!(store.decide(own), ConsentDecision::Approved);
    assert_eq!(store.decide(external), ConsentDecision::Pending);

    store.approve("external-server").expect("approve");
    assert_eq!(store.decide(external), ConsentDecision::Approved);

    let reloaded = ConsentStore::load(&codex_home);
    assert_eq!(reloaded.decide(external), ConsentDecision::Approved);
}
