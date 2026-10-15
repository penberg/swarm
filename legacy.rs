//! One-time migration of state from the data directory used before Splay was
//! renamed from Swarm.
//!
//! The legacy directory is moved into place, absolute paths recorded in each
//! repository database are rewritten, and the git worktree links between the
//! bare clone and its workspaces are repaired. A symlink is left at the old
//! location so paths captured elsewhere (shell history, agent transcripts)
//! keep resolving.

use std::{
    fs, io,
    os::unix,
    path::{Path, PathBuf},
    process::Command,
};
use turso::{Builder, Connection};

use crate::{
    SplayError, connect, database_error, sessions::process_exists, workspaces::migrate_repo_db,
};

pub(crate) async fn migrate_data_dir(legacy_dir: &Path, data_dir: &Path) -> Result<(), SplayError> {
    if fs::symlink_metadata(data_dir).is_ok() || !legacy_dir.is_dir() {
        return Ok(());
    }

    let legacy_repo_dbs = find_repo_dbs(&legacy_dir.join("repos"))?;

    let mut running = 0;
    for repo_db in &legacy_repo_dbs {
        running += count_running_sessions(repo_db).await?;
    }
    if running > 0 {
        return Err(SplayError::LegacyDataDirInUse {
            path: legacy_dir.to_path_buf(),
            sessions: running,
        });
    }

    if let Some(parent) = data_dir.parent() {
        fs::create_dir_all(parent)?;
    }
    if let Err(err) = fs::rename(legacy_dir, data_dir) {
        // Another process got here first.
        if data_dir.exists() {
            return Ok(());
        }
        return Err(err.into());
    }
    if let Err(err) = unix::fs::symlink(data_dir, legacy_dir) {
        eprintln!(
            "warning: failed to link {} to {}: {err}",
            legacy_dir.display(),
            data_dir.display()
        );
    }

    for legacy_repo_db in legacy_repo_dbs {
        let Ok(relative) = legacy_repo_db.strip_prefix(legacy_dir) else {
            continue;
        };
        let repo_db = data_dir.join(relative);
        let workspace_paths = rewrite_repo_db_paths(&repo_db, legacy_dir, data_dir).await?;
        let Some(repo_dir) = repo_db.parent() else {
            continue;
        };
        repair_worktrees(&repo_dir.join("source.git"), &workspace_paths);
    }

    Ok(())
}

/// Finds `repos/<host>/<owner>/<name>/repo.db` files.
fn find_repo_dbs(repos_dir: &Path) -> Result<Vec<PathBuf>, SplayError> {
    let mut repo_dbs = Vec::new();
    for host in read_subdirs(repos_dir)? {
        for owner in read_subdirs(&host)? {
            for repo in read_subdirs(&owner)? {
                let repo_db = repo.join("repo.db");
                if repo_db.is_file() {
                    repo_dbs.push(repo_db);
                }
            }
        }
    }
    Ok(repo_dbs)
}

fn read_subdirs(dir: &Path) -> Result<Vec<PathBuf>, SplayError> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(err.into()),
    };
    let mut dirs = Vec::new();
    for entry in entries {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            dirs.push(entry.path());
        }
    }
    Ok(dirs)
}

async fn open_repo_db(path: &Path) -> Result<Connection, SplayError> {
    let db = Builder::new_local(path.to_str().ok_or(SplayError::PathResolution)?)
        .build()
        .await
        .map_err(|err| database_error(path, "open", err))?;
    let conn = connect(&db, path)?;
    migrate_repo_db(&conn, path).await?;
    Ok(conn)
}

async fn count_running_sessions(repo_db: &Path) -> Result<usize, SplayError> {
    let conn = open_repo_db(repo_db).await?;
    let mut stmt = conn
        .prepare(
            "SELECT pid FROM sessions
             WHERE status IN ('running', 'starting') AND pid IS NOT NULL",
        )
        .await?;
    let mut rows = stmt.query(()).await?;
    let mut running = 0;
    while let Some(row) = rows.next().await? {
        if process_exists(row.get::<i64>(0)? as u32)? {
            running += 1;
        }
    }
    Ok(running)
}

