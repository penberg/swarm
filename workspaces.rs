use serde::Serialize;
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};
use turso::{Builder, Connection};

use crate::{
    SwarmError, database_error,
    repos::{Repository, RepositoryStore},
};

#[derive(Debug, Clone, Serialize)]
pub struct Workspace {
    pub repository: String,
    pub repository_alias: String,
    pub name: String,
    pub branch: String,
    pub path: PathBuf,
    pub created_at: i64,
    pub archived_at: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct WorkspaceReference {
    pub repository: String,
    pub workspace: String,
}

pub struct WorkspaceStore {
    repos: RepositoryStore,
}

impl WorkspaceStore {
    pub async fn open() -> Result<Self, SwarmError> {
        Ok(Self {
            repos: RepositoryStore::open().await?,
        })
    }

    pub async fn create(
        &self,
        repository: &str,
        name: Option<&str>,
    ) -> Result<Workspace, SwarmError> {
        let repo = self
            .repos
            .resolve_repository(repository)
            .await?
            .ok_or_else(|| SwarmError::RepositoryNotFound(repository.to_string()))?;
        let db = self.open_repo_db(&repo).await?;
        let default_branch = self.ensure_bare_repo(&repo)?;
        let workspace_name = resolve_workspace_name(name, &default_branch)?;

        if self
            .find_workspace(&db, &repo, &workspace_name)
            .await?
            .is_some()
        {
            return Err(SwarmError::DuplicateWorkspace(format!(
                "{}:{}",
                repo.alias.as_deref().unwrap_or(&repo.name),
                workspace_name
            )));
        }

        let workspaces_dir = self.repos.workspaces_dir(&repo);
        fs::create_dir_all(&workspaces_dir)?;
        let workspace_path = workspaces_dir.join(&workspace_name);

        if workspace_path.exists() {
            return Err(SwarmError::DuplicateWorkspace(format!(
                "{}:{}",
                repo.alias.as_deref().unwrap_or(&repo.name),
                workspace_name
            )));
        }

        let bare_repo_path = self.repos.bare_repo_path(&repo);
        let branch = self.materialize_worktree(
            &bare_repo_path,
            &workspace_path,
            &workspace_name,
            &default_branch,
        )?;
        let created_at = unix_timestamp();

        db.execute(
            "INSERT INTO workspaces (name, branch, path, created_at) VALUES (?1, ?2, ?3, ?4)",
            (
                workspace_name.as_str(),
                branch.as_str(),
                path_to_string(&workspace_path)?,
                created_at,
            ),
        )
        .await?;

        Ok(Workspace {
            repository: repo.canonical(),
            repository_alias: repo.alias.clone().unwrap_or_else(|| repo.name.clone()),
            name: workspace_name,
            branch,
            path: workspace_path,
            created_at,
        })
    }

    pub async fn list(&self, repository: &str) -> Result<Vec<Workspace>, SwarmError> {
        let repo = self
            .repos
            .resolve_repository(repository)
            .await?
            .ok_or_else(|| SwarmError::RepositoryNotFound(repository.to_string()))?;
        let db = self.open_repo_db(&repo).await?;
        let mut stmt = db
            .prepare(
                "SELECT name, branch, path, created_at, archived_at
                 FROM workspaces
                 WHERE archived_at IS NULL
                 ORDER BY created_at, name",
            )
            .await?;
        let mut rows = stmt.query(()).await?;
        let mut workspaces = Vec::new();

        while let Some(row) = rows.next().await? {
          let workspace = Workspace {
                repository: repo.canonical(),
                repository_alias: repo.alias.clone().unwrap_or_else(|| repo.name.clone()),
                name: row.get::<String>(0)?,
                branch: row.get::<String>(1)?,
                path: PathBuf::from(row.get::<String>(2)?),
                created_at: row.get::<i64>(3)?,
                archived_at: row.get::<Option<i64>>(4)?, // <-- ADD THIS
            };
            workspaces.push(self.refresh_workspace_branch(&db, workspace).await?);
        }

        Ok(workspaces)
    }

