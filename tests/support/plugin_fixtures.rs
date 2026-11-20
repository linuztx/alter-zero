//! Plugin marketplaces built on disk for tests: a marketplace in Claude
//! Code's or Codex's layout, as a folder or as a git repository, holding
//! plugins with the components a test asks for, plus repositories that hold
//! one plugin for a marketplace to point at.
//!
//! Test support only, shared through `#[path]` by the integration tests
//! that drive the plugin store and the `plugin` CLI. Everything is local:
//! repositories are cloned by `file://` URL, every git command carries its
//! own identity (nothing depends on the global git config), and an origin
//! serves any commit by id the way GitHub does, so a pinned `sha` can be
//! fetched. See `docs/plugins.md`.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{Value, json};

/// Run `git` in `dir` with a fixed identity and no signing; its trimmed
/// stdout. Panics when git fails.
pub fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args([
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.invalid",
        ])
        .args([
            "-c",
            "init.defaultBranch=main",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .output()
        .expect("git runs");
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// Write `text` at `root/relative`, creating the folders.
pub fn write(root: &Path, relative: &str, text: &str) {
    let path = root.join(relative);
    fs::create_dir_all(path.parent().expect("a parent")).expect("folders");
    fs::write(path, text).expect("write");
}

/// Make `dir` a repository with everything in it committed, serving any
/// commit by id. Returns the commit.
pub fn init_repo(dir: &Path) -> String {
    git(dir, &["init", "-q"]);
    git(dir, &["config", "uploadpack.allowAnySHA1InWant", "true"]);
    commit_all(dir, "initial")
}

/// Commit everything in the repository `dir`; the new commit.
pub fn commit_all(dir: &Path, message: &str) -> String {
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-qm", message, "--allow-empty"]);
    git(dir, &["rev-parse", "HEAD"])
}

/// `file://` URL of a local repository.
pub fn file_url(dir: &Path) -> String {
    format!("file://{}", dir.display())
}

/// Which tool's layout a marketplace or a plugin is written in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layout {
    /// `.claude-plugin/marketplace.json`, `.claude-plugin/plugin.json`,
    /// `hooks/hooks.json`, `${CLAUDE_PLUGIN_ROOT}` paths.
    Claude,
    /// `.agents/plugins/marketplace.json` (no descriptions in it),
    /// `.codex-plugin/plugin.json`, a root `hooks.json`, `./`-relative
    /// commands run from the plugin root.
    Codex,
}

/// One plugin to write: its manifest fields and its components.
#[derive(Debug, Clone, Default)]
pub struct Plugin {
    pub name: String,
    pub description: Option<String>,
    pub version: Option<String>,
    pub skills: Vec<String>,
    pub commands: Vec<String>,
    pub agents: Vec<String>,
    /// A `PreToolUse · Bash` hook.
    pub hook: bool,
    /// One stdio MCP server, `db`.
    pub mcp: bool,
}

impl Plugin {
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            description: Some(format!("The {name} plugin")),
            ..Self::default()
        }
    }

    pub fn version(mut self, version: &str) -> Self {
        self.version = Some(version.to_string());
        self
    }

    pub fn skill(mut self, name: &str) -> Self {
        self.skills.push(name.to_string());
        self
    }

    pub fn command(mut self, name: &str) -> Self {
        self.commands.push(name.to_string());
        self
    }

    pub fn agent(mut self, name: &str) -> Self {
        self.agents.push(name.to_string());
        self
    }

    pub fn hook(mut self) -> Self {
        self.hook = true;
        self
    }

    pub fn mcp(mut self) -> Self {
        self.mcp = true;
        self
    }

    /// Write the plugin into `dir` in `layout`.
    pub fn write_to(&self, dir: &Path, layout: Layout) {
        let mut manifest = json!({ "name": self.name });
        if let Some(version) = &self.version {
            manifest["version"] = json!(version);
        }
        match layout {
            Layout::Claude => {
                if let Some(description) = &self.description {
                    manifest["description"] = json!(description);
                }
                write(dir, ".claude-plugin/plugin.json", &manifest.to_string());
            }
            Layout::Codex => {
                manifest["skills"] = json!("./skills/");
                if let Some(description) = &self.description {
                    manifest["interface"] = json!({ "shortDescription": description });
                }
                write(dir, ".codex-plugin/plugin.json", &manifest.to_string());
            }
        }
        for skill in &self.skills {
            write(
                dir,
                &format!("skills/{skill}/SKILL.md"),
                &format!("---\nname: {skill}\ndescription: the {skill} skill\n---\nDo {skill}.\n"),
            );
        }
        for command in &self.commands {
            write(
                dir,
                &format!("commands/{command}.md"),
                &format!("---\ndescription: run {command}\n---\nRun {command} on $ARGUMENTS.\n"),
            );
        }
        for agent in &self.agents {
            write(
                dir,
                &format!("agents/{agent}.md"),
                &format!(
                    "---\nname: {agent}\ndescription: the {agent} agent\n---\nYou are {agent}.\n"
                ),
            );
        }
        if self.hook {
            let (file, command) = match layout {
                Layout::Claude => ("hooks/hooks.json", "${CLAUDE_PLUGIN_ROOT}/check.sh"),
                Layout::Codex => ("hooks.json", "./check.sh"),
            };
            let hooks = json!({"hooks": {"PreToolUse": [
                {"matcher": "Bash", "hooks": [{"type": "command", "command": command}]}
            ]}});
            write(dir, file, &hooks.to_string());
            write(dir, "check.sh", "#!/bin/sh\nexit 0\n");
        }
        if self.mcp {
            let mcp = json!({"mcpServers": {"db": {"command": "${CLAUDE_PLUGIN_ROOT}/db", "args": ["--serve"]}}});
            write(dir, ".mcp.json", &mcp.to_string());
        }
    }
}

