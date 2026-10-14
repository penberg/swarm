//! What the UI asks of `splay server`, plus the few things it reads from the
//! local machine directly.

use splay::{
    SplayError,
    api::{Request, WorkspaceWithSessions},
    client,
    repos::Repository,
    sandbox::SandboxKind,
    sessions::Session,
    templates::TemplateStatus,
    workspaces::{Workspace, workspace_git_dir},
};
pub use splay::{
    api::{
        Provisioning, RepositoryDetails, RepositorySettings, SessionEntry, WorkspaceEntry,
        WorkspaceGroup,
    },
    service::format_age,
};
use std::{
    fs, io,
    path::{Path, PathBuf},
    process::Command,
};

pub async fn load_workspace_groups() -> Result<Vec<WorkspaceGroup>, SplayError> {
    client::call(Request::Sidebar).await
}

pub async fn create_workspace(
    repository: String,
    name: Option<String>,
    sandbox: SandboxKind,
    progress: impl Fn(&str) + Send + Sync + 'static,
) -> Result<WorkspaceEntry, SplayError> {
    let created = client::call_with_progress(
        Request::WorkspaceCreate {
            repository,
            name,
            sandbox,
        },
        &progress,
    )
    .await?;
    Ok(workspace_entry(created))
}

pub async fn load_repository_settings(
    repository: String,
) -> Result<RepositorySettings, SplayError> {
    client::call(Request::RepoSettings { repository }).await
}

pub async fn save_setup_script(repository: String, script: String) -> Result<(), SplayError> {
    client::call(Request::RepoSetSetupScript { repository, script }).await
}

/// Enables or disables a remote sandbox and returns its template's status.
pub async fn set_sandbox_enabled(
    repository: String,
    sandbox: SandboxKind,
    enabled: bool,
) -> Result<TemplateStatus, SplayError> {
    client::call(Request::RepoSetSandboxEnabled {
        repository,
        sandbox,
        enabled,
    })
    .await
}

pub async fn rebuild_template(
    repository: String,
    sandbox: SandboxKind,
) -> Result<TemplateStatus, SplayError> {
    client::call(Request::TemplateStatus {
        repository,
        sandbox,
        rebuild: true,
    })
    .await
}

/// Returns a template's status with the end of its latest build's output.
pub async fn load_template(
    repository: String,
    sandbox: SandboxKind,
) -> Result<(TemplateStatus, String), SplayError> {
    client::call(Request::TemplateLog {
        repository,
        sandbox,
    })
    .await
}

/// Returns the template statuses of the given repositories' remote sandboxes.
pub async fn load_template_statuses(
    repositories: Vec<(String, Vec<SandboxKind>)>,
) -> Result<Vec<(String, Vec<(SandboxKind, TemplateStatus)>)>, SplayError> {
    client::call(Request::TemplateStatuses { repositories }).await
}

pub async fn add_repository(
    repository: String,
    alias: Option<String>,
) -> Result<Repository, SplayError> {
    client::call(Request::RepoAdd { repository, alias }).await
}

pub async fn sync_repository(repository: String) -> Result<(), SplayError> {
    let _: Repository = client::call(Request::RepoSync { repository }).await?;
    Ok(())
}

pub async fn archive_repository(repository: String) -> Result<(), SplayError> {
    let _: Repository = client::call(Request::RepoArchive { repository }).await?;
    Ok(())
}

pub async fn set_repository_collapsed(
    repository: String,
    collapsed: bool,
) -> Result<(), SplayError> {
    let _: Repository = client::call(Request::RepoSetCollapsed {
        repository,
        collapsed,
    })
    .await?;
    Ok(())
}

pub async fn set_all_repositories_collapsed(collapsed: bool) -> Result<(), SplayError> {
    client::call(Request::RepoSetAllCollapsed { collapsed }).await
}

pub async fn mark_workspace_used(repository: String, workspace: String) -> Result<(), SplayError> {
    client::call(Request::WorkspaceMarkUsed {
        repository,
        workspace,
    })
    .await
}

pub async fn rename_workspace(
    workspace_ref: String,
    name: String,
) -> Result<WorkspaceEntry, SplayError> {
    let renamed = client::call(Request::WorkspaceRename {
        workspace: workspace_ref,
        name,
    })
    .await?;
    Ok(workspace_entry(renamed))
}

pub async fn clone_workspace(
    workspace_ref: String,
    name: String,
) -> Result<WorkspaceEntry, SplayError> {
    let cloned = client::call(Request::WorkspaceClone {
        workspace: workspace_ref,
        name,
    })
    .await?;
    Ok(workspace_entry(cloned))
}

