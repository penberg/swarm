pub mod cmd;
pub mod forges;
mod legacy;
pub mod opts;
pub mod prune;
pub mod repos;
pub mod sandbox;
pub mod sessions;
pub mod templates;
pub mod toolchains;
pub mod workspaces;

use std::{
    os::unix::process::CommandExt,
    path::{Path, PathBuf},
    process::Command,
};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum SplayError {
    #[error("{0}")]
    Io(#[from] std::io::Error),

    #[error("{0}")]
    Json(#[from] serde_json::Error),

    #[error("{0}")]
    Turso(#[from] turso::Error),

    #[error("database `{path}` {operation} failed: {source}")]
    Database {
        path: PathBuf,
        operation: String,
        #[source]
        source: turso::Error,
    },

    #[error("invalid repository `{0}`; expected host/owner/name or a full git remote URL")]
    InvalidRepository(String),

    #[error("repository `{0}` already exists")]
    DuplicateRepository(String),

    #[error("alias `{0}` already exists")]
    DuplicateAlias(String),

    #[error("repository `{0}` not found")]
    RepositoryNotFound(String),

    #[error("invalid workspace `{0}`")]
    InvalidWorkspace(String),

    #[error("invalid workspace reference `{0}`; expected <repo>:<workspace>")]
    InvalidWorkspaceReference(String),

    #[error("workspace `{0}` already exists")]
    DuplicateWorkspace(String),

    #[error("workspace `{0}` not found")]
    WorkspaceNotFound(String),

    #[error("invalid session `{0}`")]
    InvalidSession(String),

    #[error("session `{0}` not found")]
    SessionNotFound(String),

    #[error("session `{0}` is still running")]
    SessionRunning(String),

    #[error("invalid sandbox `{0}`; expected local or exedev")]
    InvalidSandbox(String),

    #[error(
        "{sandbox} is not enabled for repository `{repository}`; enable it with `splay repo enable {repository} {sandbox}`"
    )]
    SandboxNotEnabled { repository: String, sandbox: String },

    #[error("{0}")]
    TemplateNotReady(String),

    #[error("sandbox command failed: {0}")]
    Sandbox(String),

    #[error("git command failed: {0}")]
    Git(String),

    #[error("failed to prune {0} workspace(s)")]
    PruneIncomplete(usize),

    #[error("failed to resolve splay data directory")]
    PathResolution,

    #[error(
        "cannot move `{path}` to the Splay data directory while {sessions} session(s) still run there; stop them and try again"
    )]
    LegacyDataDirInUse { path: PathBuf, sessions: usize },
}

/// The running executable. Unlike [`std::env::current_exe`], it keeps
/// working after an upgrade or rebuild replaces the file that this process
/// started from.
pub(crate) const SELF_EXE: &str = "/proc/self/exe";

/// Returns a command that runs this executable; see [`SELF_EXE`].
pub(crate) fn self_command() -> Command {
    let mut command = Command::new(SELF_EXE);
    if let Some(arg0) = std::env::args_os().next() {
        command.arg0(arg0);
    }
    command
}

/// How long a connection waits for another connection's write to finish.
/// The database admits one writer at a time, and a process may run several
/// operations on it at once.
const BUSY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// Connects to an open database.
pub(crate) fn connect(db: &turso::Database, path: &Path) -> Result<turso::Connection, SplayError> {
    let conn = db
        .connect()
        .map_err(|err| database_error(path, "connect", err))?;
    conn.busy_timeout(BUSY_TIMEOUT)
        .map_err(|err| database_error(path, "configure", err))?;
    Ok(conn)
}

pub fn database_error(path: &Path, operation: &str, source: turso::Error) -> SplayError {
    SplayError::Database {
        path: path.to_path_buf(),
        operation: operation.to_string(),
        source,
    }
}
