use std::{
    fs, io,
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, SystemTime},
};
use swarm::{
    SwarmError,
    repos::{Repository, RepositoryStore},
    sessions::{SessionStore, default_session_command},
    workspaces::{Workspace, WorkspaceStore},
};

#[derive(Debug, Clone)]
pub struct WorkspaceGroup {
    pub repo_label: String,
    pub repo_canonical: String,
    pub repo_status: Option<String>,
    pub collapsed: bool,
    pub workspace_count: usize,
    pub workspaces: Vec<WorkspaceEntry>,
}

#[derive(Debug, Clone)]
pub struct WorkspaceEntry {
    pub repo_canonical: String,
    pub name: String,
    pub branch: String,
    pub path: String,
    pub sessions: Vec<SessionEntry>,
}

#[derive(Debug, Clone)]
pub struct SessionEntry {
    pub id: String,
    pub pid: Option<u32>,
    pub program: String,
    pub status: String,
    pub log_path: String,
    pub socket_path: String,
}

pub fn load_workspace_groups() -> Result<Vec<WorkspaceGroup>, SwarmError> {
    let runtime = tokio::runtime::Runtime::new()?;
    runtime.block_on(async {
        let repo_store = RepositoryStore::open().await?;
        let workspace_store = WorkspaceStore::open().await?;
        let session_store = SessionStore::open().await?;
        let repos = repo_store.list().await?;
        let mut groups = Vec::with_capacity(repos.len());

        for repo in repos {
            let repo_label = repo.alias.clone().unwrap_or_else(|| repo.name.clone());
            let repo_canonical = repo.canonical();
            let repo_status = repo_sync_status(&repo_store, &repo);
            let workspaces = workspace_store.list(&repo_canonical).await?;
            let workspaces = workspaces.into_iter().map(|workspace| async {
                let workspace_ref = format!("{}:{}", workspace.repository, workspace.name);
                let sessions = session_store
                    .list(Some(&workspace_ref))
                    .await?
                    .into_iter()
                    .map(|session| SessionEntry {
                        pid: session.pid,
                        program: session_program(session.pid, &session.command),
                        id: session.id,
                        status: session.status,
                        log_path: session.log_path.display().to_string(),
                        socket_path: session.socket_path.display().to_string(),
                    })
                    .collect::<Vec<_>>();

                Ok::<WorkspaceEntry, SwarmError>(map_workspace(
                    &repo_canonical,
                    workspace,
                    sessions,
                ))
            });
            let mut workspace_entries = Vec::new();
            for workspace in workspaces {
                workspace_entries.push(workspace.await?);
            }
            let workspace_count = workspace_entries.len();

            groups.push(WorkspaceGroup {
                repo_label,
                repo_canonical,
                repo_status,
                collapsed: repo.collapsed,
                workspace_count,
                workspaces: workspace_entries,
            });
        }

        Ok(groups)
    })
}

pub fn create_workspace(
    repository: &str,
    name: Option<&str>,
) -> Result<WorkspaceEntry, SwarmError> {
    let runtime = tokio::runtime::Runtime::new()?;
    runtime.block_on(async {
        let repo_store = RepositoryStore::open().await?;
        let workspace_store = WorkspaceStore::open().await?;
        let session_store = SessionStore::open().await?;

        let repo = repo_store
            .resolve_repository(repository)
            .await?
            .ok_or_else(|| SwarmError::RepositoryNotFound(repository.to_string()))?;
        let workspace = workspace_store.create(repository, name).await?;
        let workspace_ref = format!("{}:{}", workspace.repository, workspace.name);
        if let Err(err) = session_store
            .create(&workspace_ref, &default_session_command())
            .await
        {
            eprintln!("failed to create default session for {workspace_ref}: {err}");
        }
        let sessions = session_store
            .list(Some(&workspace_ref))
            .await?
            .into_iter()
            .map(|session| SessionEntry {
                pid: session.pid,
                program: session_program(session.pid, &session.command),
                id: session.id,
                status: session.status,
                log_path: session.log_path.display().to_string(),
                socket_path: session.socket_path.display().to_string(),
            })
            .collect::<Vec<_>>();

        Ok(map_workspace(&repo.canonical(), workspace, sessions))
    })
}

