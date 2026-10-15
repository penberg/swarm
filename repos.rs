use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::{Arc, LazyLock, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::sync::OwnedMutexGuard;
use turso::{Builder, Connection, Database};

use crate::{SplayError, connect, database_error, legacy, sandbox::SandboxKind};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Repository {
    pub host: String,
    pub owner: String,
    pub name: String,
    pub alias: Option<String>,
    pub remote_url: Option<String>,
    pub collapsed: bool,
    pub last_used_at: Option<i64>,
    /// When the repository was archived, hiding it from the sidebar.
    pub archived_at: Option<i64>,
}

impl Repository {
    pub fn parse(input: &str, alias: Option<&str>) -> Result<Self, SplayError> {
        let input = input.trim();
        let (host, owner, name, remote_url) = parse_remote_reference(input)
            .or_else(|| {
                parse_canonical_reference(input)
                    .map(|(host, owner, name)| (host, owner, name, None))
            })
            .ok_or_else(|| SplayError::InvalidRepository(input.to_string()))?;

        let default_alias = alias
            .map(str::trim)
            .filter(|alias| !alias.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| name.to_string());

        Ok(Self {
            host,
            owner,
            name: name.clone(),
            alias: Some(default_alias),
            remote_url,
            collapsed: false,
            last_used_at: None,
            archived_at: None,
        })
    }

    pub fn canonical(&self) -> String {
        format!("{}/{}/{}", self.host, self.owner, self.name)
    }

    pub fn remote_url(&self) -> String {
        self.remote_url.clone().unwrap_or_else(|| {
            format!(
                "https://{}/{}/{}.git",
                resolve_remote_host(&self.host),
                self.owner,
                self.name
            )
        })
    }
}

#[derive(Debug, Clone)]
struct SplayPaths {
    data_dir: PathBuf,
    repos_dir: PathBuf,
    index_db_path: PathBuf,
}

impl SplayPaths {
    fn resolve() -> Result<Self, SplayError> {
        let dirs =
            ProjectDirs::from("com", "penberg", "splay").ok_or(SplayError::PathResolution)?;
        let data_dir = dirs.data_dir().to_path_buf();
        let repos_dir = data_dir.join("repos");
        let index_db_path = data_dir.join("index.db");

        Ok(Self {
            data_dir,
            repos_dir,
            index_db_path,
        })
    }

    /// Where data lived before Splay was renamed from Swarm.
    fn legacy_data_dir() -> Option<PathBuf> {
        ProjectDirs::from("com", "penberg", "swarm").map(|dirs| dirs.data_dir().to_path_buf())
    }

    fn repo_dir(&self, repo: &Repository) -> PathBuf {
        self.repos_dir
            .join(&repo.host)
            .join(&repo.owner)
            .join(&repo.name)
    }

    fn repo_db_path(&self, repo: &Repository) -> PathBuf {
        self.repo_dir(repo).join("repo.db")
    }

    fn repo_meta_path(&self, repo: &Repository) -> PathBuf {
        self.repo_dir(repo).join("meta.toml")
    }
}

/// Serializes changes to each repository's mirror and workspace names within
/// the process: git refuses concurrent writes to a repository's config, and
/// two new workspaces must not take the same name.
static REPO_LOCKS: LazyLock<Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Holds the lock of `repo` until the returned guard is dropped.
pub async fn lock_repo(repo: &Repository) -> OwnedMutexGuard<()> {
    let lock = REPO_LOCKS
        .lock()
        .unwrap_or_else(|err| err.into_inner())
        .entry(repo.canonical())
        .or_default()
        .clone();
    lock.lock_owned().await
}

/// Returns the directory Splay keeps its state in, without creating it.
pub fn data_dir() -> Result<PathBuf, SplayError> {
    Ok(SplayPaths::resolve()?.data_dir)
}

const INDEX_DB_OPEN_ATTEMPTS: u32 = 40;
const INDEX_DB_OPEN_RETRY_DELAY: Duration = Duration::from_millis(50);

pub struct RepositoryStore {
    paths: SplayPaths,
    conn: Connection,
}

impl RepositoryStore {
    pub async fn open() -> Result<Self, SplayError> {
        let paths = SplayPaths::resolve()?;
        if let Some(legacy_data_dir) = SplayPaths::legacy_data_dir() {
            legacy::migrate_data_dir(&legacy_data_dir, &paths.data_dir).await?;
        }
        fs::create_dir_all(&paths.data_dir)?;
        fs::create_dir_all(&paths.repos_dir)?;

        let index_db_path = paths.index_db_path.clone();
        let db = open_index_db(&index_db_path).await?;
        let conn = connect(&db, &index_db_path)?;

        conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS repos (
                host TEXT NOT NULL,
                owner TEXT NOT NULL,
                name TEXT NOT NULL,
                alias TEXT,
                remote_url TEXT,
                collapsed INTEGER NOT NULL DEFAULT 0,
                created_at INTEGER NOT NULL,
                last_used_at INTEGER,
                PRIMARY KEY (host, owner, name)
            );

            CREATE UNIQUE INDEX IF NOT EXISTS repos_alias_idx
            ON repos(alias)
            WHERE alias IS NOT NULL;

            CREATE TABLE IF NOT EXISTS repo_integrations (
                host TEXT NOT NULL,
                owner TEXT NOT NULL,
                name TEXT NOT NULL,
                integration TEXT NOT NULL,
                PRIMARY KEY (host, owner, name, integration)
            );

            CREATE TABLE IF NOT EXISTS workspace_usage (
                repository TEXT NOT NULL,
                workspace TEXT NOT NULL,
                last_used_at INTEGER NOT NULL,
                PRIMARY KEY (repository, workspace)
            );
            ",
        )
        .await
        .map_err(|err| database_error(&index_db_path, "initialize schema", err))?;
        ensure_collapsed_column(&conn, &index_db_path).await?;
        ensure_remote_url_column(&conn, &index_db_path).await?;
        ensure_last_used_at_column(&conn, &index_db_path).await?;
        ensure_setup_script_column(&conn, &index_db_path).await?;
        ensure_archived_at_column(&conn, &index_db_path).await?;

        Ok(Self { paths, conn })
    }

    /// Returns the remote sandboxes enabled for `repo`.
    pub async fn enabled_sandboxes(
        &self,
        repo: &Repository,
    ) -> Result<Vec<SandboxKind>, SplayError> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT integration
                 FROM repo_integrations
                 WHERE host = ?1 AND owner = ?2 AND name = ?3
                 ORDER BY integration",
            )
            .await?;
        let mut rows = stmt
            .query((repo.host.as_str(), repo.owner.as_str(), repo.name.as_str()))
            .await?;
        let mut sandboxes = Vec::new();

        while let Some(row) = rows.next().await? {
            // Skip integrations this version does not know about.
            if let Ok(sandbox) = SandboxKind::parse(&row.get::<String>(0)?)
                && sandbox != SandboxKind::Local
            {
                sandboxes.push(sandbox);
            }
        }

        Ok(sandboxes)
    }

    /// Enables or disables a remote sandbox for `repository`.
    pub async fn set_sandbox_enabled(
        &self,
        repository: &str,
        sandbox: SandboxKind,
        enabled: bool,
    ) -> Result<(), SplayError> {
        let repo = self
            .resolve(repository)
            .await?
            .ok_or_else(|| SplayError::RepositoryNotFound(repository.to_string()))?;
        if sandbox == SandboxKind::Local {
            return Err(SplayError::InvalidSandbox(sandbox.to_string()));
        }

        let key = (
            repo.host.as_str(),
            repo.owner.as_str(),
            repo.name.as_str(),
            sandbox.as_str(),
        );
        self.conn
            .execute(
                "DELETE FROM repo_integrations
                 WHERE host = ?1 AND owner = ?2 AND name = ?3 AND integration = ?4",
                key,
            )
            .await?;
        if enabled {
            self.conn
                .execute(
                    "INSERT INTO repo_integrations (host, owner, name, integration)
                     VALUES (?1, ?2, ?3, ?4)",
                    key,
                )
                .await?;
        }

        Ok(())
    }

    /// Returns the script that sets up new remote workspaces of `repository`.
    pub async fn setup_script(&self, repository: &str) -> Result<Option<String>, SplayError> {
        let repo = self
            .resolve(repository)
            .await?
            .ok_or_else(|| SplayError::RepositoryNotFound(repository.to_string()))?;
        let mut stmt = self
            .conn
            .prepare(
                "SELECT setup_script
                 FROM repos
                 WHERE host = ?1 AND owner = ?2 AND name = ?3",
            )
            .await?;
        let mut rows = stmt
            .query((repo.host.as_str(), repo.owner.as_str(), repo.name.as_str()))
            .await?;

        let Some(row) = rows.next().await? else {
            return Ok(None);
        };
        Ok(row
            .get::<Option<String>>(0)?
            .filter(|script| !script.trim().is_empty()))
    }

    /// Sets the setup script of `repository`; an empty script removes it.
    pub async fn set_setup_script(&self, repository: &str, script: &str) -> Result<(), SplayError> {
        let repo = self
            .resolve(repository)
            .await?
            .ok_or_else(|| SplayError::RepositoryNotFound(repository.to_string()))?;
        let script = Some(script).filter(|script| !script.trim().is_empty());

        self.conn
            .execute(
                "UPDATE repos
                 SET setup_script = ?4
                 WHERE host = ?1 AND owner = ?2 AND name = ?3",
                (
                    repo.host.as_str(),
                    repo.owner.as_str(),
                    repo.name.as_str(),
                    script,
                ),
            )
            .await?;

        Ok(())
    }

    pub async fn add(
        &self,
        repository: &str,
        alias: Option<&str>,
    ) -> Result<Repository, SplayError> {
        let mut repo = Repository::parse(repository, alias)?;

        if self.find_repository(&repo).await?.is_some() {
            return Err(SplayError::DuplicateRepository(repo.canonical()));
        }

        if let Some(alias) = &repo.alias {
            if self.find_alias(alias).await? {
                return Err(SplayError::DuplicateAlias(alias.clone()));
            }
        }

        let repo_dir = self.paths.repo_dir(&repo);
        fs::create_dir_all(&repo_dir)?;
        fs::write(self.paths.repo_meta_path(&repo), render_meta_toml(&repo))?;

        let repo_db_path = self.paths.repo_db_path(&repo);
        let repo_db = Builder::new_local(path_to_string(&repo_db_path)?)
            .build()
            .await
            .map_err(|err| database_error(&repo_db_path, "open", err))?;
        let repo_conn = connect(&repo_db, &repo_db_path)?;
        repo_conn
            .execute_batch(
                "
                CREATE TABLE IF NOT EXISTS workspaces (
                    name TEXT PRIMARY KEY,
                    branch TEXT NOT NULL,
                    path TEXT NOT NULL UNIQUE,
                    created_at INTEGER NOT NULL
                );
                ",
            )
            .await
            .map_err(|err| database_error(&repo_db_path, "initialize schema", err))?;

        let created_at = unix_timestamp();
        repo.last_used_at = Some(created_at);
        self.conn
            .execute(
                "INSERT INTO repos (host, owner, name, alias, remote_url, collapsed, created_at, last_used_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                (
                    repo.host.as_str(),
                    repo.owner.as_str(),
                    repo.name.as_str(),
                    repo.alias.as_deref(),
                    repo.remote_url.as_deref(),
                    repo.collapsed,
                    created_at,
                    created_at,
                ),
            )
            .await?;

        Ok(repo)
    }

    pub async fn list(&self) -> Result<Vec<Repository>, SplayError> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT host, owner, name, alias, collapsed
                 , remote_url, last_used_at, archived_at
                 FROM repos
                 ORDER BY host, owner, name",
            )
            .await?;
        let mut rows = stmt.query(()).await?;
        let mut repos = Vec::new();

        while let Some(row) = rows.next().await? {
            repos.push(Repository {
                host: row.get::<String>(0)?,
                owner: row.get::<String>(1)?,
                name: row.get::<String>(2)?,
                alias: row.get::<Option<String>>(3)?,
                collapsed: row.get::<i64>(4)? != 0,
                remote_url: row.get::<Option<String>>(5)?,
                last_used_at: row.get::<Option<i64>>(6)?,
                archived_at: row.get::<Option<i64>>(7)?,
            });
        }

        Ok(repos)
    }

    pub async fn sync(&self, repository: &str) -> Result<Repository, SplayError> {
        let repo = self
            .resolve(repository)
            .await?
            .ok_or_else(|| SplayError::RepositoryNotFound(repository.to_string()))?;
        let _lock = lock_repo(&repo).await;
        self.sync_repo(&repo)?;

        Ok(repo)
    }

    pub fn sync_repo(&self, repo: &Repository) -> Result<(), SplayError> {
        let repo_dir = self.paths.repo_dir(repo);
        let bare_repo_path = self.bare_repo_path(repo);
        let remote_url = repo.remote_url();

        fs::create_dir_all(&repo_dir)?;

        if bare_repo_path.exists() && !git_is_bare_repository(&bare_repo_path)? {
            fs::remove_dir_all(&bare_repo_path)?;
        }

        if !bare_repo_path.exists() {
            run_git(
                Some(&repo_dir),
                [
                    "init".to_string(),
                    "--bare".to_string(),
                    bare_repo_path.display().to_string(),
                ],
            )?;
            run_git(
                Some(&repo_dir),
                [
                    format!("--git-dir={}", bare_repo_path.display()),
                    "remote".to_string(),
                    "add".to_string(),
                    "origin".to_string(),
                    remote_url.clone(),
                ],
            )?;
        } else if !git_has_remote(&bare_repo_path, "origin")? {
            run_git(
                Some(&repo_dir),
                [
                    format!("--git-dir={}", bare_repo_path.display()),
                    "remote".to_string(),
                    "add".to_string(),
                    "origin".to_string(),
                    remote_url,
                ],
            )?;
        }
        run_git(
            Some(&repo_dir),
            [
                format!("--git-dir={}", bare_repo_path.display()),
                "fetch".to_string(),
                "origin".to_string(),
                "--prune".to_string(),
                "+refs/heads/*:refs/remotes/origin/*".to_string(),
            ],
        )?;
        run_git(
            Some(&repo_dir),
            [
                format!("--git-dir={}", bare_repo_path.display()),
                "remote".to_string(),
                "set-head".to_string(),
                "origin".to_string(),
                "--auto".to_string(),
            ],
        )?;

        Ok(())
    }

    pub async fn resolve_repository(
        &self,
        reference: &str,
    ) -> Result<Option<Repository>, SplayError> {
        self.resolve(reference).await
    }

    pub fn repo_dir(&self, repo: &Repository) -> PathBuf {
        self.paths.repo_dir(repo)
    }

    pub fn repo_db_path(&self, repo: &Repository) -> PathBuf {
        self.paths.repo_db_path(repo)
    }

    pub fn workspaces_dir(&self, repo: &Repository) -> PathBuf {
        self.paths.repo_dir(repo).join("workspaces")
    }

    pub fn bare_repo_path(&self, repo: &Repository) -> PathBuf {
        self.paths.repo_dir(repo).join("source.git")
    }

    /// Returns the default branch recorded by the last sync, without fetching.
    pub fn default_branch(&self, repo: &Repository) -> Option<String> {
        let head = run_git(
            Some(&self.paths.repo_dir(repo)),
            [
                format!("--git-dir={}", self.bare_repo_path(repo).display()),
                "symbolic-ref".to_string(),
                "--short".to_string(),
                "refs/remotes/origin/HEAD".to_string(),
            ],
        )
        .ok()?;
        Some(head.strip_prefix("origin/").unwrap_or(&head).to_string())
    }

    pub fn sessions_dir(&self, repo: &Repository) -> PathBuf {
        self.paths.repo_dir(repo).join("sessions")
    }

    pub async fn remove(&self, repository: &str) -> Result<Repository, SplayError> {
        let repo = self
            .resolve(repository)
            .await?
            .ok_or_else(|| SplayError::RepositoryNotFound(repository.to_string()))?;

        self.conn
            .execute(
                "DELETE FROM repos WHERE host = ?1 AND owner = ?2 AND name = ?3",
                (repo.host.as_str(), repo.owner.as_str(), repo.name.as_str()),
            )
            .await?;

        let repo_dir = self.paths.repo_dir(&repo);
        if repo_dir.exists() {
            fs::remove_dir_all(repo_dir)?;
        }

        Ok(repo)
    }

    pub async fn collapse(&self, repository: &str) -> Result<Repository, SplayError> {
        self.set_collapsed(repository, true).await
    }

    pub async fn expand(&self, repository: &str) -> Result<Repository, SplayError> {
        self.set_collapsed(repository, false).await
    }

    /// Archives a repository, hiding it from the sidebar. Its workspaces,
    /// sessions and files are kept.
    pub async fn archive(&self, repository: &str) -> Result<Repository, SplayError> {
        self.set_archived_at(repository, Some(unix_timestamp()))
            .await
    }

    pub async fn unarchive(&self, repository: &str) -> Result<Repository, SplayError> {
        self.set_archived_at(repository, None).await
    }

    pub async fn set_all_collapsed(&self, collapsed: bool) -> Result<(), SplayError> {
        self.conn
            .execute("UPDATE repos SET collapsed = ?1", (collapsed,))
            .await?;
        Ok(())
    }

    /// Records that a workspace was used, which also counts as using its
    /// repository.
    pub async fn mark_workspace_used(
        &self,
        repository: &str,
        workspace: &str,
    ) -> Result<(), SplayError> {
        let repo = self
            .resolve(repository)
            .await?
            .ok_or_else(|| SplayError::RepositoryNotFound(repository.to_string()))?;
        let canonical = repo.canonical();
        let now = unix_timestamp();

        self.conn
            .execute(
                "UPDATE repos
                 SET last_used_at = ?4
                 WHERE host = ?1 AND owner = ?2 AND name = ?3",
                (
                    repo.host.as_str(),
                    repo.owner.as_str(),
                    repo.name.as_str(),
                    now,
                ),
            )
            .await?;

        let updated = self
            .conn
            .execute(
                "UPDATE workspace_usage
                 SET last_used_at = ?3
                 WHERE repository = ?1 AND workspace = ?2",
                (canonical.as_str(), workspace, now),
            )
            .await?;
        if updated == 0 {
            self.conn
                .execute(
                    "INSERT INTO workspace_usage (repository, workspace, last_used_at)
                     VALUES (?1, ?2, ?3)",
                    (canonical.as_str(), workspace, now),
                )
                .await?;
        }

        Ok(())
    }

    /// Returns when each workspace of a repository was last used, keyed by
    /// workspace name.
    pub async fn workspace_last_used(
        &self,
        repo: &Repository,
    ) -> Result<HashMap<String, i64>, SplayError> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT workspace, last_used_at
                 FROM workspace_usage
                 WHERE repository = ?1",
            )
            .await?;
        let mut rows = stmt.query([repo.canonical()]).await?;
        let mut last_used = HashMap::new();

        while let Some(row) = rows.next().await? {
            last_used.insert(row.get::<String>(0)?, row.get::<i64>(1)?);
        }

        Ok(last_used)
    }

    async fn find_repository(&self, repo: &Repository) -> Result<Option<Repository>, SplayError> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT host, owner, name, alias, collapsed
                 , remote_url, last_used_at, archived_at
                 FROM repos
                 WHERE host = ?1 AND owner = ?2 AND name = ?3",
            )
            .await?;
        let mut rows = stmt
            .query((repo.host.as_str(), repo.owner.as_str(), repo.name.as_str()))
            .await?;

        if let Some(row) = rows.next().await? {
            return Ok(Some(Repository {
                host: row.get::<String>(0)?,
                owner: row.get::<String>(1)?,
                name: row.get::<String>(2)?,
                alias: row.get::<Option<String>>(3)?,
                collapsed: row.get::<i64>(4)? != 0,
                remote_url: row.get::<Option<String>>(5)?,
                last_used_at: row.get::<Option<i64>>(6)?,
                archived_at: row.get::<Option<i64>>(7)?,
            }));
        }

        Ok(None)
    }

    async fn find_alias(&self, alias: &str) -> Result<bool, SplayError> {
        let mut stmt = self
            .conn
            .prepare("SELECT 1 FROM repos WHERE alias = ?1 LIMIT 1")
            .await?;
        let mut rows = stmt.query([alias]).await?;
        Ok(rows.next().await?.is_some())
    }

    async fn resolve(&self, reference: &str) -> Result<Option<Repository>, SplayError> {
        if let Some(repo) = self.find_by_alias(reference).await? {
            return Ok(Some(repo));
        }

        let parsed = Repository::parse(reference, Some("placeholder"));
        if let Ok(repo) = parsed {
            return self
                .find_repository(&Repository {
                    host: repo.host,
                    owner: repo.owner,
                    name: repo.name,
                    alias: None,
                    remote_url: None,
                    collapsed: false,
                    last_used_at: None,
                    archived_at: None,
                })
                .await;
        }

        Ok(None)
    }

    async fn find_by_alias(&self, alias: &str) -> Result<Option<Repository>, SplayError> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT host, owner, name, alias, collapsed
                 , remote_url, last_used_at, archived_at
                 FROM repos
                 WHERE alias = ?1
                 LIMIT 1",
            )
            .await?;
        let mut rows = stmt.query([alias]).await?;

        if let Some(row) = rows.next().await? {
            return Ok(Some(Repository {
                host: row.get::<String>(0)?,
                owner: row.get::<String>(1)?,
                name: row.get::<String>(2)?,
                alias: row.get::<Option<String>>(3)?,
                collapsed: row.get::<i64>(4)? != 0,
                remote_url: row.get::<Option<String>>(5)?,
                last_used_at: row.get::<Option<i64>>(6)?,
                archived_at: row.get::<Option<i64>>(7)?,
            }));
        }

        Ok(None)
    }

    async fn set_collapsed(
        &self,
        repository: &str,
        collapsed: bool,
    ) -> Result<Repository, SplayError> {
        let repo = self
            .resolve(repository)
            .await?
            .ok_or_else(|| SplayError::RepositoryNotFound(repository.to_string()))?;

        self.conn
            .execute(
                "UPDATE repos
                 SET collapsed = ?4
                 WHERE host = ?1 AND owner = ?2 AND name = ?3",
                (
                    repo.host.as_str(),
                    repo.owner.as_str(),
                    repo.name.as_str(),
                    collapsed,
                ),
            )
            .await?;

        Ok(Repository { collapsed, ..repo })
    }

    async fn set_archived_at(
        &self,
        repository: &str,
        archived_at: Option<i64>,
    ) -> Result<Repository, SplayError> {
        let repo = self
            .resolve(repository)
            .await?
            .ok_or_else(|| SplayError::RepositoryNotFound(repository.to_string()))?;

        self.conn
            .execute(
                "UPDATE repos
                 SET archived_at = ?4
                 WHERE host = ?1 AND owner = ?2 AND name = ?3",
                (
                    repo.host.as_str(),
                    repo.owner.as_str(),
                    repo.name.as_str(),
                    archived_at,
                ),
            )
            .await?;

        Ok(Repository {
            archived_at,
            ..repo
        })
    }
}

