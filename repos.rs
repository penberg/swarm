use directories::ProjectDirs;
use serde::Serialize;
use std::{
    fs,
    path::{Path, PathBuf},
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
}

impl Repository {
    pub fn parse(input: &str, alias: Option<&str>) -> Result<Self, SwarmError> {
        let (host, owner, name) = parse_repository_input(input)?;

        let default_alias = alias
            .map(str::trim)
            .filter(|alias| !alias.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| name.to_string());

        Ok(Self {
            host: host.to_string(),
            owner: owner.to_string(),
            name: name.to_string(),
            alias: Some(default_alias),
        })
    }

    pub fn canonical(&self) -> String {
        format!("{}/{}/{}", self.host, self.owner, self.name)
    }

    pub fn remote_url(&self) -> String {
        format!(
            "https://{}/{}/{}.git",
            resolve_remote_host(&self.host),
            self.owner,
            self.name
        )
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
                "INSERT INTO repos (host, owner, name, alias, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
                (
                    repo.host.as_str(),
                    repo.owner.as_str(),
                    repo.name.as_str(),
                    repo.alias.as_deref(),
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
                "SELECT host, owner, name, alias
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
            });
        }

        Ok(repos)
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

    async fn find_repository(&self, repo: &Repository) -> Result<Option<Repository>, SwarmError> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT host, owner, name, alias
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
                })
                .await;
        }

        Ok(None)
    }

    async fn find_by_alias(&self, alias: &str) -> Result<Option<Repository>, SwarmError> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT host, owner, name, alias
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
            }));
        }

        Ok(None)
    }
}

fn path_to_string(path: &Path) -> Result<&str, SwarmError> {
    path.to_str().ok_or(SwarmError::PathResolution)
}

fn parse_repository_input(input: &str) -> Result<(&str, &str, &str), SwarmError> {
    let input = input.trim();

    if let Some(repo) = parse_repository_url(input) {
        return Ok(repo);
    }

    parse_canonical_repository(input)
        .ok_or_else(|| SwarmError::InvalidRepository(input.to_string()))
}

fn parse_repository_url(input: &str) -> Option<(&str, &str, &str)> {
    if let Some(rest) = input
        .strip_prefix("https://")
        .or_else(|| input.strip_prefix("http://"))
    {
        return parse_url_like_repository(rest);
    }

    if let Some(rest) = input.strip_prefix("ssh://") {
        return parse_url_like_repository(rest.strip_prefix("git@").unwrap_or(rest));
    }

    if let Some(rest) = input.strip_prefix("git@") {
        let (host, path) = rest.split_once(':')?;
        return parse_repository_path(host, path, true);
    }

    None
}

fn parse_url_like_repository(input: &str) -> Option<(&str, &str, &str)> {
    let (authority, path) = input.split_once('/')?;
    let host = authority
        .rsplit_once('@')
        .map(|(_, host)| host)
        .unwrap_or(authority);

    parse_repository_path(host, path, true)
}

fn parse_canonical_repository(input: &str) -> Option<(&str, &str, &str)> {
    let mut parts = input.split('/');
    let host = parts.next()?.trim();
    let owner = parts.next()?.trim();
    let name = parts.next()?.trim_end_matches(".git").trim();

    if parts.next().is_some() {
        return None;
    }

    validate_repository_parts(host, owner, name).then_some((host, owner, name))
}

fn parse_repository_path<'a>(
    host: &'a str,
    path: &'a str,
    allow_extra_segments: bool,
) -> Option<(&'a str, &'a str, &'a str)> {
    let mut parts = path.trim_matches('/').split('/');
    let owner = parts.next()?.trim();
    let name = parts.next()?.trim_end_matches(".git").trim();

    if !allow_extra_segments && parts.next().is_some() {
        return None;
    }

    validate_repository_parts(host.trim(), owner, name).then_some((host.trim(), owner, name))
}

fn validate_repository_parts(host: &str, owner: &str, name: &str) -> bool {
    !host.is_empty()
        && !owner.is_empty()
        && !name.is_empty()
        && [host, owner, name]
            .iter()
            .all(|part| !part.contains(char::is_whitespace))
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

#[cfg(test)]
mod tests {
    use super::Repository;

    #[test]
    fn parses_canonical_repository() {
        let repo = Repository::parse("github.com/penberg/swarm", None).unwrap();

        assert_eq!(repo.host, "github.com");
        assert_eq!(repo.owner, "penberg");
        assert_eq!(repo.name, "swarm");
        assert_eq!(repo.alias.as_deref(), Some("swarm"));
    }

    #[test]
    fn parses_https_github_repository_url() {
        let repo = Repository::parse("https://github.com/penberg/swarm", None).unwrap();

        assert_eq!(repo.canonical(), "github.com/penberg/swarm");
    }

    #[test]
    fn parses_github_repository_url_with_trailing_segments() {
        let repo =
            Repository::parse("https://github.com/penberg/swarm/pull/123", Some("local")).unwrap();

        assert_eq!(repo.canonical(), "github.com/penberg/swarm");
        assert_eq!(repo.alias.as_deref(), Some("local"));
    }

    #[test]
    fn parses_git_remote_urls() {
        let https_repo = Repository::parse("https://github.com/penberg/swarm.git", None).unwrap();
        let ssh_repo = Repository::parse("git@github.com:penberg/swarm.git", None).unwrap();

        assert_eq!(https_repo.canonical(), "github.com/penberg/swarm");
        assert_eq!(ssh_repo.canonical(), "github.com/penberg/swarm");
    }
}