pub fn add_repository(repository: &str, alias: Option<&str>) -> Result<Repository, SwarmError> {
    let runtime = tokio::runtime::Runtime::new()?;
    runtime.block_on(async {
        let repo_store = RepositoryStore::open().await?;
        repo_store.add(repository, alias).await
    })
}

pub fn sync_repository(repository: &str) -> Result<(), SwarmError> {
    let runtime = tokio::runtime::Runtime::new()?;
    runtime.block_on(async {
        let repo_store = RepositoryStore::open().await?;
        repo_store.sync(repository).await?;
        Ok(())
    })
}

pub fn collapse_repository(repository: &str) -> Result<(), SwarmError> {
    let runtime = tokio::runtime::Runtime::new()?;
    runtime.block_on(async {
        let repo_store = RepositoryStore::open().await?;
        repo_store.collapse(repository).await?;
        Ok(())
    })
}

pub fn expand_repository(repository: &str) -> Result<(), SwarmError> {
    let runtime = tokio::runtime::Runtime::new()?;
    runtime.block_on(async {
        let repo_store = RepositoryStore::open().await?;
        repo_store.expand(repository).await?;
        Ok(())
    })
}

pub fn rename_workspace(workspace_ref: &str, name: &str) -> Result<WorkspaceEntry, SwarmError> {
    let runtime = tokio::runtime::Runtime::new()?;
    runtime.block_on(async {
        let repo_store = RepositoryStore::open().await?;
        let workspace_store = WorkspaceStore::open().await?;
        let session_store = SessionStore::open().await?;
        let workspace = workspace_store.rename(workspace_ref, name).await?;
        let repo = repo_store
            .resolve_repository(&workspace.repository)
            .await?
            .ok_or_else(|| SwarmError::RepositoryNotFound(workspace.repository.clone()))?;
        let workspace_ref = format!("{}:{}", workspace.repository, workspace.name);
        let sessions = session_store
            .list(Some(&workspace_ref))
            .await?
            .into_iter()
            .map(|session| SessionEntry {
                pid: session.pid,
                program: session_program(session.pid, &session.command),
                id: session.id,
                status: session.status,
                log_path: session.log_path.display().to_string(),
                socket_path: session.socket_path.display().to_string(),
            })
            .collect::<Vec<_>>();

        Ok(map_workspace(&repo.canonical(), workspace, sessions))
    })
}

pub fn clone_workspace(workspace_ref: &str, name: &str) -> Result<WorkspaceEntry, SwarmError> {
    let runtime = tokio::runtime::Runtime::new()?;
    runtime.block_on(async {
        let repo_store = RepositoryStore::open().await?;
        let workspace_store = WorkspaceStore::open().await?;
        let session_store = SessionStore::open().await?;
        let workspace = workspace_store.clone(workspace_ref, name).await?;
        let repo = repo_store
            .resolve_repository(&workspace.repository)
            .await?
            .ok_or_else(|| SwarmError::RepositoryNotFound(workspace.repository.clone()))?;
        let workspace_ref = format!("{}:{}", workspace.repository, workspace.name);
        if let Err(err) = session_store
            .create(&workspace_ref, &default_session_command())
            .await
        {
            eprintln!("failed to create default session for {workspace_ref}: {err}");
        }
        let sessions = session_store
            .list(Some(&workspace_ref))
            .await?
            .into_iter()
            .map(|session| SessionEntry {
                pid: session.pid,
                program: session_program(session.pid, &session.command),
                id: session.id,
                status: session.status,
                log_path: session.log_path.display().to_string(),
                socket_path: session.socket_path.display().to_string(),
            })
            .collect::<Vec<_>>();

        Ok(map_workspace(&repo.canonical(), workspace, sessions))
    })
}

