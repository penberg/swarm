pub mod exedev;
pub mod local;

use serde::Serialize;
use std::{fmt, path::Path};

use crate::{SplayError, repos::Repository, workspaces::Workspace};

/// Where a workspace's checkout lives and where its sessions run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum SandboxKind {
    #[serde(rename = "local")]
    Local,
    #[serde(rename = "exedev")]
    ExeDev,
}

impl SandboxKind {
    pub fn as_str(self) -> &'static str {
        match self {
            SandboxKind::Local => "local",
            SandboxKind::ExeDev => "exedev",
        }
    }

    pub fn parse(input: &str) -> Result<Self, SplayError> {
        match input {
            "local" => Ok(SandboxKind::Local),
            "exedev" => Ok(SandboxKind::ExeDev),
            _ => Err(SplayError::InvalidSandbox(input.to_string())),
        }
    }
}

impl fmt::Display for SandboxKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Paths of the repository's local bare clone.
pub struct RepoPaths<'a> {
    pub repo_dir: &'a Path,
    pub bare_repo_path: &'a Path,
}

/// Receives a short description of each creation step as it starts.
pub type Progress<'a> = &'a (dyn Fn(&str) + Send + Sync);

pub struct CreateRequest<'a> {
    pub repo: &'a Repository,
    pub paths: RepoPaths<'a>,
    /// Local directory reserved for the workspace.
    pub workspace_path: &'a Path,
    pub workspace_name: &'a str,
    pub default_branch: &'a str,
    pub progress: Progress<'a>,
}

pub struct Checkout {
    pub branch: String,
    /// Provider-specific identifier, such as a VM name.
    pub sandbox_ref: Option<String>,
}

pub trait Sandbox: Sync {
    /// Creates the workspace checkout on branch `workspace_name`.
    fn create(&self, request: &CreateRequest) -> Result<Checkout, SplayError>;

    /// Destroys the workspace checkout and everything it holds.
    fn destroy(&self, workspace: &Workspace, paths: &RepoPaths) -> Result<(), SplayError>;

    /// Returns the command the local session supervisor runs for `command`.
    fn session_command(
        &self,
        repo: &Repository,
        workspace: &Workspace,
        command: &[String],
    ) -> Result<Vec<String>, SplayError>;
}

pub fn sandbox(kind: SandboxKind) -> &'static dyn Sandbox {
    match kind {
        SandboxKind::Local => &local::Local,
        SandboxKind::ExeDev => &exedev::ExeDev,
    }
}