/// Opens the index database. A process holds the database file locked while
/// it has it open, so wait briefly for other Splay processes, such as a
/// template builder, to close it.
async fn open_index_db(path: &Path) -> Result<Database, SplayError> {
    let mut attempt = 1;
    loop {
        match Builder::new_local(path_to_string(path)?).build().await {
            Ok(db) => return Ok(db),
            Err(err) if attempt < INDEX_DB_OPEN_ATTEMPTS && err.to_string().contains("lock") => {
                attempt += 1;
                tokio::time::sleep(INDEX_DB_OPEN_RETRY_DELAY).await;
            }
            Err(err) => return Err(database_error(path, "open", err)),
        }
    }
}

async fn ensure_collapsed_column(conn: &Connection, path: &Path) -> Result<(), SplayError> {
    if has_column(conn, "repos", "collapsed").await? {
        return Ok(());
    }

    conn.execute(
        "ALTER TABLE repos ADD COLUMN collapsed INTEGER NOT NULL DEFAULT 0",
        (),
    )
    .await
    .map_err(|err| database_error(path, "add collapsed column", err))?;

    Ok(())
}

async fn ensure_remote_url_column(conn: &Connection, path: &Path) -> Result<(), SplayError> {
    if has_column(conn, "repos", "remote_url").await? {
        return Ok(());
    }

    conn.execute("ALTER TABLE repos ADD COLUMN remote_url TEXT", ())
        .await
        .map_err(|err| database_error(path, "add remote_url column", err))?;

    Ok(())
}