    pub async fn info(&self, workspace: &str) -> Result<Workspace, SwarmError> {
        let reference = parse_workspace_reference(workspace)?;
        let repo = self
            .resolve_repo_from_workspace_reference(&reference)
            .await?;
        let db = self.open_repo_db(&repo).await?;

        self.find_workspace(&db, &repo, &reference.workspace)
            .await?
            .ok_or_else(|| SwarmError::WorkspaceNotFound(workspace.to_string()))
    }

  pub async fn remove(&self, workspace: &str) -> Result<Workspace, SwarmError> {
        let reference = parse_workspace_reference(workspace)?;
        let repo = self
            .resolve_repo_from_workspace_reference(&reference)
            .await?;
        let db = self.open_repo_db(&repo).await?;
        let workspace = self
            .find_workspace(&db, &repo, &reference.workspace)
            .await?
            .ok_or_else(|| SwarmError::WorkspaceNotFound(workspace.to_string()))?;
       
        let archived_time = unix_timestamp();
        db.execute(
            "UPDATE workspaces SET archived_at = ?2 WHERE name = ?1",
            (workspace.name.as_str(), archived_time),
        )
        .await?;

        Ok(Workspace {
            archived_at: Some(archived_time),
            ..workspace
        })
    }
    pub async fn prune(&self, repository: &str) -> Result<Vec<String>, SwarmError> {
        let repo = self
            .repos
            .resolve_repository(repository)
            .await?
            .ok_or_else(|| SwarmError::RepositoryNotFound(repository.to_string()))?;
        let db = self.open_repo_db(&repo).await?;
        let mut stmt = db
            .prepare("SELECT name, path FROM workspaces WHERE archived_at IS NOT NULL")
            .await?;
        let mut rows = stmt.query(()).await?;

        let mut pruned_names = Vec::new();
        let bare_repo_path = self.repos.bare_repo_path(&repo); 
        while let Some(row) = rows.next().await? {
            let name: String = row.get(0)?;
            let path_str: String = row.get(1)?;
            let path = PathBuf::from(path_str);

            let _ = run_git(
                Some(&self.repos.repo_dir(&repo)),
                [
                    format!("--git-dir={}", bare_repo_path.display()),
                    "worktree".to_string(),
                    "remove".to_string(),
                    "--force".to_string(),
                    path.display().to_string(),
                ],
            );

            pruned_names.push(name);
        }
        db.execute("DELETE FROM workspaces WHERE archived_at IS NOT NULL", ())
            .await?;

        Ok(pruned_names)
    }

    pub async fn rename(&self, workspace: &str, new_name: &str) -> Result<Workspace, SwarmError> {
        let reference = parse_workspace_reference(workspace)?;
        let repo = self
            .resolve_repo_from_workspace_reference(&reference)
            .await?;
        let db = self.open_repo_db(&repo).await?;
        let workspace = self
            .find_workspace(&db, &repo, &reference.workspace)
            .await?
            .ok_or_else(|| SwarmError::WorkspaceNotFound(workspace.to_string()))?;
        let next_name = resolve_workspace_name(Some(new_name), &workspace.branch)?;

        if next_name == workspace.name {
            return Ok(workspace);
        }

        if self.find_workspace(&db, &repo, &next_name).await?.is_some() {
            return Err(SwarmError::DuplicateWorkspace(format!(
                "{}:{}",
                repo.alias.as_deref().unwrap_or(&repo.name),
                next_name
            )));
        }

        let sessions = self.count_workspace_sessions(&db, &workspace.name).await?;
        if sessions > 0 {
            return Err(SwarmError::InvalidWorkspace(format!(
                "cannot rename workspace `{}` with sessions",
                workspace.name
            )));
        }

        let bare_repo_path = self.repos.bare_repo_path(&repo);
        let next_path = workspace
            .path
            .parent()
            .ok_or(SwarmError::PathResolution)?
            .join(&next_name);

        run_git(
            Some(&self.repos.repo_dir(&repo)),
            [
                format!("--git-dir={}", bare_repo_path.display()),
                "branch".to_string(),
                "-m".to_string(),
                workspace.branch.clone(),
                next_name.clone(),
            ],
        )?;

        run_git(
            Some(&self.repos.repo_dir(&repo)),
            [
                format!("--git-dir={}", bare_repo_path.display()),
                "worktree".to_string(),
                "move".to_string(),
                workspace.path.display().to_string(),
                next_path.display().to_string(),
            ],
        )?;

        db.execute(
            "UPDATE workspaces
             SET name = ?2, branch = ?3, path = ?4
             WHERE name = ?1",
            (
                workspace.name.as_str(),
                next_name.as_str(),
                next_name.as_str(),
                path_to_string(&next_path)?,
            ),
        )
        .await?;

        Ok(Workspace {
            repository: repo.canonical(),
            repository_alias: repo.alias.clone().unwrap_or_else(|| repo.name.clone()),
            name: next_name.clone(),
            branch: next_name,
            path: next_path,
            created_at: workspace.created_at,
        })
    }

