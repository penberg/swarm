//! What `splay server` does for each [`Request`], and what Splay's clients
//! show, gathered from the stores.

use serde::Serialize;
use std::{
    env, fs,
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use crate::{
    SplayError,
    api::{
        PruneFailureSummary, PruneSummary, RepositoryDetails, RepositorySettings, Request,
        ServerStatus, SessionEntry, WorkspaceEntry, WorkspaceGroup, WorkspaceWithSessions,
        build_id,
    },
    prune::PruneStore,
    repos::{Repository, RepositoryStore, data_dir},
    sandbox::Progress,
    sessions::{SessionStore, default_session_command},
    templates,
    workspaces::{Workspace, WorkspaceStore, workspace_git_dir},
};

/// How long a repository stays in the main sidebar list after its last use.
const RECENTLY_USED_WINDOW: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// Handles one request and returns its reply value. `Shutdown` is handled by
/// the server itself.
pub async fn handle(
    request: Request,
    progress: Progress<'_>,
) -> Result<serde_json::Value, SplayError> {
    match request {
        Request::Status => reply(ServerStatus {
            pid: std::process::id(),
            build: build_id().to_string(),
            data_dir: data_dir()?,
        }),
        Request::Shutdown => reply(()),

        Request::RepoAdd { repository, alias } => {
            reply(repos().await?.add(&repository, alias.as_deref()).await?)
        }
        Request::RepoSync { repository } => reply(repos().await?.sync(&repository).await?),
        Request::RepoRemove { repository } => reply(repos().await?.remove(&repository).await?),
        Request::RepoArchive { repository } => reply(repos().await?.archive(&repository).await?),
        Request::RepoUnarchive { repository } => {
            reply(repos().await?.unarchive(&repository).await?)
        }
        Request::RepoList => reply(repos().await?.list().await?),
        Request::RepoSetCollapsed {
            repository,
            collapsed,
        } => {
            let store = repos().await?;
            if collapsed {
                reply(store.collapse(&repository).await?)
            } else {
                reply(store.expand(&repository).await?)
            }
        }
        Request::RepoSetAllCollapsed { collapsed } => {
            reply(repos().await?.set_all_collapsed(collapsed).await?)
        }
        Request::RepoSettings { repository } => reply(repository_settings(&repository).await?),
        Request::RepoSetSetupScript { repository, script } => {
            let store = repos().await?;
            reply(templates::set_setup_script(&store, &repository, &script).await?)
        }
        Request::RepoSetSandboxEnabled {
            repository,
            sandbox,
            enabled,
        } => {
            let store = repos().await?;
            templates::set_sandbox_enabled(&store, &repository, sandbox, enabled).await?;
            let repo = resolve_repository(&store, &repository).await?;
            reply(templates::status(&store, &repo, sandbox))
        }
        Request::TemplateStatus {
            repository,
            sandbox,
            rebuild,
        } => {
            let store = repos().await?;
            let repo = resolve_repository(&store, &repository).await?;
            if rebuild {
                templates::rebuild(&store, &repo, sandbox).await?;
            }
            reply(templates::status(&store, &repo, sandbox))
        }
        Request::TemplateLog {
            repository,
            sandbox,
        } => {
            let store = repos().await?;
            let repo = resolve_repository(&store, &repository).await?;
            reply((
                templates::status(&store, &repo, sandbox),
                templates::build_log(&store, &repo, sandbox),
            ))
        }
        Request::TemplateStatuses { repositories } => {
            let store = repos().await?;
            let mut statuses = Vec::with_capacity(repositories.len());
            for (repository, sandboxes) in repositories {
                let repo = resolve_repository(&store, &repository).await?;
                let templates = sandboxes
                    .into_iter()
                    .map(|sandbox| (sandbox, templates::status(&store, &repo, sandbox)))
                    .collect::<Vec<_>>();
                statuses.push((repository, templates));
            }
            reply(statuses)
        }

        Request::WorkspaceCreate {
            repository,
            name,
            sandbox,
        } => {
            let workspace = WorkspaceStore::open()
                .await?
                .create(&repository, name.as_deref(), sandbox, progress)
                .await?;
            progress("Starting session");
            let sessions = SessionStore::open().await?;
            let session_error = sessions
                .create_initial(&workspace)
                .await
                .err()
                .map(|err| err.to_string());
            reply(with_sessions(&sessions, workspace, session_error).await?)
        }
        Request::WorkspaceClone { workspace, name } => {
            let cloned = WorkspaceStore::open()
                .await?
                .clone(&workspace, &name)
                .await?;
            let sessions = SessionStore::open().await?;
            let session_error = sessions
                .create(&workspace_ref(&cloned), &default_session_command())
                .await
                .err()
                .map(|err| err.to_string());
            reply(with_sessions(&sessions, cloned, session_error).await?)
        }
        Request::WorkspaceList { repository } => {
            reply(WorkspaceStore::open().await?.list(&repository).await?)
        }
        Request::WorkspaceInfo { workspace } => {
            reply(WorkspaceStore::open().await?.info(&workspace).await?)
        }
        Request::WorkspaceRename { workspace, name } => {
            let renamed = WorkspaceStore::open()
                .await?
                .rename(&workspace, &name)
                .await?;
            let sessions = SessionStore::open().await?;
            reply(with_sessions(&sessions, renamed, None).await?)
        }
        Request::WorkspaceRemove {
            workspace,
            stop_sessions,
        } => {
            if stop_sessions {
                let sessions = SessionStore::open().await?;
                for session in sessions.list(Some(&workspace)).await? {
                    sessions.stop(&session.id).await?;
                    sessions.remove(&session.id).await?;
                }
            }
            reply(WorkspaceStore::open().await?.remove(&workspace).await?)
        }
        Request::WorkspaceMarkUsed {
            repository,
            workspace,
        } => reply(
            repos()
                .await?
                .mark_workspace_used(&repository, &workspace)
                .await?,
        ),

        Request::SessionCreate { workspace, command } => {
            let command = command.unwrap_or_else(default_session_command);
            reply(
                SessionStore::open()
                    .await?
                    .create(&workspace, &command)
                    .await?,
            )
        }
        Request::SessionList { workspace } => reply(
            SessionStore::open()
                .await?
                .list(workspace.as_deref())
                .await?,
        ),
        Request::SessionInfo { session } => {
            reply(SessionStore::open().await?.info(&session).await?)
        }
        Request::SessionStop { session } => {
            reply(SessionStore::open().await?.stop(&session).await?)
        }
        Request::SessionRemove { session } => {
            reply(SessionStore::open().await?.remove(&session).await?)
        }
        Request::SessionClose { session } => {
            let sessions = SessionStore::open().await?;
            sessions.stop(&session).await?;
            reply(sessions.remove(&session).await?)
        }

        Request::PruneSessions => reply(PruneStore::open().await?.sessions().await?),
        Request::PruneWorkspaces => {
            let report = PruneStore::open().await?.workspaces().await?;
            reply(PruneSummary {
                pruned: report.pruned,
                failed: report
                    .failed
                    .into_iter()
                    .map(|failure| PruneFailureSummary {
                        name: failure.name,
                        path: failure.path,
                        error: failure.error.to_string(),
                    })
                    .collect(),
            })
        }

        Request::Sidebar => reply(sidebar().await?),
    }
}

fn reply<T: Serialize>(value: T) -> Result<serde_json::Value, SplayError> {
    Ok(serde_json::to_value(value)?)
}

async fn repos() -> Result<RepositoryStore, SplayError> {
    RepositoryStore::open().await
}

async fn resolve_repository(
    store: &RepositoryStore,
    repository: &str,
) -> Result<Repository, SplayError> {
    store
        .resolve_repository(repository)
        .await?
        .ok_or_else(|| SplayError::RepositoryNotFound(repository.to_string()))
}

fn workspace_ref(workspace: &Workspace) -> String {
    format!("{}:{}", workspace.repository, workspace.name)
}

async fn with_sessions(
    sessions: &SessionStore,
    workspace: Workspace,
    session_error: Option<String>,
) -> Result<WorkspaceWithSessions, SplayError> {
    let sessions = sessions.list(Some(&workspace_ref(&workspace))).await?;
    Ok(WorkspaceWithSessions {
        workspace,
        sessions,
        session_error,
    })
}

/// Loads what the repository settings page shows.
pub async fn repository_settings(repository: &str) -> Result<RepositorySettings, SplayError> {
    let store = repos().await?;
    let repo = resolve_repository(&store, repository).await?;
    let mirror_path = store.bare_repo_path(&repo).display().to_string();
    let mirror_path = match env::var("HOME") {
        Ok(home) if !home.is_empty() && mirror_path.starts_with(&home) => {
            format!("~{}", &mirror_path[home.len()..])
        }
        _ => mirror_path,
    };
    let setup_script = store.setup_script(repository).await?.unwrap_or_default();

    Ok(RepositorySettings {
        details: RepositoryDetails {
            remote_url: repo.remote_url(),
            default_branch: store.default_branch(&repo),
            mirror_path,
            alias: repo.alias,
        },
        setup_script,
    })
}

/// Loads the sidebar: every repository that is not archived, with its
/// workspaces and their sessions.
pub async fn sidebar() -> Result<Vec<WorkspaceGroup>, SplayError> {
    let repo_store = repos().await?;
    let workspace_store = WorkspaceStore::open().await?;
    let session_store = SessionStore::open().await?;
    let repos = repo_store
        .list()
        .await?
        .into_iter()
        .filter(|repo| repo.archived_at.is_none())
        .collect::<Vec<_>>();
    let mut groups = Vec::with_capacity(repos.len());
    let recently_used_since = unix_now() - RECENTLY_USED_WINDOW.as_secs() as i64;

    for repo in repos {
        let repo_label = repo.alias.clone().unwrap_or_else(|| repo.name.clone());
        let repo_canonical = repo.canonical();
        let repo_status = repo_sync_status(&repo_store, &repo);
        let workspace_last_used = repo_store.workspace_last_used(&repo).await?;
        let remote_sandboxes = repo_store.enabled_sandboxes(&repo).await?;
        let templates = remote_sandboxes
            .iter()
            .map(|sandbox| (*sandbox, templates::status(&repo_store, &repo, *sandbox)))
            .collect();
        let mut workspace_entries = Vec::new();
        for workspace in workspace_store.list(&repo_canonical).await? {
            let sessions = session_store.list(Some(&workspace_ref(&workspace))).await?;
            let has_running_session = sessions
                .iter()
                .any(|session| matches!(session.status.as_str(), "running" | "starting"));
            let last_activity = sessions
                .iter()
                .flat_map(|session| [Some(session.created_at), session.stopped_at])
                .chain([
                    Some(workspace.created_at),
                    workspace_last_used.get(&workspace.name).copied(),
                    workspace_last_commit(&workspace.path),
                ])
                .flatten()
                .max();
            let recently_used = has_running_session
                || last_activity.is_some_and(|last_activity| last_activity >= recently_used_since);
            workspace_entries.push(WorkspaceEntry {
                repo_canonical: repo_canonical.clone(),
                name: workspace.name,
                branch: workspace.branch,
                path: workspace.path.display().to_string(),
                sandbox: workspace.sandbox,
                sessions: sessions.iter().map(SessionEntry::new).collect(),
                recently_used,
                provisioning: None,
            });
        }
        let recently_used = repo
            .last_used_at
            .is_some_and(|last_used_at| last_used_at >= recently_used_since)
            || workspace_entries
                .iter()
                .any(|workspace| workspace.recently_used);
        let workspace_count = workspace_entries.len();

        groups.push(WorkspaceGroup {
            repo_label,
            repo_canonical,
            repo_status,
            collapsed: repo.collapsed,
            recently_used,
            workspace_count,
            workspaces: workspace_entries,
            remote_sandboxes,
            templates,
        });
    }

    groups.sort_by(|left, right| {
        right
            .recently_used
            .cmp(&left.recently_used)
            .then_with(|| {
                left.repo_label
                    .to_lowercase()
                    .cmp(&right.repo_label.to_lowercase())
            })
            .then_with(|| left.repo_canonical.cmp(&right.repo_canonical))
    });

    Ok(groups)
}

/// Returns when HEAD last moved in a workspace (commit, checkout, reset), from
/// the modification time of its HEAD reflog.
fn workspace_last_commit(workspace_path: &Path) -> Option<i64> {
    let reflog = workspace_git_dir(workspace_path)
        .ok()?
        .join("logs")
        .join("HEAD");
    let modified = file_modified_time(reflog)?;
    let elapsed = modified.duration_since(UNIX_EPOCH).ok()?;
    Some(elapsed.as_secs() as i64)
}

fn repo_sync_status(repo_store: &RepositoryStore, repo: &Repository) -> Option<String> {
    let bare_repo_path = repo_store.bare_repo_path(repo);
    let timestamp = [
        bare_repo_path.join("FETCH_HEAD"),
        bare_repo_path.join("HEAD"),
        bare_repo_path
            .join("refs")
            .join("remotes")
            .join("origin")
            .join("HEAD"),
    ]
    .into_iter()
    .find_map(file_modified_time)?;

    Some(format_relative_time(timestamp))
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or_default()
}

fn file_modified_time(path: PathBuf) -> Option<SystemTime> {
    fs::metadata(path).ok()?.modified().ok()
}

/// Formats a Unix timestamp relative to now, like "5 minutes ago".
pub fn format_age(timestamp: i64) -> String {
    format_relative_time(UNIX_EPOCH + Duration::from_secs(timestamp.max(0) as u64))
}

fn format_relative_time(timestamp: SystemTime) -> String {
    let Ok(elapsed) = SystemTime::now().duration_since(timestamp) else {
        return "just now".to_string();
    };

    format_elapsed(elapsed)
}

fn format_elapsed(elapsed: Duration) -> String {
    let seconds = elapsed.as_secs();
    if seconds < 60 {
        return "just now".to_string();
    }

    let minutes = seconds / 60;
    if minutes < 60 {
        return pluralize(minutes, "minute");
    }

    let hours = minutes / 60;
    if hours < 24 {
        return pluralize(hours, "hour");
    }

    let days = hours / 24;
    if days < 7 {
        return pluralize(days, "day");
    }

    let weeks = days / 7;
    if weeks < 5 {
        return pluralize(weeks, "week");
    }

    let months = days / 30;
    if months < 12 {
        return pluralize(months.max(1), "month");
    }

    pluralize((days / 365).max(1), "year")
}

fn pluralize(value: u64, unit: &str) -> String {
    if value == 1 {
        format!("1 {unit} ago")
    } else {
        format!("{value} {unit}s ago")
    }
}