/// Rewrites absolute paths under `from` to live under `to`, returning the
/// workspace paths after the rewrite.
async fn rewrite_repo_db_paths(
    repo_db: &Path,
    from: &Path,
    to: &Path,
) -> Result<Vec<PathBuf>, SplayError> {
    let conn = open_repo_db(repo_db).await?;

    let mut workspaces = Vec::new();
    let mut stmt = conn.prepare("SELECT name, path FROM workspaces").await?;
    let mut rows = stmt.query(()).await?;
    while let Some(row) = rows.next().await? {
        workspaces.push((row.get::<String>(0)?, row.get::<String>(1)?));
    }

    let mut workspace_paths = Vec::new();
    for (name, path) in workspaces {
        let new_path = rebase(&path, from, to);
        if let Some(new_path) = &new_path {
            conn.execute(
                "UPDATE workspaces SET path = ?2 WHERE name = ?1",
                (name.as_str(), new_path.as_str()),
            )
            .await
            .map_err(|err| database_error(repo_db, "migrate workspace path", err))?;
        }
        workspace_paths.push(PathBuf::from(new_path.unwrap_or(path)));
    }

    let mut sessions = Vec::new();
    let mut stmt = conn
        .prepare("SELECT id, path, log_path, socket_path FROM sessions")
        .await?;
    let mut rows = stmt.query(()).await?;
    while let Some(row) = rows.next().await? {
        sessions.push((
            row.get::<String>(0)?,
            row.get::<String>(1)?,
            row.get::<String>(2)?,
            row.get::<String>(3)?,
        ));
    }

    for (id, path, log_path, socket_path) in sessions {
        let path = rebase(&path, from, to).unwrap_or(path);
        let log_path = rebase(&log_path, from, to).unwrap_or(log_path);
        let socket_path = rebase(&socket_path, from, to).unwrap_or(socket_path);
        conn.execute(
            "UPDATE sessions SET path = ?2, log_path = ?3, socket_path = ?4 WHERE id = ?1",
            (
                id.as_str(),
                path.as_str(),
                log_path.as_str(),
                socket_path.as_str(),
            ),
        )
        .await
        .map_err(|err| database_error(repo_db, "migrate session paths", err))?;
    }

    Ok(workspace_paths)
}

fn rebase(path: &str, from: &Path, to: &Path) -> Option<String> {
    let relative = Path::new(path).strip_prefix(from).ok()?;
    to.join(relative).to_str().map(str::to_string)
}

