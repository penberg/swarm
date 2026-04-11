use directories::ProjectDirs;
use serde::Serialize;
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};
use turso::{Builder, Connection};

use crate::{SwarmError, database_error};

#[derive(Debug, Clone, Serialize)]
pub struct Repository {
    pub host: String,
    pub owner: String,
    pub name: String,
    pub alias: Option<String>,
    pub remote_url: Option<String>,
    pub collapsed: bool,
}

impl Repository {
    pub fn parse(input: &str, alias: Option<&str>) -> Result<Self, SwarmError> {
        let input = input.trim();
        let (host, owner, name, remote_url) = parse_remote_reference(input)
            .or_else(|| {
                parse_canonical_reference(input)
                    .map(|(host, owner, name)| (host, owner, name, None))
            })
            .ok_or_else(|| SwarmError::InvalidRepository(input.to_string()))?;

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
struct SwarmPaths {
    data_dir: PathBuf,
    repos_dir: PathBuf,
    index_db_path: PathBuf,
}

impl SwarmPaths {
    fn resolve() -> Result<Self, SwarmError> {
        let dirs =
            ProjectDirs::from("com", "penberg", "swarm").ok_or(SwarmError::PathResolution)?;
        let data_dir = dirs.data_dir().to_path_buf();
        let repos_dir = data_dir.join("repos");
        let index_db_path = data_dir.join("index.db");

        Ok(Self {
            data_dir,
            repos_dir,
            index_db_path,
        })
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

pub struct RepositoryStore {
    paths: SwarmPaths,
    conn: Connection,
}

impl RepositoryStore {
    pub async fn open() -> Result<Self, SwarmError> {
        let paths = SwarmPaths::resolve()?;
        fs::create_dir_all(&paths.data_dir)?;
        fs::create_dir_all(&paths.repos_dir)?;

        let index_db_path = paths.index_db_path.clone();
        let db = Builder::new_local(path_to_string(&paths.index_db_path)?)
            .build()
            .await
            .map_err(|err| database_error(&index_db_path, "open", err))?;
        let conn = db
            .connect()
            .map_err(|err| database_error(&index_db_path, "connect", err))?;

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
                PRIMARY KEY (host, owner, name)
            );

            CREATE UNIQUE INDEX IF NOT EXISTS repos_alias_idx
            ON repos(alias)
            WHERE alias IS NOT NULL;
            ",
        )
        .await
        .map_err(|err| database_error(&index_db_path, "initialize schema", err))?;
        ensure_collapsed_column(&conn, &index_db_path).await?;
        ensure_remote_url_column(&conn, &index_db_path).await?;

        Ok(Self { paths, conn })
    }

    pub async fn add(
        &self,
        repository: &str,
        alias: Option<&str>,
    ) -> Result<Repository, SwarmError> {
        let repo = Repository::parse(repository, alias)?;

        if self.find_repository(&repo).await?.is_some() {
            return Err(SwarmError::DuplicateRepository(repo.canonical()));
        }

        if let Some(alias) = &repo.alias {
            if self.find_alias(alias).await? {
                return Err(SwarmError::DuplicateAlias(alias.clone()));
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
        let repo_conn = repo_db
            .connect()
            .map_err(|err| database_error(&repo_db_path, "connect", err))?;
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

        self.conn
            .execute(
                "INSERT INTO repos (host, owner, name, alias, remote_url, collapsed, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                (
                    repo.host.as_str(),
                    repo.owner.as_str(),
                    repo.name.as_str(),
                    repo.alias.as_deref(),
                    repo.remote_url.as_deref(),
                    repo.collapsed,
                    unix_timestamp(),
                ),
            )
            .await?;

        Ok(repo)
    }

    pub async fn list(&self) -> Result<Vec<Repository>, SwarmError> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT host, owner, name, alias, collapsed
                 , remote_url
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
            });
        }

