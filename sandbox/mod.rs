pub mod exedev;
pub mod local;

use serde::Serialize;
use std::{fmt, fs::File, path::Path};

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
    /// Branch new local workspaces start from.
    pub default_branch: Option<&'a str>,
    /// Template that new remote workspaces are copied from.
    pub template: Option<&'a str>,
    pub progress: Progress<'a>,
}

pub struct TemplateRequest<'a> {
    pub repo: &'a Repository,
    /// Name of the template to build, from [`Sandbox::new_template_name`].
    pub name: &'a str,
    pub setup_script: Option<&'a str>,
    /// Receives the output of every build step.
    pub log: &'a File,
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

    /// Returns a fresh name for a template of `repo`.
    fn new_template_name(&self, repo: &Repository) -> String;

    /// Builds a template that new workspaces are copied from: the repository
    /// checkout with the setup script already run.
    fn build_template(&self, request: &TemplateRequest) -> Result<(), SplayError>;

    /// Destroys a template built by `build_template`.
    fn destroy_template(&self, name: &str) -> Result<(), SplayError>;

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