async fn ensure_last_used_at_column(conn: &Connection, path: &Path) -> Result<(), SplayError> {
    if has_column(conn, "repos", "last_used_at").await? {
        return Ok(());
    }

    conn.execute("ALTER TABLE repos ADD COLUMN last_used_at INTEGER", ())
        .await
        .map_err(|err| database_error(path, "add last_used_at column", err))?;

    Ok(())
}

async fn ensure_archived_at_column(conn: &Connection, path: &Path) -> Result<(), SplayError> {
    if has_column(conn, "repos", "archived_at").await? {
        return Ok(());
    }

    conn.execute("ALTER TABLE repos ADD COLUMN archived_at INTEGER", ())
        .await
        .map_err(|err| database_error(path, "add archived_at column", err))?;

    Ok(())
}

async fn ensure_setup_script_column(conn: &Connection, path: &Path) -> Result<(), SplayError> {
    if has_column(conn, "repos", "setup_script").await? {
        return Ok(());
    }

    conn.execute("ALTER TABLE repos ADD COLUMN setup_script TEXT", ())
        .await
        .map_err(|err| database_error(path, "add setup_script column", err))?;

    Ok(())
}

async fn has_column(conn: &Connection, table: &str, column: &str) -> Result<bool, SplayError> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})")).await?;
    let mut rows = stmt.query(()).await?;

    while let Some(row) = rows.next().await? {
        if row.get::<String>(1)? == column {
            return Ok(true);
        }
    }

    Ok(false)
}

