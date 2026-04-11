use crate::{
    SwarmError, repos::RepositoryStore, sessions::SessionStore, workspaces::WorkspaceStore,
};

pub struct PruneStore {
    repos: RepositoryStore,
    sessions: SessionStore,
    workspaces: WorkspaceStore,
}

impl PruneStore {
    pub async fn open() -> Result<Self, SwarmError> {
        Ok(Self {
            repos: RepositoryStore::open().await?,
            sessions: SessionStore::open().await?,
            workspaces: WorkspaceStore::open().await?,
        })
    }

    pub async fn sessions(&self) -> Result<usize, SwarmError> {
        self.sessions.prune_terminal_sessions().await
    }

    pub async fn workspaces(&self) -> Result<usize, SwarmError> {
        let mut pruned = 0;

        for repo in self.repos.list().await? {
            pruned += self.workspaces.prune(&repo.canonical()).await?.len();
        }

        Ok(pruned)
    }
}
