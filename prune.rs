use crate::{
    SplayError,
    repos::RepositoryStore,
    sessions::SessionStore,
    workspaces::{PruneReport, WorkspaceStore},
};

pub struct PruneStore {
    repos: RepositoryStore,
    sessions: SessionStore,
    workspaces: WorkspaceStore,
}

impl PruneStore {
    pub async fn open() -> Result<Self, SplayError> {
        Ok(Self {
            repos: RepositoryStore::open().await?,
            sessions: SessionStore::open().await?,
            workspaces: WorkspaceStore::open().await?,
        })
    }

    pub async fn sessions(&self) -> Result<usize, SplayError> {
        self.sessions.prune_terminal_sessions().await
    }

    pub async fn workspaces(&self) -> Result<PruneReport, SplayError> {
        let mut report = PruneReport::default();

        for repo in self.repos.list().await? {
            let mut repo_report = self.workspaces.prune(&repo.canonical()).await?;
            report.pruned.append(&mut repo_report.pruned);
            report.failed.append(&mut repo_report.failed);
        }

        Ok(report)
    }
}