/// Points the bare clone and its worktrees back at each other after the move.
fn repair_worktrees(bare_repo: &Path, workspace_paths: &[PathBuf]) {
    let existing: Vec<&PathBuf> = workspace_paths
        .iter()
        .filter(|path| path.exists())
        .collect();
    if !bare_repo.is_dir() || existing.is_empty() {
        return;
    }

    let result = Command::new("git")
        .arg(format!("--git-dir={}", bare_repo.display()))
        .arg("worktree")
        .arg("repair")
        .args(existing)
        .output();
    match result {
        Ok(output) if output.status.success() => {}
        Ok(output) => eprintln!(
            "warning: git worktree repair failed for {}: {}",
            bare_repo.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        ),
        Err(err) => eprintln!(
            "warning: git worktree repair failed for {}: {err}",
            bare_repo.display()
        ),
    }
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::PathBuf,
        process::Command,
        time::{SystemTime, UNIX_EPOCH},
    };

    use super::{migrate_data_dir, open_repo_db};

    fn unique_temp_path(prefix: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("{prefix}-{nonce}"))
    }

    fn git(args: &[&str]) {
        let output = Command::new("git")
            .args(args)
            .env("GIT_AUTHOR_NAME", "Splay Tests")
            .env("GIT_AUTHOR_EMAIL", "splay-tests@example.com")
            .env("GIT_COMMITTER_NAME", "Splay Tests")
            .env("GIT_COMMITTER_EMAIL", "splay-tests@example.com")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[tokio::test]
    async fn migrates_legacy_data_dir() {
        let root = unique_temp_path("splay-legacy-migration");
        let legacy_dir = root.join("swarm");
        let data_dir = root.join("splay");
        let repo_dir = legacy_dir.join("repos/github.com/penberg/weave");
        let bare_repo = repo_dir.join("source.git");
        let workspace = repo_dir.join("workspaces/main");
        let session = repo_dir.join("sessions/s1");

        let seed = root.join("seed");
        fs::create_dir_all(&seed).unwrap();
        git(&["-C", seed.to_str().unwrap(), "init", "-q", "-b", "main"]);
        git(&[
            "-C",
            seed.to_str().unwrap(),
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "init",
        ]);
        git(&[
            "clone",
            "-q",
            "--bare",
            seed.to_str().unwrap(),
            bare_repo.to_str().unwrap(),
        ]);
        git(&[
            &format!("--git-dir={}", bare_repo.display()),
            "worktree",
            "add",
            "-q",
            workspace.to_str().unwrap(),
            "main",
        ]);
        fs::create_dir_all(&session).unwrap();

        let repo_db = repo_dir.join("repo.db");
        {
            let conn = open_repo_db(&repo_db).await.unwrap();
            conn.execute(
                "INSERT INTO workspaces (name, branch, path, created_at) VALUES ('main', 'main', ?1, 1)",
                (workspace.to_str().unwrap(),),
            )
            .await
            .unwrap();
            conn.execute(
                "INSERT INTO sessions (id, workspace_name, command_json, path, log_path, socket_path, status, created_at)
                 VALUES ('s1', 'main', '[]', ?1, ?2, ?3, 'exited', 1)",
                (
                    session.to_str().unwrap(),
                    session.join("log").to_str().unwrap(),
                    session.join("sock").to_str().unwrap(),
                ),
            )
            .await
            .unwrap();
        }

        migrate_data_dir(&legacy_dir, &data_dir).await.unwrap();

        let new_repo_dir = data_dir.join("repos/github.com/penberg/weave");
        let new_workspace = new_repo_dir.join("workspaces/main");
        let new_session = new_repo_dir.join("sessions/s1");
        assert!(new_workspace.is_dir());
        assert_eq!(fs::read_link(&legacy_dir).unwrap(), data_dir);

        let conn = open_repo_db(&new_repo_dir.join("repo.db")).await.unwrap();
        let mut stmt = conn.prepare("SELECT path FROM workspaces").await.unwrap();
        let mut rows = stmt.query(()).await.unwrap();
        let row = rows.next().await.unwrap().unwrap();
        assert_eq!(
            row.get::<String>(0).unwrap(),
            new_workspace.to_str().unwrap()
        );

        let mut stmt = conn
            .prepare("SELECT path, log_path, socket_path FROM sessions")
            .await
            .unwrap();
        let mut rows = stmt.query(()).await.unwrap();
        let row = rows.next().await.unwrap().unwrap();
        assert_eq!(row.get::<String>(0).unwrap(), new_session.to_str().unwrap());
        assert_eq!(
            row.get::<String>(1).unwrap(),
            new_session.join("log").to_str().unwrap()
        );
        assert_eq!(
            row.get::<String>(2).unwrap(),
            new_session.join("sock").to_str().unwrap()
        );

        // The worktree's .git file must point at the moved bare clone.
        let gitfile = fs::read_to_string(new_workspace.join(".git")).unwrap();
        assert!(
            gitfile.contains(data_dir.to_str().unwrap()),
            "unexpected .git file: {gitfile}"
        );
        let status = Command::new("git")
            .arg("-C")
            .arg(&new_workspace)
            .arg("status")
            .output()
            .unwrap();
        assert!(status.status.success());

        // Running it again is a no-op.
        migrate_data_dir(&legacy_dir, &data_dir).await.unwrap();

        fs::remove_dir_all(&root).unwrap();
    }

    #[tokio::test]
    async fn skips_when_nothing_to_migrate() {
        let root = unique_temp_path("splay-legacy-noop");
        let legacy_dir = root.join("swarm");
        let data_dir = root.join("splay");

        migrate_data_dir(&legacy_dir, &data_dir).await.unwrap();
        assert!(!data_dir.exists());
    }
}