    async fn resolve_repo_from_workspace_reference(
        &self,
        reference: &WorkspaceReference,
    ) -> Result<Repository, SwarmError> {
        self.repos
            .resolve_repository(&reference.repository)
            .await?
            .ok_or_else(|| SwarmError::RepositoryNotFound(reference.repository.clone()))
    }

    async fn open_repo_db(&self, repo: &Repository) -> Result<Connection, SwarmError> {
        let repo_db_path = self.repos.repo_db_path(repo);
        let db = Builder::new_local(path_to_string(&repo_db_path)?)
            .build()
            .await
            .map_err(|err| database_error(&repo_db_path, "open", err))?;
        let conn = db
            .connect()
            .map_err(|err| database_error(&repo_db_path, "connect", err))?;

        migrate_repo_db(&conn, &repo_db_path).await?;

        Ok(conn)
    }

    pub async fn resolve_reference(
        &self,
        workspace: &str,
    ) -> Result<(Repository, Workspace), SwarmError> {
        let reference = parse_workspace_reference(workspace)?;
        let repo = self
            .resolve_repo_from_workspace_reference(&reference)
            .await?;
        let db = self.open_repo_db(&repo).await?;
        let workspace = self
            .find_workspace(&db, &repo, &reference.workspace)
            .await?
            .ok_or_else(|| SwarmError::WorkspaceNotFound(workspace.to_string()))?;

        Ok((repo, workspace))
    }

    async fn find_workspace(
        &self,
        db: &Connection,
        repo: &Repository,
        name: &str,
    ) -> Result<Option<Workspace>, SwarmError> {
        let mut stmt = db
            .prepare(
               "SELECT name, branch, path, created_at, archived_at
                 FROM workspaces
                 WHERE name = ?1 AND archived_at IS NULL
                 LIMIT 1"
            )
            .await?;
        let mut rows = stmt.query([name]).await?;

        if let Some(row) = rows.next().await? {
            let workspace = Workspace {
                repository: repo.canonical(),
                repository_alias: repo.alias.clone().unwrap_or_else(|| repo.name.clone()),
                name: row.get::<String>(0)?,
                branch: row.get::<String>(1)?,
                path: PathBuf::from(row.get::<String>(2)?),
                created_at: row.get::<i64>(3)?,
                archived_at: row.get::<Option<i64>>(4)?,
            };
            return Ok(Some(self.refresh_workspace_branch(db, workspace).await?));
        }

        Ok(None)
    }

    async fn refresh_workspace_branch(
        &self,
        db: &Connection,
        workspace: Workspace,
    ) -> Result<Workspace, SwarmError> {
        let branch = git_current_branch(&workspace.path)?;
        if branch == workspace.branch {
            return Ok(workspace);
        }

        db.execute(
            "UPDATE workspaces
             SET branch = ?2
             WHERE name = ?1",
            (workspace.name.as_str(), branch.as_str()),
        )
        .await?;

        Ok(Workspace { branch, ..workspace })
    }