pub async fn remove_workspace(workspace_ref: String) -> Result<WorkspaceEntry, SplayError> {
    let workspace: Workspace = client::call(Request::WorkspaceRemove {
        workspace: workspace_ref,
        stop_sessions: true,
    })
    .await?;
    Ok(WorkspaceEntry::new(workspace, &[]))
}

pub async fn create_session(workspace_ref: String) -> Result<SessionEntry, SplayError> {
    let session: Session = client::call(Request::SessionCreate {
        workspace: workspace_ref,
        command: None,
    })
    .await?;
    Ok(SessionEntry::new(&session))
}

pub async fn close_session(session_id: String) -> Result<SessionEntry, SplayError> {
    let session: Session = client::call(Request::SessionClose {
        session: session_id,
    })
    .await?;
    Ok(SessionEntry::new(&session))
}

fn workspace_entry(created: WorkspaceWithSessions) -> WorkspaceEntry {
    if let Some(error) = &created.session_error {
        eprintln!(
            "failed to start a session in {}:{}: {error}",
            created.workspace.repository, created.workspace.name
        );
    }
    WorkspaceEntry::new(created.workspace, &created.sessions)
}

pub fn current_workspace_branch(path: &str) -> Result<String, SplayError> {
    let path = Path::new(path);
    let branch = run_git(path, ["branch", "--show-current"])?;
    if !branch.is_empty() {
        return Ok(branch);
    }

    run_git(path, ["rev-parse", "--short", "HEAD"])
}

/// Resolves the workspace's HEAD commit by reading git's files directly, so
/// callers on the GTK main thread never pay for a subprocess. Handles detached
/// HEAD, loose refs, packed refs, and linked worktrees (`commondir`).
pub fn read_workspace_head(path: &str) -> Result<String, SplayError> {
    let head_path = workspace_head_path(path)?;
    let git_dir = head_path
        .parent()
        .ok_or_else(|| SplayError::Git(format!("no git dir for {path}")))?;
    let head = fs::read_to_string(&head_path)?;
    let head = head.trim();
    let Some(ref_name) = head.strip_prefix("ref: ") else {
        return Ok(head.to_string());
    };

    // Linked worktrees keep HEAD in their private git dir while refs live in
    // the main repository's dir, pointed to by the `commondir` file.
    let common_dir = match fs::read_to_string(git_dir.join("commondir")) {
        Ok(dir) => {
            let dir = dir.trim();
            if Path::new(dir).is_absolute() {
                PathBuf::from(dir)
            } else {
                git_dir.join(dir)
            }
        }
        Err(_) => git_dir.to_path_buf(),
    };

    if let Ok(sha) = fs::read_to_string(common_dir.join(ref_name)) {
        return Ok(sha.trim().to_string());
    }

    let packed = fs::read_to_string(common_dir.join("packed-refs"))?;
    packed_ref_target(&packed, ref_name)
        .ok_or_else(|| SplayError::Git(format!("unresolved ref {ref_name} for {path}")))
}

fn packed_ref_target(packed_refs: &str, ref_name: &str) -> Option<String> {
    for line in packed_refs.lines() {
        if line.starts_with('#') || line.starts_with('^') {
            continue;
        }
        if let Some((sha, name)) = line.split_once(' ')
            && name == ref_name
        {
            return Some(sha.to_string());
        }
    }
    None
}

pub fn workspace_head_path(path: &str) -> Result<PathBuf, SplayError> {
    Ok(workspace_git_dir(Path::new(path))?.join("HEAD"))
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

fn run_git<I, S>(cwd: &Path, args: I) -> Result<String, SplayError>
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

#[cfg(test)]
mod tests {
    use super::packed_ref_target;

    #[test]
    fn packed_ref_target_resolves_branches() {
        let packed = "# pack-refs with: peeled fully-peeled sorted \n\
            0123456789abcdef0123456789abcdef01234567 refs/heads/main\n\
            89abcdef0123456789abcdef0123456789abcdef refs/heads/feature\n\
            ^7777777777abcdef0123456789abcdef01234567\n";
        assert_eq!(
            packed_ref_target(packed, "refs/heads/feature").as_deref(),
            Some("89abcdef0123456789abcdef0123456789abcdef")
        );
        assert_eq!(
            packed_ref_target(packed, "refs/heads/main").as_deref(),
            Some("0123456789abcdef0123456789abcdef01234567")
        );
        assert_eq!(packed_ref_target(packed, "refs/heads/missing"), None);
    }
}