/// A marketplace being built in a folder.
#[derive(Debug, Clone)]
pub struct Marketplace {
    pub root: PathBuf,
    pub name: String,
    pub layout: Layout,
    entries: Vec<Value>,
    renames: Vec<(String, String)>,
}

impl Marketplace {
    /// An empty marketplace named `name` at `root` (created).
    pub fn new(root: &Path, name: &str, layout: Layout) -> Self {
        fs::create_dir_all(root).expect("marketplace folder");
        Self {
            root: root.to_path_buf(),
            name: name.to_string(),
            layout,
            entries: Vec::new(),
            renames: Vec::new(),
        }
    }

    /// A plugin written under `plugins/<name>` and listed with a relative
    /// source — the description in the entry for Claude, only in the
    /// plugin's own manifest for Codex (as OpenAI's marketplace does).
    pub fn in_repo(&mut self, plugin: &Plugin) -> &mut Self {
        plugin.write_to(&self.plugin_dir(&plugin.name), self.layout);
        let path = format!("./plugins/{}", plugin.name);
        let entry = match self.layout {
            Layout::Claude => {
                json!({"name": plugin.name, "source": path, "description": plugin.description})
            }
            Layout::Codex => json!({
                "name": plugin.name,
                "source": {"source": "local", "path": path},
                "policy": {"installation": "AVAILABLE"},
                "category": "Developer Tools"
            }),
        };
        self.entry(entry)
    }

    /// Any entry, as written.
    pub fn entry(&mut self, entry: Value) -> &mut Self {
        self.entries.push(entry);
        self
    }

    /// A `renames` record: `old` is now `new`.
    pub fn rename(&mut self, old: &str, new: &str) -> &mut Self {
        self.renames.push((old.to_string(), new.to_string()));
        self
    }

    /// Change one listed entry in place.
    pub fn edit_entry(&mut self, name: &str, change: impl FnOnce(&mut Value)) -> &mut Self {
        let entry = self
            .entries
            .iter_mut()
            .find(|entry| entry["name"] == name)
            .expect("a listed entry");
        change(entry);
        self
    }

    /// Where an in-repo plugin's folder is.
    pub fn plugin_dir(&self, name: &str) -> PathBuf {
        self.root.join("plugins").join(name)
    }

    /// Write the manifest as it now stands.
    pub fn save(&self) -> &Self {
        let mut manifest = json!({ "name": self.name, "plugins": self.entries });
        if !self.renames.is_empty() {
            let renames: serde_json::Map<String, Value> = self
                .renames
                .iter()
                .map(|(old, new)| (old.clone(), json!(new)))
                .collect();
            manifest["renames"] = Value::Object(renames);
        }
        let file = match self.layout {
            Layout::Claude => {
                manifest["owner"] = json!({"name": "Fixture"});
                ".claude-plugin/marketplace.json"
            }
            Layout::Codex => {
                manifest["interface"] = json!({"displayName": format!("{} (Codex)", self.name)});
                ".agents/plugins/marketplace.json"
            }
        };
        write(
            &self.root,
            file,
            &serde_json::to_string_pretty(&manifest).expect("json"),
        );
        self
    }

    /// Save, make the folder a repository and commit; the commit.
    pub fn init_git(&self) -> String {
        self.save();
        init_repo(&self.root)
    }

    /// Save and commit; the commit.
    pub fn commit(&self, message: &str) -> String {
        self.save();
        commit_all(&self.root, message)
    }

    pub fn url(&self) -> String {
        file_url(&self.root)
    }
}

/// A repository holding one plugin — at its root, or in `subdir` — with two
/// commits: the plugin as given, then the same plugin at version `2.0.0`.
/// Returns the two commits.
pub fn plugin_repo(
    root: &Path,
    plugin: &Plugin,
    subdir: Option<&str>,
    layout: Layout,
) -> [String; 2] {
    fs::create_dir_all(root).expect("repository folder");
    let dir = subdir.map_or_else(|| root.to_path_buf(), |subdir| root.join(subdir));
    plugin.write_to(&dir, layout);
    write(root, "README.md", "a plugin repository\n");
    let first = init_repo(root);
    plugin.clone().version("2.0.0").write_to(&dir, layout);
    let second = commit_all(root, "version 2");
    [first, second]
}