    async fn count_workspace_sessions(
        &self,
        db: &Connection,
        workspace_name: &str,
    ) -> Result<i64, SwarmError> {
        let mut stmt = db
            .prepare(
                "SELECT COUNT(*)
                 FROM sessions
                 WHERE workspace_name = ?1",
            )
            .await?;
        let mut rows = stmt.query([workspace_name]).await?;

        if let Some(row) = rows.next().await? {
            return Ok(row.get::<i64>(0)?);
        }

        Ok(0)
    }

    fn ensure_bare_repo(&self, repo: &Repository) -> Result<String, SwarmError> {
        let bare_repo_path = self.repos.bare_repo_path(repo);
        self.repos.sync_repo(repo)?;

        let head = run_git(
            Some(&self.repos.repo_dir(repo)),
            [
                format!("--git-dir={}", bare_repo_path.display()),
                "symbolic-ref".to_string(),
                "--short".to_string(),
                "refs/remotes/origin/HEAD".to_string(),
            ],
        )?;

        Ok(head
            .trim()
            .strip_prefix("origin/")
            .unwrap_or(head.trim())
            .to_string())
    }

    fn materialize_worktree(
        &self,
        bare_repo_path: &Path,
        workspace_path: &Path,
        workspace_name: &str,
        default_branch: &str,
    ) -> Result<String, SwarmError> {
        let args = build_worktree_add_args(
            bare_repo_path,
            workspace_path,
            workspace_name,
            default_branch,
        )?;
        let branch = workspace_name.to_string();

        run_git(workspace_path.parent(), args)?;

        Ok(branch)
    }
}

pub fn parse_workspace_reference(input: &str) -> Result<WorkspaceReference, SwarmError> {
    let split = input
        .rsplit_once(':')
        .or_else(|| input.rsplit_once('/'))
        .ok_or_else(|| SwarmError::InvalidWorkspaceReference(input.to_string()))?;
    let (repository, workspace) = split;

    let repository = repository.trim();
    let workspace = workspace.trim();

    if repository.is_empty() || workspace.is_empty() {
        return Err(SwarmError::InvalidWorkspaceReference(input.to_string()));
    }

    Ok(WorkspaceReference {
        repository: repository.to_string(),
        workspace: workspace.to_string(),
    })
}

fn resolve_workspace_name(name: Option<&str>, default_branch: &str) -> Result<String, SwarmError> {
    let workspace_name = name.unwrap_or(default_branch).trim();

    if workspace_name.is_empty()
        || workspace_name.contains('/')
        || workspace_name.contains('\\')
        || workspace_name.contains(':')
        || workspace_name.contains(char::is_whitespace)
    {
        return Err(SwarmError::InvalidWorkspace(workspace_name.to_string()));
    }

    Ok(workspace_name.to_string())
}

fn build_worktree_add_args(
    bare_repo_path: &Path,
    workspace_path: &Path,
    workspace_name: &str,
    default_branch: &str,
) -> Result<Vec<String>, SwarmError> {
    let mut args = vec![
        format!("--git-dir={}", bare_repo_path.display()),
        "worktree".to_string(),
        "add".to_string(),
        "-B".to_string(),
        workspace_name.to_string(),
        workspace_path.display().to_string(),
    ];
    let upstream_branch = format!("refs/remotes/origin/{default_branch}");
    args.push(upstream_branch);

    Ok(args)
}

fn git_current_branch(path: &Path) -> Result<String, SwarmError> {
    let branch = run_git(Some(path), ["branch", "--show-current"])?;
    if !branch.is_empty() {
        return Ok(branch);
    }

    run_git(Some(path), ["rev-parse", "--short", "HEAD"])
}

fn run_git<I, S>(cwd: Option<&Path>, args: I) -> Result<String, SwarmError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut cmd = Command::new("git");
    if let Some(cwd) = cwd {
        cmd.current_dir(cwd);
    }

    for arg in args {
        cmd.arg(arg.as_ref());
    }

