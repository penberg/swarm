//! What Splay shows of its state: the sidebar's repositories, workspaces and
//! sessions, and a repository's settings.

use std::path::Path;

use crate::{
    sandbox::SandboxKind, sessions::Session, templates::TemplateStatus, workspaces::Workspace,
};

#[derive(Debug, Clone)]
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

#[derive(Debug, Clone)]
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

#[derive(Debug, Clone)]
pub struct Provisioning {
    pub stage: String,
    pub error: Option<String>,
}

#[derive(Debug, Clone)]
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
#[derive(Debug, Clone)]
pub struct RepositoryDetails {
    pub remote_url: String,
    pub default_branch: Option<String>,
    pub mirror_path: String,
    pub alias: Option<String>,
}

/// Everything the repository settings page shows.
#[derive(Debug, Clone)]
pub struct RepositorySettings {
    pub details: RepositoryDetails,
    pub setup_script: String,
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