pub fn remove_workspace(workspace_ref: &str) -> Result<WorkspaceEntry, SwarmError> {
    let runtime = tokio::runtime::Runtime::new()?;
    runtime.block_on(async {
        let repo_store = RepositoryStore::open().await?;
        let session_store = SessionStore::open().await?;
        let workspace_store = WorkspaceStore::open().await?;
        let sessions = session_store.list(Some(workspace_ref)).await?;

        for session in sessions {
            session_store.stop(&session.id).await?;
            session_store.remove(&session.id).await?;
        }

        let workspace = workspace_store.remove(workspace_ref).await?;
        let repo = repo_store
            .resolve_repository(&workspace.repository)
            .await?
            .ok_or_else(|| SwarmError::RepositoryNotFound(workspace.repository.clone()))?;

        Ok(map_workspace(&repo.canonical(), workspace, Vec::new()))
    })
}

pub fn create_session(workspace_ref: &str) -> Result<SessionEntry, SwarmError> {
    let runtime = tokio::runtime::Runtime::new()?;
    runtime.block_on(async {
        let session_store = SessionStore::open().await?;
        let session = session_store
            .create(workspace_ref, &default_session_command())
            .await?;

        Ok(SessionEntry {
            id: session.id,
            pid: session.pid,
            program: session_program(session.pid, &session.command),
            status: session.status,
            log_path: session.log_path.display().to_string(),
            socket_path: session.socket_path.display().to_string(),
        })
    })
}

pub fn close_session(session_id: &str) -> Result<SessionEntry, SwarmError> {
    let runtime = tokio::runtime::Runtime::new()?;
    runtime.block_on(async {
        let session_store = SessionStore::open().await?;
        session_store.stop(session_id).await?;
        let session = session_store.remove(session_id).await?;

        Ok(SessionEntry {
            id: session.id,
            pid: session.pid,
            program: session_program(session.pid, &session.command),
            status: session.status,
            log_path: session.log_path.display().to_string(),
            socket_path: session.socket_path.display().to_string(),
        })
    })
}

pub fn current_workspace_branch(path: &str) -> Result<String, SwarmError> {
    let path = Path::new(path);
    let branch = run_git(path, ["branch", "--show-current"])?;
    if !branch.is_empty() {
        return Ok(branch);
    }

    run_git(path, ["rev-parse", "--short", "HEAD"])
}

pub fn current_workspace_head(path: &str) -> Result<String, SwarmError> {
    run_git(Path::new(path), ["rev-parse", "HEAD"])
}

pub fn workspace_head_path(path: &str) -> Result<PathBuf, SwarmError> {
    let workspace_path = Path::new(path);
    let git_path = workspace_path.join(".git");
    let git_dir = if git_path.is_dir() {
        git_path
    } else {
        resolve_gitdir_file(&git_path, workspace_path)?
    };

    Ok(git_dir.join("HEAD"))
}

fn map_workspace(
    repo_canonical: &str,
    workspace: Workspace,
    sessions: Vec<SessionEntry>,
) -> WorkspaceEntry {
    WorkspaceEntry {
        repo_canonical: repo_canonical.to_string(),
        name: workspace.name,
        branch: workspace.branch,
        path: workspace.path.display().to_string(),
        sessions,
    }
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

    Some(format_relative_sync_time(timestamp))
}

fn file_modified_time(path: PathBuf) -> Option<SystemTime> {
    fs::metadata(path).ok()?.modified().ok()
}