    let output = cmd.output()?;
    if !output.status.success() {
        return Err(SwarmError::Git(render_git_failure(output)));
    }

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn render_git_failure(output: std::process::Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();

    if !stderr.is_empty() {
        return stderr;
    }

    if !stdout.is_empty() {
        return stdout;
    }

    format!("exit status {}", output.status)
}

fn path_to_string(path: &Path) -> Result<&str, SwarmError> {
    path.to_str().ok_or(SwarmError::PathResolution)
}

fn unix_timestamp() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

pub async fn migrate_repo_db(conn: &Connection, path: &Path) -> Result<(), SwarmError> {
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS workspaces (
            name TEXT PRIMARY KEY,
            branch TEXT NOT NULL,
            path TEXT NOT NULL UNIQUE,
            created_at INTEGER NOT NULL,
            archived_at INTEGER
        );

        CREATE TABLE IF NOT EXISTS sessions (
            id TEXT PRIMARY KEY,
            workspace_name TEXT NOT NULL,
            command_json TEXT NOT NULL,
            path TEXT NOT NULL UNIQUE,
            log_path TEXT NOT NULL UNIQUE,
            socket_path TEXT NOT NULL UNIQUE,
            pid INTEGER,
            status TEXT NOT NULL,
            exit_code INTEGER,
            created_at INTEGER NOT NULL,
            stopped_at INTEGER
        );
        ",
    )
    .await
    .map_err(|err| database_error(path, "initialize schema", err))?;

    let mut stmt = conn
        .prepare("PRAGMA table_info(workspaces)")
        .await
        .map_err(|err| database_error(path, "inspect workspaces schema", err))?;
    let mut rows = stmt
        .query(())
        .await
        .map_err(|err| database_error(path, "inspect workspaces schema", err))?;
    let mut columns = Vec::new();

    while let Some(row) = rows.next().await? {
        columns.push(row.get::<String>(1)?);
    }

    if columns != ["name", "branch", "path", "created_at", "archived_at"] {
        conn.execute_batch(
            "
            DROP TABLE workspaces;
            CREATE TABLE workspaces (
                name TEXT PRIMARY KEY,
                branch TEXT NOT NULL,
                path TEXT NOT NULL UNIQUE,
                created_at INTEGER NOT NULL,
                archived_at INTEGER
            );
            ",
        )
        .await
        .map_err(|err| database_error(path, "migrate workspaces schema", err))?;
    }

    let mut stmt = conn
        .prepare("PRAGMA table_info(sessions)")
        .await
        .map_err(|err| database_error(path, "inspect sessions schema", err))?;
    let mut rows = stmt
        .query(())
        .await
        .map_err(|err| database_error(path, "inspect sessions schema", err))?;
    let mut session_columns = Vec::new();

    while let Some(row) = rows.next().await? {
        session_columns.push(row.get::<String>(1)?);
    }

    if session_columns
        != [
            "id",
            "workspace_name",
            "command_json",
            "path",
            "log_path",
            "socket_path",
            "pid",
            "status",
            "exit_code",
            "created_at",
            "stopped_at",
        ]
    {
        conn.execute_batch(
            "
            DROP TABLE sessions;
            CREATE TABLE sessions (
                id TEXT PRIMARY KEY,
                workspace_name TEXT NOT NULL,
                command_json TEXT NOT NULL,
                path TEXT NOT NULL UNIQUE,
                log_path TEXT NOT NULL UNIQUE,
                socket_path TEXT NOT NULL UNIQUE,
                pid INTEGER,
                status TEXT NOT NULL,
                exit_code INTEGER,
                created_at INTEGER NOT NULL,
                stopped_at INTEGER
            );
            ",
        )
        .await
        .map_err(|err| database_error(path, "migrate sessions schema", err))?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::PathBuf,
        process::Command,
        time::{SystemTime, UNIX_EPOCH},
    };

    use super::{build_worktree_add_args, render_git_failure, run_git};

    #[test]
    fn default_branch_workspace_is_reset_to_origin_tip() {
        let bare_repo_path = create_bare_repo("main");
        let workspace_path = PathBuf::from("/tmp/swarm-test-main");

        let args =
            build_worktree_add_args(&bare_repo_path, &workspace_path, "main", "main").unwrap();

        assert_eq!(
            args,
            vec![
                format!("--git-dir={}", bare_repo_path.display()),
                "worktree".to_string(),
                "add".to_string(),
                "-B".to_string(),
                "main".to_string(),
                workspace_path.display().to_string(),
                "refs/remotes/origin/main".to_string(),
            ]
        );
    }