fn path_to_string(path: &Path) -> Result<&str, SplayError> {
    path.to_str().ok_or(SplayError::PathResolution)
}

fn git_is_bare_repository(path: &Path) -> Result<bool, SplayError> {
    let output = Command::new("git")
        .arg(format!("--git-dir={}", path.display()))
        .args(["rev-parse", "--is-bare-repository"])
        .output()?;

    if !output.status.success() {
        return Ok(false);
    }

    Ok(String::from_utf8_lossy(&output.stdout).trim() == "true")
}

fn git_has_remote(path: &Path, remote: &str) -> Result<bool, SplayError> {
    let output = Command::new("git")
        .arg(format!("--git-dir={}", path.display()))
        .args(["remote", "get-url", remote])
        .output()?;

    Ok(output.status.success())
}

fn run_git<I, S>(cwd: Option<&Path>, args: I) -> Result<String, SplayError>
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
        return Err(SplayError::Git(render_git_failure(output)));
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

fn unix_timestamp() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

fn render_meta_toml(repo: &Repository) -> String {
    let mut out = String::new();
    out.push_str(&format!("host = {:?}\n", repo.host));
    out.push_str(&format!("owner = {:?}\n", repo.owner));
    out.push_str(&format!("name = {:?}\n", repo.name));
    out.push_str(&format!("canonical = {:?}\n", repo.canonical()));

    match &repo.alias {
        Some(alias) => out.push_str(&format!("alias = {:?}\n", alias)),
        None => {}
    }

    match &repo.remote_url {
        Some(remote_url) => out.push_str(&format!("remote_url = {:?}\n", remote_url)),
        None => {}
    }

    out
}

