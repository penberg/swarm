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
        let requested_name = resolve_workspace_name(name)?;
        let workspaces_dir = self.repos.workspaces_dir(&repo);
        fs::create_dir_all(&workspaces_dir)?;
        let workspace_name =
            Self::allocate_workspace_name(&db, &workspaces_dir, &requested_name).await?;
        let workspace_path = workspaces_dir.join(&workspace_name);

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
            archived_at: None,
        })
    }

    pub async fn clone(&self, workspace: &str, name: &str) -> Result<Workspace, SwarmError> {
        let reference = parse_workspace_reference(workspace)?;
        let repo = self
            .resolve_repo_from_workspace_reference(&reference)
            .await?;
        let db = self.open_repo_db(&repo).await?;
        let source = self
            .find_workspace(&db, &repo, &reference.workspace)
            .await?
            .ok_or_else(|| SwarmError::WorkspaceNotFound(workspace.to_string()))?;
        let requested_name = resolve_workspace_name(Some(name))?;
        let workspaces_dir = self.repos.workspaces_dir(&repo);
        fs::create_dir_all(&workspaces_dir)?;
        let workspace_name =
            Self::allocate_workspace_name(&db, &workspaces_dir, &requested_name).await?;
        let workspace_path = workspaces_dir.join(&workspace_name);

        let bare_repo_path = self.repos.bare_repo_path(&repo);
        let branch = self.materialize_cloned_worktree(
            &bare_repo_path,
            &source,
            &workspace_path,
            &workspace_name,
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
            archived_at: None,
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
        let mut archived_workspaces = Vec::new();

        while let Some(row) = rows.next().await? {
            archived_workspaces.push((row.get::<String>(0)?, PathBuf::from(row.get::<String>(1)?)));
        }

        let mut pruned_names = Vec::new();
        let bare_repo_path = self.repos.bare_repo_path(&repo);
        for (name, path) in archived_workspaces {
            run_git(
                Some(&self.repos.repo_dir(&repo)),
                [
                    format!("--git-dir={}", bare_repo_path.display()),
                    "worktree".to_string(),
                    "remove".to_string(),
                    "--force".to_string(),
                    path.display().to_string(),
                ],
            )?;

            db.execute("DELETE FROM workspaces WHERE name = ?1", [name.as_str()])
                .await?;

            pruned_names.push(name);
        }

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
        let next_name = resolve_workspace_name(Some(new_name))?;

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
            archived_at: workspace.archived_at,
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
        self.reconcile_workspaces(&conn, repo).await?;

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
                 LIMIT 1",
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

        Ok(Workspace {
            branch,
            ..workspace
        })
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

    async fn allocate_workspace_name(
        db: &Connection,
        workspaces_dir: &Path,
        requested_name: &str,
    ) -> Result<String, SwarmError> {
        if !Self::workspace_name_exists(db, workspaces_dir, requested_name).await? {
            return Ok(requested_name.to_string());
        }

        let mut suffix = 2;
        loop {
            let candidate = format!("{requested_name}-{suffix}");
            if !Self::workspace_name_exists(db, workspaces_dir, &candidate).await? {
                return Ok(candidate);
            }

            suffix += 1;
        }
    }

    async fn workspace_name_exists(
        db: &Connection,
        workspaces_dir: &Path,
        name: &str,
    ) -> Result<bool, SwarmError> {
        let mut stmt = db
            .prepare(
                "SELECT 1
                 FROM workspaces
                 WHERE name = ?1
                 LIMIT 1",
            )
            .await?;
        let mut rows = stmt.query([name]).await?;

        if rows.next().await?.is_some() {
            return Ok(true);
        }

        Ok(workspaces_dir.join(name).exists())
    }

    async fn reconcile_workspaces(
        &self,
        db: &Connection,
        repo: &Repository,
    ) -> Result<(), SwarmError> {
        let workspaces_dir = self.repos.workspaces_dir(repo);
        if !workspaces_dir.exists() {
            return Ok(());
        }

        let registered_worktrees = self.repos.bare_repo_path(repo).join("worktrees");
        let mut entries = fs::read_dir(&workspaces_dir)?
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false))
            .collect::<Vec<_>>();
        entries.sort_by_key(|entry| entry.file_name());

        for entry in entries {
            let workspace_name = entry.file_name().to_string_lossy().to_string();
            let workspace_path = entry.path();
            let workspace_gitdir = workspace_path.join(".git");
            let registered_path = registered_worktrees.join(&workspace_name);

            if !workspace_gitdir.exists() || !registered_path.exists() {
                continue;
            }

            let mut stmt = db
                .prepare(
                    "SELECT path
                     FROM workspaces
                     WHERE name = ?1
                     LIMIT 1",
                )
                .await?;
            let mut rows = stmt.query([workspace_name.as_str()]).await?;

            if let Some(row) = rows.next().await? {
                let recorded_path = PathBuf::from(row.get::<String>(0)?);
                if recorded_path != workspace_path {
                    db.execute(
                        "UPDATE workspaces
                         SET path = ?2
                         WHERE name = ?1",
                        (workspace_name.as_str(), path_to_string(&workspace_path)?),
                    )
                    .await?;
                }
                continue;
            }

            let branch = git_current_branch(&workspace_path)?;
            let created_at = workspace_created_at(&workspace_path);

            db.execute(
                "INSERT INTO workspaces (name, branch, path, created_at, archived_at)
                 VALUES (?1, ?2, ?3, ?4, NULL)",
                (
                    workspace_name.as_str(),
                    branch.as_str(),
                    path_to_string(&workspace_path)?,
                    created_at,
                ),
            )
            .await?;
        }

        Ok(())
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

    fn materialize_cloned_worktree(
        &self,
        bare_repo_path: &Path,
        source: &Workspace,
        workspace_path: &Path,
        workspace_name: &str,
    ) -> Result<String, SwarmError> {
        let source_head = git_rev_parse(&source.path, "HEAD")?;
        let args = build_clone_worktree_add_args(
            bare_repo_path,
            workspace_path,
            workspace_name,
            &source_head,
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

fn resolve_workspace_name(name: Option<&str>) -> Result<String, SwarmError> {
    let workspace_name = name.unwrap_or("workspace").trim();

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

fn build_clone_worktree_add_args(
    bare_repo_path: &Path,
    workspace_path: &Path,
    workspace_name: &str,
    source_head: &str,
) -> Result<Vec<String>, SwarmError> {
    Ok(vec![
        format!("--git-dir={}", bare_repo_path.display()),
        "worktree".to_string(),
        "add".to_string(),
        "-b".to_string(),
        workspace_name.to_string(),
        workspace_path.display().to_string(),
        source_head.to_string(),
    ])
}

fn git_current_branch(path: &Path) -> Result<String, SwarmError> {
    let branch = run_git(Some(path), ["branch", "--show-current"])?;
    if !branch.is_empty() {
        return Ok(branch);
    }

    run_git(Some(path), ["rev-parse", "--short", "HEAD"])
}

fn git_rev_parse(path: &Path, revision: &str) -> Result<String, SwarmError> {
    run_git(Some(path), ["rev-parse", revision])
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

fn workspace_created_at(path: &Path) -> i64 {
    fs::metadata(path)
        .and_then(|metadata| metadata.created().or_else(|_| metadata.modified()))
        .ok()
        .and_then(|timestamp| timestamp.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or_else(unix_timestamp)
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

    let columns = table_columns(conn, "workspaces", path).await?;

    if columns == ["name", "branch", "path", "created_at"] {
        conn.execute("ALTER TABLE workspaces ADD COLUMN archived_at INTEGER", ())
            .await
            .map_err(|err| database_error(path, "migrate workspaces schema", err))?;
    } else if columns != ["name", "branch", "path", "created_at", "archived_at"] {
        return Err(SwarmError::Io(std::io::Error::other(format!(
            "unsupported workspaces schema columns: {}",
            columns.join(", ")
        ))));
    }

    let session_columns = table_columns(conn, "sessions", path).await?;

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

async fn table_columns(
    conn: &Connection,
    table: &str,
    path: &Path,
) -> Result<Vec<String>, SwarmError> {
    let mut stmt = conn
        .prepare(&format!("PRAGMA table_info({table})"))
        .await
        .map_err(|err| database_error(path, &format!("inspect {table} schema"), err))?;
    let mut rows = stmt
        .query(())
        .await
        .map_err(|err| database_error(path, &format!("inspect {table} schema"), err))?;
    let mut columns = Vec::new();

    while let Some(row) = rows.next().await? {
        columns.push(row.get::<String>(1)?);
    }

    Ok(columns)
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::PathBuf,
        process::Command,
        sync::{Mutex, OnceLock},
        time::{SystemTime, UNIX_EPOCH},
    };
    use turso::Builder;

    use super::{
        build_clone_worktree_add_args, build_worktree_add_args, migrate_repo_db,
        render_git_failure, run_git,
    };
    use crate::{SwarmError, repos::RepositoryStore, workspaces::WorkspaceStore};

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

    #[test]
    fn cloned_workspace_branches_from_source_head() {
        let bare_repo_path = create_bare_repo("main");
        let workspace_path = PathBuf::from("/tmp/swarm-test-clone");

        let args = build_clone_worktree_add_args(
            &bare_repo_path,
            &workspace_path,
            "feature-copy",
            "abc123def456",
        )
        .unwrap();

        assert_eq!(
            args,
            vec![
                format!("--git-dir={}", bare_repo_path.display()),
                "worktree".to_string(),
                "add".to_string(),
                "-b".to_string(),
                "feature-copy".to_string(),
                workspace_path.display().to_string(),
                "abc123def456".to_string(),
            ]
        );
    }

    #[tokio::test]
    async fn migrate_repo_db_adds_archived_at_without_dropping_workspaces() {
        let db_path = unique_temp_path("swarm-workspaces-db");
        let db = Builder::new_local(db_path.to_str().unwrap())
            .build()
            .await
            .unwrap();
        let conn = db.connect().unwrap();

        conn.execute_batch(
            "
            CREATE TABLE workspaces (
                name TEXT PRIMARY KEY,
                branch TEXT NOT NULL,
                path TEXT NOT NULL UNIQUE,
                created_at INTEGER NOT NULL
            );

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
        .unwrap();
        conn.execute(
            "INSERT INTO workspaces (name, branch, path, created_at) VALUES (?1, ?2, ?3, ?4)",
            ("main", "main", "/tmp/workspaces/main", 123_i64),
        )
        .await
        .unwrap();

        migrate_repo_db(&conn, &db_path).await.unwrap();

        let mut stmt = conn.prepare("PRAGMA table_info(workspaces)").await.unwrap();
        let mut rows = stmt.query(()).await.unwrap();
        let mut columns = Vec::new();
        while let Some(row) = rows.next().await.unwrap() {
            columns.push(row.get::<String>(1).unwrap());
        }
        assert_eq!(
            columns,
            vec!["name", "branch", "path", "created_at", "archived_at"]
        );

        let mut stmt = conn
            .prepare("SELECT name, branch, path, created_at, archived_at FROM workspaces")
            .await
            .unwrap();
        let mut rows = stmt.query(()).await.unwrap();
        let row = rows.next().await.unwrap().unwrap();
        assert_eq!(row.get::<String>(0).unwrap(), "main");
        assert_eq!(row.get::<String>(1).unwrap(), "main");
        assert_eq!(row.get::<String>(2).unwrap(), "/tmp/workspaces/main");
        assert_eq!(row.get::<i64>(3).unwrap(), 123);
        assert_eq!(row.get::<Option<i64>>(4).unwrap(), None);
    }

    #[tokio::test]
    async fn prune_keeps_archived_row_when_git_remove_fails() {
        let _guard = env_lock().lock().unwrap_or_else(|err| err.into_inner());
        let data_home = unique_temp_path("swarm-test-data-home");
        fs::create_dir_all(&data_home).unwrap();
        let _env_guard = ScopedEnvVar::set("XDG_DATA_HOME", &data_home);

        let result = async {
            let repos = RepositoryStore::open().await.unwrap();
            let repo = repos.add("github/penberg/swarm", None).await.unwrap();
            let bare_repo_path = repos.bare_repo_path(&repo);
            fs::create_dir_all(repos.repo_dir(&repo)).unwrap();
            run_git(
                Some(&repos.repo_dir(&repo)),
                [
                    "init".to_string(),
                    "--bare".to_string(),
                    bare_repo_path.display().to_string(),
                ],
            )
            .unwrap();

            let repo_db_path = repos.repo_db_path(&repo);
            let db = Builder::new_local(repo_db_path.to_str().unwrap())
                .build()
                .await
                .unwrap();
            let conn = db.connect().unwrap();
            migrate_repo_db(&conn, &repo_db_path).await.unwrap();
            conn.execute(
                "INSERT INTO workspaces (name, branch, path, created_at, archived_at) VALUES (?1, ?2, ?3, ?4, ?5)",
                ("archived", "main", "/tmp/does-not-exist", 1_i64, 2_i64),
            )
            .await
            .unwrap();

            let store = WorkspaceStore::open().await.unwrap();
            let err = store.prune("github/penberg/swarm").await.unwrap_err();
            assert!(matches!(err, SwarmError::Git(_)));

            let mut stmt = conn
                .prepare("SELECT name FROM workspaces WHERE archived_at IS NOT NULL")
                .await
                .unwrap();
            let mut rows = stmt.query(()).await.unwrap();
            let row = rows.next().await.unwrap().unwrap();
            assert_eq!(row.get::<String>(0).unwrap(), "archived");
            assert!(rows.next().await.unwrap().is_none());
        }
        .await;

        result
    }

    #[tokio::test]
    async fn allocate_workspace_name_skips_archived_workspace_ids() {
        let db_path = unique_temp_path("swarm-workspaces-db");
        let db = Builder::new_local(db_path.to_str().unwrap())
            .build()
            .await
            .unwrap();
        let conn = db.connect().unwrap();
        migrate_repo_db(&conn, &db_path).await.unwrap();
        conn.execute(
            "INSERT INTO workspaces (name, branch, path, created_at, archived_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            ("main", "main", "/tmp/workspaces/main", 1_i64, 2_i64),
        )
        .await
        .unwrap();

        let workspaces_dir = unique_temp_path("swarm-workspaces-dir");
        fs::create_dir_all(&workspaces_dir).unwrap();
        assert_eq!(
            WorkspaceStore::allocate_workspace_name(&conn, &workspaces_dir, "main")
                .await
                .unwrap(),
            "main-2"
        );
    }

    #[tokio::test]
    async fn workspace_name_exists_treats_archived_ids_as_taken() {
        let _guard = env_lock().lock().unwrap_or_else(|err| err.into_inner());
        let data_home = unique_temp_path("swarm-test-data-home");
        fs::create_dir_all(&data_home).unwrap();
        let _env_guard = ScopedEnvVar::set("XDG_DATA_HOME", &data_home);
        let db_path = unique_temp_path("swarm-workspaces-db");
        let db = Builder::new_local(db_path.to_str().unwrap())
            .build()
            .await
            .unwrap();
        let conn = db.connect().unwrap();
        migrate_repo_db(&conn, &db_path).await.unwrap();
        conn.execute(
            "INSERT INTO workspaces (name, branch, path, created_at, archived_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            ("main", "main", "/tmp/workspaces/main", 1_i64, 2_i64),
        )
        .await
        .unwrap();

        let workspaces_dir = unique_temp_path("swarm-workspaces-dir");
        fs::create_dir_all(&workspaces_dir).unwrap();
        assert!(
            WorkspaceStore::workspace_name_exists(&conn, &workspaces_dir, "main")
                .await
                .unwrap()
        );
    }

    #[tokio::test]
    async fn list_reconciles_missing_workspace_rows_from_live_worktrees() {
        let _guard = env_lock().lock().unwrap_or_else(|err| err.into_inner());
        let data_home = unique_temp_path("swarm-test-data-home");
        fs::create_dir_all(&data_home).unwrap();
        let _env_guard = ScopedEnvVar::set("XDG_DATA_HOME", &data_home);

        let repos = RepositoryStore::open().await.unwrap();
        let repo = repos.add("github/penberg/swarm", None).await.unwrap();
        let bare_repo_path = repos.bare_repo_path(&repo);
        fs::rename(create_bare_repo("main"), &bare_repo_path).unwrap();

        let workspaces_dir = repos.workspaces_dir(&repo);
        fs::create_dir_all(&workspaces_dir).unwrap();
        let workspace_path = workspaces_dir.join("restored");
        run_git(
            Some(&workspaces_dir),
            build_worktree_add_args(&bare_repo_path, &workspace_path, "restored", "main").unwrap(),
        )
        .unwrap();

        let store = WorkspaceStore::open().await.unwrap();
        let workspaces = store.list("swarm").await.unwrap();

        assert_eq!(workspaces.len(), 1);
        assert_eq!(workspaces[0].repository, "github/penberg/swarm");
        assert_eq!(workspaces[0].name, "restored");
        assert_eq!(workspaces[0].branch, "restored");
        assert_eq!(workspaces[0].path, workspace_path);
        assert!(workspaces[0].created_at > 0);

        let db = store.open_repo_db(&repo).await.unwrap();
        let mut stmt = db
            .prepare(
                "SELECT name, branch, path, archived_at
                 FROM workspaces
                 WHERE name = 'restored'",
            )
            .await
            .unwrap();
        let mut rows = stmt.query(()).await.unwrap();
        let row = rows.next().await.unwrap().unwrap();
        assert_eq!(row.get::<String>(0).unwrap(), "restored");
        assert_eq!(row.get::<String>(1).unwrap(), "restored");
        assert_eq!(
            row.get::<String>(2).unwrap(),
            workspace_path.to_str().unwrap()
        );
        assert_eq!(row.get::<Option<i64>>(3).unwrap(), None);
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

    fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    struct ScopedEnvVar {
        key: &'static str,
        original: Option<std::ffi::OsString>,
    }

    impl ScopedEnvVar {
        fn set(key: &'static str, value: &std::path::Path) -> Self {
            let original = std::env::var_os(key);
            // SAFETY: the caller serializes environment mutation with `env_lock`.
            unsafe {
                std::env::set_var(key, value);
            }

            Self { key, original }
        }
    }

    impl Drop for ScopedEnvVar {
        fn drop(&mut self) {
            // SAFETY: the caller serializes environment mutation with `env_lock`.
            unsafe {
                if let Some(value) = self.original.as_ref() {
                    std::env::set_var(self.key, value);
                } else {
                    std::env::remove_var(self.key);
                }
            }
        }
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