    #[test]
    fn new_workspace_branches_from_origin_default_branch() {
        let bare_repo_path = create_bare_repo("main");
        let workspace_path = PathBuf::from("/tmp/swarm-test-feature");

        let args =
            build_worktree_add_args(&bare_repo_path, &workspace_path, "feature", "main").unwrap();

        assert_eq!(
            args,
            vec![
                format!("--git-dir={}", bare_repo_path.display()),
                "worktree".to_string(),
                "add".to_string(),
                "-B".to_string(),
                "feature".to_string(),
                workspace_path.display().to_string(),
                "refs/remotes/origin/main".to_string(),
            ]
        );
    }

    #[test]
    fn orphan_workspace_branch_is_reset_to_origin_default_branch() {
        let bare_repo_path = create_bare_repo("main");
        let workspace_path = PathBuf::from("/tmp/swarm-test-existing");
        run_git(
            None,
            [
                format!("--git-dir={}", bare_repo_path.display()),
                "branch".to_string(),
                "existing".to_string(),
                "refs/remotes/origin/main".to_string(),
            ],
        )
        .unwrap();

        let args =
            build_worktree_add_args(&bare_repo_path, &workspace_path, "existing", "main").unwrap();

        assert_eq!(
            args,
            vec![
                format!("--git-dir={}", bare_repo_path.display()),
                "worktree".to_string(),
                "add".to_string(),
                "-B".to_string(),
                "existing".to_string(),
                workspace_path.display().to_string(),
                "refs/remotes/origin/main".to_string(),
            ]
        );
    }

    fn create_bare_repo(default_branch: &str) -> PathBuf {
        let repo_path = unique_temp_path("swarm-workspaces-test");
        fs::create_dir_all(&repo_path).unwrap();
        run_git(
            None,
            [
                "init".to_string(),
                "--bare".to_string(),
                repo_path.display().to_string(),
            ],
        )
        .unwrap();
        let commit = run_git(
            None,
            [
                format!("--git-dir={}", repo_path.display()),
                "commit-tree".to_string(),
                "4b825dc642cb6eb9a060e54bf8d69288fbee4904".to_string(),
                "-m".to_string(),
                "init".to_string(),
            ],
        )
        .or_else(|_| {
            run_git_with_env(
                [
                    ("GIT_AUTHOR_NAME", "Swarm Tests"),
                    ("GIT_AUTHOR_EMAIL", "swarm-tests@example.com"),
                    ("GIT_COMMITTER_NAME", "Swarm Tests"),
                    ("GIT_COMMITTER_EMAIL", "swarm-tests@example.com"),
                ],
                [
                    format!("--git-dir={}", repo_path.display()),
                    "commit-tree".to_string(),
                    "4b825dc642cb6eb9a060e54bf8d69288fbee4904".to_string(),
                    "-m".to_string(),
                    "init".to_string(),
                ],
            )
        })
        .unwrap();
        run_git(
            None,
            [
                format!("--git-dir={}", repo_path.display()),
                "update-ref".to_string(),
                format!("refs/remotes/origin/{default_branch}"),
                commit,
            ],
        )
        .unwrap();

        repo_path
    }

    fn unique_temp_path(prefix: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("{prefix}-{nonce}"))
    }

    fn run_git_with_env<I, S, E, K, V>(envs: E, args: I) -> Result<String, String>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
        E: IntoIterator<Item = (K, V)>,
        K: AsRef<str>,
        V: AsRef<str>,
    {
        let mut cmd = Command::new("git");

        for (key, value) in envs {
            cmd.env(key.as_ref(), value.as_ref());
        }

        for arg in args {
            cmd.arg(arg.as_ref());
        }

        let output = cmd.output().map_err(|err| err.to_string())?;
        if !output.status.success() {
            return Err(render_git_failure(output));
        }

        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }
}