fn resolve_remote_host(host: &str) -> &str {
    match host {
        "github" => "github.com",
        "gitlab" => "gitlab.com",
        "codeberg" => "codeberg.org",
        "bitbucket" => "bitbucket.org",
        _ => host,
    }
}

fn parse_canonical_reference(input: &str) -> Option<(String, String, String)> {
    let mut parts = input.split('/');
    let host = parts.next()?.trim();
    let owner = parts.next()?.trim();
    let name = parts.next()?.trim();

    if host.is_empty()
        || owner.is_empty()
        || name.is_empty()
        || parts.next().is_some()
        || [host, owner, name]
            .iter()
            .any(|part| part.contains(char::is_whitespace))
    {
        return None;
    }

    Some((host.to_string(), owner.to_string(), name.to_string()))
}

fn parse_remote_reference(input: &str) -> Option<(String, String, String, Option<String>)> {
    let input = input.trim();

    if let Some((_, remainder)) = input.split_once("://") {
        let (authority, path) = split_remote_authority_and_path(remainder)?;
        let host = authority
            .rsplit_once('@')
            .map_or(authority, |(_, host)| host);
        let (owner, name) = parse_owner_and_name(path)?;
        return Some((host.to_string(), owner, name, Some(input.to_string())));
    }

    let colon_index = input.find(':')?;
    let slash_index = input.find('/')?;
    if colon_index > slash_index {
        return None;
    }

    let (authority, path) = input.split_at(colon_index);
    let path = path.strip_prefix(':')?;
    let host = authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host);
    let (owner, name) = parse_owner_and_name(path)?;

    Some((host.to_string(), owner, name, Some(input.to_string())))
}

