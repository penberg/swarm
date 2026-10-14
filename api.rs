//! The protocol between `splay server` and its clients.
//!
//! A client connects to the server's Unix socket, writes one [`Envelope`] as
//! a line of JSON, and reads [`Reply`] lines until it gets a final one:
//! [`Reply::Ok`], [`Reply::Error`], or [`Reply::BuildMismatch`]. Long
//! operations send [`Reply::Progress`] lines before their final reply.
//!
//! Replies carry the stores' own types, such as [`Workspace`], and the types
//! below that describe what clients show, such as the sidebar's
//! [`WorkspaceGroup`]s.

use serde::{Deserialize, Serialize};
use std::{
    path::{Path, PathBuf},
    sync::OnceLock,
};

use crate::{
    SELF_EXE, SplayError, sandbox::SandboxKind, sessions::Session, templates::TemplateStatus,
    workspaces::Workspace,
};

#[derive(Debug, Serialize, Deserialize)]
pub struct Envelope {
    /// The client's [`build_id`]. The server refuses requests from other
    /// builds, so that a client never talks to a stale server.
    pub build: String,
    pub request: Request,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "method", content = "params", rename_all = "snake_case")]
pub enum Request {
    Status,
    Shutdown,

    RepoAdd {
        repository: String,
        alias: Option<String>,
    },
    RepoSync {
        repository: String,
    },
    RepoRemove {
        repository: String,
    },
    RepoArchive {
        repository: String,
    },
    RepoUnarchive {
        repository: String,
    },
    RepoList,
    RepoSetCollapsed {
        repository: String,
        collapsed: bool,
    },
    RepoSetAllCollapsed {
        collapsed: bool,
    },
    RepoSettings {
        repository: String,
    },
    RepoSetSetupScript {
        repository: String,
        script: String,
    },
    RepoSetSandboxEnabled {
        repository: String,
        sandbox: SandboxKind,
        enabled: bool,
    },
    TemplateStatus {
        repository: String,
        sandbox: SandboxKind,
        rebuild: bool,
    },
    TemplateLog {
        repository: String,
        sandbox: SandboxKind,
    },
    TemplateStatuses {
        repositories: Vec<(String, Vec<SandboxKind>)>,
    },

    WorkspaceCreate {
        repository: String,
        name: Option<String>,
        sandbox: SandboxKind,
    },
    WorkspaceClone {
        workspace: String,
        name: String,
    },
    WorkspaceList {
        repository: String,
    },
    WorkspaceInfo {
        workspace: String,
    },
    WorkspaceRename {
        workspace: String,
        name: String,
    },
    WorkspaceRemove {
        workspace: String,
        /// Stop and remove the workspace's sessions first.
        stop_sessions: bool,
    },
    WorkspaceMarkUsed {
        repository: String,
        workspace: String,
    },

    SessionCreate {
        workspace: String,
        /// The default shell when `None`.
        command: Option<Vec<String>>,
    },
    SessionList {
        workspace: Option<String>,
    },
    SessionInfo {
        session: String,
    },
    SessionStop {
        session: String,
    },
    SessionRemove {
        session: String,
    },
    /// Stops and removes a session.
    SessionClose {
        session: String,
    },

    PruneSessions,
    PruneWorkspaces,

    /// Everything the sidebar shows.
    Sidebar,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Reply {
    Progress(String),
    Ok(serde_json::Value),
    Error(WireError),
    /// The server runs a different build than the client; carries the
    /// server's build.
    BuildMismatch(String),
}

/// An error as it crosses the socket. Variants that clients match on keep
/// their kind; everything else becomes a message.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum WireError {
    RepositoryNotFound(String),
    WorkspaceNotFound(String),
    SessionNotFound(String),
    SessionRunning(String),
    PruneIncomplete(usize),
    Other(String),
}

impl From<&SplayError> for WireError {
    fn from(err: &SplayError) -> Self {
        match err {
            SplayError::RepositoryNotFound(value) => WireError::RepositoryNotFound(value.clone()),
            SplayError::WorkspaceNotFound(value) => WireError::WorkspaceNotFound(value.clone()),
            SplayError::SessionNotFound(value) => WireError::SessionNotFound(value.clone()),
            SplayError::SessionRunning(value) => WireError::SessionRunning(value.clone()),
            SplayError::PruneIncomplete(count) => WireError::PruneIncomplete(*count),
            other => WireError::Other(other.to_string()),
        }
    }
}