fn format_relative_sync_time(timestamp: SystemTime) -> String {
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

fn session_program(pid: Option<u32>, command: &[String]) -> String {
    foreground_program(pid).unwrap_or_else(|| command_program(command))
}

pub fn foreground_program(pid: Option<u32>) -> Option<String> {
    let stat = read_proc_stat(pid?).ok()?;
    if stat.tpgid <= 0 || stat.tty_nr == 0 {
        return None;
    }
    if stat.tpgid == stat.pgrp {
        return process_label(pid?);
    }

    let mut fallback_pid = None;
    let proc_dir = fs::read_dir("/proc").ok()?;
    for entry in proc_dir.flatten() {
        let name = entry.file_name();
        let Some(pid) = name.to_str().and_then(|value| value.parse::<u32>().ok()) else {
            continue;
        };
        let Some(candidate) = read_proc_stat(pid).ok() else {
            continue;
        };
        if candidate.tty_nr == stat.tty_nr && candidate.pgrp == stat.tpgid {
            if pid == candidate.pgrp as u32 {
                return process_label(pid);
            }
            if fallback_pid.is_none_or(|current| pid < current) {
                fallback_pid = Some(pid);
            }
        }
    }

    fallback_pid.and_then(process_label)
}

fn command_program(command: &[String]) -> String {
    command
        .first()
        .and_then(|arg| Path::new(arg).file_name())
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or("session")
        .to_string()
}

struct ProcStat {
    pgrp: i32,
    tty_nr: i32,
    tpgid: i32,
}

fn read_proc_stat(pid: u32) -> Result<ProcStat, io::Error> {
    let path = format!("/proc/{pid}/stat");
    let contents = fs::read_to_string(path)?;
    parse_proc_stat(&contents)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "invalid /proc stat"))
}

fn parse_proc_stat(contents: &str) -> Option<ProcStat> {
    contents.find('(')?;
    let close = contents.rfind(')')?;
    let rest = contents.get(close + 2..)?;
    let fields = rest.split_whitespace().collect::<Vec<_>>();
    Some(ProcStat {
        pgrp: fields.get(2)?.parse().ok()?,
        tty_nr: fields.get(4)?.parse().ok()?,
        tpgid: fields.get(5)?.parse().ok()?,
    })
}

fn process_label(pid: u32) -> Option<String> {
    process_cmdline_label(pid)
        .or_else(|| process_exe_label(pid))
        .or_else(|| process_comm_label(pid))
}

fn process_cmdline_label(pid: u32) -> Option<String> {
    let path = format!("/proc/{pid}/cmdline");
    let contents = fs::read(path).ok()?;
    let args = contents
        .split(|byte| *byte == 0)
        .filter(|part| !part.is_empty())
        .map(|part| String::from_utf8(part.to_vec()).ok())
        .collect::<Option<Vec<_>>>()?;
    let first = args.first()?;
    let first_name = Path::new(first).file_name()?.to_str()?;

    if is_interpreter(first_name) {
        let script = args
            .iter()
            .skip(1)
            .find(|arg| !arg.starts_with('-'))
            .and_then(|arg| Path::new(arg).file_name())
            .and_then(|name| name.to_str());
        if let Some(script) = script {
            return Some(script.to_string());
        }
    }

    Some(first_name.to_string())
}

fn process_exe_label(pid: u32) -> Option<String> {
    let path = format!("/proc/{pid}/exe");
    fs::read_link(path)
        .ok()?
        .file_name()?
        .to_str()
        .map(|name| name.to_string())
}

fn process_comm_label(pid: u32) -> Option<String> {
    let path = format!("/proc/{pid}/comm");
    fs::read_to_string(path)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn is_interpreter(name: &str) -> bool {
    matches!(
        name,
        "python" | "python3" | "bash" | "sh" | "zsh" | "node" | "ruby"
    )
}

fn resolve_gitdir_file(git_path: &Path, workspace_path: &Path) -> Result<PathBuf, SwarmError> {
    let contents = fs::read_to_string(git_path)?;
    let git_dir = contents
        .strip_prefix("gitdir: ")
        .map(str::trim)
        .filter(|path| !path.is_empty())
        .ok_or_else(|| SwarmError::Git(format!("invalid gitdir file at {}", git_path.display())))?;

    let git_dir = Path::new(git_dir);
    if git_dir.is_absolute() {
        return Ok(git_dir.to_path_buf());
    }

    Ok(workspace_path.join(git_dir))
}

fn run_git<I, S>(cwd: &Path, args: I) -> Result<String, SwarmError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut cmd = Command::new("git");
    cmd.current_dir(cwd);

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
