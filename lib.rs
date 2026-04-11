pub mod forges;
pub mod prune;
pub mod repos;
pub mod sessions;
pub mod toolchains;
pub mod workspaces;

use std::path::{Path, PathBuf};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum SwarmError {
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

    #[error("git command failed: {0}")]
    Git(String),

    #[error("failed to resolve swarm data directory")]
    PathResolution,
}

pub fn database_error(path: &Path, operation: &str, source: turso::Error) -> SwarmError {
    SwarmError::Database {
        path: path.to_path_buf(),
        operation: operation.to_string(),
        source,
    }
}