impl From<WireError> for SplayError {
    fn from(err: WireError) -> Self {
        match err {
            WireError::RepositoryNotFound(value) => SplayError::RepositoryNotFound(value),
            WireError::WorkspaceNotFound(value) => SplayError::WorkspaceNotFound(value),
            WireError::SessionNotFound(value) => SplayError::SessionNotFound(value),
            WireError::SessionRunning(value) => SplayError::SessionRunning(value),
            WireError::PruneIncomplete(count) => SplayError::PruneIncomplete(count),
            WireError::Other(message) => SplayError::Server(message),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerStatus {
    pub pid: u32,
    pub build: String,
    pub data_dir: PathBuf,
}

/// A workspace that was just created, cloned or renamed, with its sessions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceWithSessions {
    pub workspace: Workspace,
    pub sessions: Vec<Session>,
    /// Why the workspace's first session failed to start, if it did.
    pub session_error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PruneSummary {
    pub pruned: Vec<String>,
    pub failed: Vec<PruneFailureSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PruneFailureSummary {
    pub name: String,
    pub path: PathBuf,
    pub error: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceGroup {
    pub repo_label: String,
    pub repo_canonical: String,
    pub repo_status: Option<String>,
    pub collapsed: bool,
    pub recently_used: bool,
    pub workspace_count: usize,
    pub workspaces: Vec<WorkspaceEntry>,
    /// Remote sandboxes enabled for the repository.
    pub remote_sandboxes: Vec<SandboxKind>,
    /// Templates of the enabled remote sandboxes.
    pub templates: Vec<(SandboxKind, TemplateStatus)>,
}

impl WorkspaceGroup {
    pub fn template(&self, sandbox: SandboxKind) -> Option<&TemplateStatus> {
        self.templates
            .iter()
            .find(|(kind, _)| *kind == sandbox)
            .map(|(_, status)| status)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceEntry {
    pub repo_canonical: String,
    pub name: String,
    pub branch: String,
    pub path: String,
    pub sandbox: SandboxKind,
    pub sessions: Vec<SessionEntry>,
    pub recently_used: bool,
    /// Set while the workspace is being created; such entries exist only in the UI.
    pub provisioning: Option<Provisioning>,
}

impl WorkspaceEntry {
    pub fn new(workspace: Workspace, sessions: &[Session]) -> Self {
        WorkspaceEntry {
            repo_canonical: workspace.repository,
            name: workspace.name,
            branch: workspace.branch,
            path: workspace.path.display().to_string(),
            sandbox: workspace.sandbox,
            sessions: sessions.iter().map(SessionEntry::new).collect(),
            provisioning: None,
            // Workspaces that were just created, cloned or renamed are in use.
            recently_used: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Provisioning {
    pub stage: String,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionEntry {
    pub id: String,
    pub pid: Option<u32>,
    pub program: String,
    pub status: String,
    pub log_path: String,
    pub socket_path: String,
}

impl SessionEntry {
    pub fn new(session: &Session) -> Self {
        SessionEntry {
            id: session.id.clone(),
            pid: session.pid,
            program: command_program(&session.command),
            status: session.status.clone(),
            log_path: session.log_path.display().to_string(),
            socket_path: session.socket_path.display().to_string(),
        }
    }
}

/// Read-only facts about a repository for its settings page.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepositoryDetails {
    pub remote_url: String,
    pub default_branch: Option<String>,
    pub mirror_path: String,
    pub alias: Option<String>,
}

/// Everything the repository settings page shows.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepositorySettings {
    pub details: RepositoryDetails,
    pub setup_script: String,
}

/// Identifies this build of Splay: its version and when its executable was
/// built. Clients and the server must agree on it.
pub fn build_id() -> &'static str {
    static BUILD_ID: OnceLock<String> = OnceLock::new();
    BUILD_ID.get_or_init(|| {
        let modified = std::fs::metadata(SELF_EXE)
            .and_then(|metadata| metadata.modified())
            .ok()
            .and_then(|modified| modified.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|elapsed| elapsed.as_nanos())
            .unwrap_or_default();
        format!("{}+{modified:x}", env!("CARGO_PKG_VERSION"))
    })
}

pub fn command_program(command: &[String]) -> String {
    command
        .first()
        .and_then(|arg| Path::new(arg).file_name())
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or("session")
        .to_string()
}