fn split_remote_authority_and_path(input: &str) -> Option<(&str, &str)> {
    let (authority, path) = input.split_once('/')?;
    if authority.is_empty() || path.is_empty() {
        return None;
    }

    Some((authority, path))
}

fn parse_owner_and_name(path: &str) -> Option<(String, String)> {
    let path = path.trim_matches('/');
    let (owner, name) = path.split_once('/')?;
    if owner.is_empty() || name.is_empty() || name.contains('/') {
        return None;
    }

    let name = name.strip_suffix(".git").unwrap_or(name);
    if [owner, name]
        .iter()
        .any(|part| part.is_empty() || part.contains(char::is_whitespace))
    {
        return None;
    }

    Some((owner.to_string(), name.to_string()))
}

#[cfg(test)]
mod tests {
    use super::Repository;

    #[test]
    fn shorthand_repo_defaults_to_https_remote() {
        let repo = Repository {
            host: "github".to_string(),
            owner: "penberg".to_string(),
            name: "splay".to_string(),
            alias: None,
            remote_url: None,
            collapsed: false,
            last_used_at: None,
            archived_at: None,
        };

        assert_eq!(repo.remote_url(), "https://github.com/penberg/splay.git");
    }

    #[test]
    fn parses_https_remote_url() {
        let repo = Repository::parse("https://github.com/penberg/splay.git", None).unwrap();

        assert_eq!(
            (repo.host.as_str(), repo.owner.as_str(), repo.name.as_str()),
            ("github.com", "penberg", "splay")
        );
        assert_eq!(
            repo.remote_url.as_deref(),
            Some("https://github.com/penberg/splay.git")
        );
    }

    #[test]
    fn parses_ssh_remote_url() {
        let repo = Repository::parse("git@github.com:penberg/splay.git", None).unwrap();

        assert_eq!(
            (repo.host.as_str(), repo.owner.as_str(), repo.name.as_str()),
            ("github.com", "penberg", "splay")
        );
        assert_eq!(
            repo.remote_url.as_deref(),
            Some("git@github.com:penberg/splay.git")
        );
    }

    #[test]
    fn parses_ssh_scheme_remote_url() {
        let repo = Repository::parse("ssh://git@github.com/penberg/splay.git", None).unwrap();

        assert_eq!(
            (repo.host.as_str(), repo.owner.as_str(), repo.name.as_str()),
            ("github.com", "penberg", "splay")
        );
        assert_eq!(
            repo.remote_url.as_deref(),
            Some("ssh://git@github.com/penberg/splay.git")
        );
    }
}