        Ok(repos)
    }

    pub async fn sync(&self, repository: &str) -> Result<Repository, SwarmError> {
        let repo = self
            .resolve(repository)
            .await?
            .ok_or_else(|| SwarmError::RepositoryNotFound(repository.to_string()))?;
        self.sync_repo(&repo)?;

        Ok(repo)
    }

    pub fn sync_repo(&self, repo: &Repository) -> Result<(), SwarmError> {
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
    ) -> Result<Option<Repository>, SwarmError> {
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

    pub fn sessions_dir(&self, repo: &Repository) -> PathBuf {
        self.paths.repo_dir(repo).join("sessions")
    }

    pub async fn remove(&self, repository: &str) -> Result<Repository, SwarmError> {
        let repo = self
            .resolve(repository)
            .await?
            .ok_or_else(|| SwarmError::RepositoryNotFound(repository.to_string()))?;

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

    pub async fn collapse(&self, repository: &str) -> Result<Repository, SwarmError> {
        self.set_collapsed(repository, true).await
    }

    pub async fn expand(&self, repository: &str) -> Result<Repository, SwarmError> {
        self.set_collapsed(repository, false).await
    }

    async fn find_repository(&self, repo: &Repository) -> Result<Option<Repository>, SwarmError> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT host, owner, name, alias, collapsed
                 , remote_url
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
            }));
        }

        Ok(None)
    }

    async fn find_alias(&self, alias: &str) -> Result<bool, SwarmError> {
        let mut stmt = self
            .conn
            .prepare("SELECT 1 FROM repos WHERE alias = ?1 LIMIT 1")
            .await?;
        let mut rows = stmt.query([alias]).await?;
        Ok(rows.next().await?.is_some())
    }

    async fn resolve(&self, reference: &str) -> Result<Option<Repository>, SwarmError> {
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
                })
                .await;
        }

        Ok(None)
    }

    async fn find_by_alias(&self, alias: &str) -> Result<Option<Repository>, SwarmError> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT host, owner, name, alias, collapsed
                 , remote_url
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
            }));
        }

        Ok(None)
    }

    async fn set_collapsed(
        &self,
        repository: &str,
        collapsed: bool,
    ) -> Result<Repository, SwarmError> {
        let repo = self
            .resolve(repository)
            .await?
            .ok_or_else(|| SwarmError::RepositoryNotFound(repository.to_string()))?;

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
}

async fn ensure_collapsed_column(conn: &Connection, path: &Path) -> Result<(), SwarmError> {
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

async fn ensure_remote_url_column(conn: &Connection, path: &Path) -> Result<(), SwarmError> {
    if has_column(conn, "repos", "remote_url").await? {
        return Ok(());
    }

    conn.execute("ALTER TABLE repos ADD COLUMN remote_url TEXT", ())
        .await
        .map_err(|err| database_error(path, "add remote_url column", err))?;

    Ok(())
}

async fn has_column(conn: &Connection, table: &str, column: &str) -> Result<bool, SwarmError> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})")).await?;
    let mut rows = stmt.query(()).await?;

    while let Some(row) = rows.next().await? {
        if row.get::<String>(1)? == column {
            return Ok(true);
        }
    }

    Ok(false)
}

fn path_to_string(path: &Path) -> Result<&str, SwarmError> {
    path.to_str().ok_or(SwarmError::PathResolution)
}

fn git_is_bare_repository(path: &Path) -> Result<bool, SwarmError> {
    let output = Command::new("git")
        .arg(format!("--git-dir={}", path.display()))
        .args(["rev-parse", "--is-bare-repository"])
        .output()?;

    if !output.status.success() {
        return Ok(false);
    }

    Ok(String::from_utf8_lossy(&output.stdout).trim() == "true")
}

fn git_has_remote(path: &Path, remote: &str) -> Result<bool, SwarmError> {
    let output = Command::new("git")
        .arg(format!("--git-dir={}", path.display()))
        .args(["remote", "get-url", remote])
        .output()?;

    Ok(output.status.success())
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
            name: "swarm".to_string(),
            alias: None,
            remote_url: None,
            collapsed: false,
        };

        assert_eq!(repo.remote_url(), "https://github.com/penberg/swarm.git");
    }

    #[test]
    fn parses_https_remote_url() {
        let repo = Repository::parse("https://github.com/penberg/swarm.git", None).unwrap();

        assert_eq!(
            (repo.host.as_str(), repo.owner.as_str(), repo.name.as_str()),
            ("github.com", "penberg", "swarm")
        );
        assert_eq!(
            repo.remote_url.as_deref(),
            Some("https://github.com/penberg/swarm.git")
        );
    }

    #[test]
    fn parses_ssh_remote_url() {
        let repo = Repository::parse("git@github.com:penberg/swarm.git", None).unwrap();

        assert_eq!(
            (repo.host.as_str(), repo.owner.as_str(), repo.name.as_str()),
            ("github.com", "penberg", "swarm")
        );
        assert_eq!(
            repo.remote_url.as_deref(),
            Some("git@github.com:penberg/swarm.git")
        );
    }

    #[test]
    fn parses_ssh_scheme_remote_url() {
        let repo = Repository::parse("ssh://git@github.com/penberg/swarm.git", None).unwrap();

        assert_eq!(
            (repo.host.as_str(), repo.owner.as_str(), repo.name.as_str()),
            ("github.com", "penberg", "swarm")
        );
        assert_eq!(
            repo.remote_url.as_deref(),
            Some("ssh://git@github.com/penberg/swarm.git")
        );
    }
}
