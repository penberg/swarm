//! Templates that new remote workspaces are copied from.
//!
//! A remote sandbox keeps one template per repository: a checkout with the
//! repository's setup script already run. Templates are built by a detached
//! `splayctl repo build-template` process, so that a build survives the
//! process that started it. Its state lives in a JSON file next to the build
//! log, rather than in the index database, because the builder updates it
//! from another process for minutes at a time.

use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read, Seek, SeekFrom, Write},
    os::unix::process::CommandExt,
    path::{Path, PathBuf},
    process::{self, Command, Stdio},
    time::{SystemTime, UNIX_EPOCH},
};

use crate::{
    SplayError,
    repos::{Repository, RepositoryStore},
    sandbox::{SandboxKind, TemplateRequest, sandbox},
    sessions::{process_exists, supervisor_executable, terminate_process},
};

/// How much of the end of the build log `build_log` returns.
const BUILD_LOG_TAIL_BYTES: u64 = 64 * 1024;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReadyTemplate {
    /// Name of the template in its sandbox, such as a VM name.
    pub name: String,
    /// Hash of the setup script the template was built with.
    pub script_hash: String,
    pub built_at: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct TemplateState {
    ready: Option<ReadyTemplate>,
    build: Option<TemplateBuild>,
}

/// The latest build: running while its process is, failed once it has an error.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct TemplateBuild {
    pid: u32,
    name: Option<String>,
    started_at: i64,
    error: Option<String>,
}

/// A repository's template in one sandbox.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TemplateStatus {
    /// The template new workspaces are copied from, if one has been built.
    pub ready: Option<ReadyTemplate>,
    /// When the build that is running now started.
    pub building_since: Option<i64>,
    /// Why the latest build failed.
    pub error: Option<String>,
}

impl TemplateStatus {
    pub fn is_building(&self) -> bool {
        self.building_since.is_some()
    }
}

/// Returns the state of the template of `repo` in `sandbox_kind`.
pub fn status(
    repos: &RepositoryStore,
    repo: &Repository,
    sandbox_kind: SandboxKind,
) -> TemplateStatus {
    let state = read_state(&template_dir(repos, repo, sandbox_kind));
    let mut status = TemplateStatus {
        ready: state.ready,
        ..TemplateStatus::default()
    };

    if let Some(build) = state.build {
        match build.error {
            Some(error) => status.error = Some(error),
            None if process_exists(build.pid).unwrap_or(false) => {
                status.building_since = Some(build.started_at);
            }
            None => status.error = Some("the template build stopped unexpectedly".to_string()),
        }
    }

    status
}

/// Returns the end of the latest build's output.
pub fn build_log(repos: &RepositoryStore, repo: &Repository, sandbox_kind: SandboxKind) -> String {
    let path = template_dir(repos, repo, sandbox_kind).join("build.log");
    let Ok(mut file) = File::open(path) else {
        return String::new();
    };
    let length = file.metadata().map(|metadata| metadata.len()).unwrap_or(0);
    let _ = file.seek(SeekFrom::Start(length.saturating_sub(BUILD_LOG_TAIL_BYTES)));
    let mut bytes = Vec::new();
    let _ = file.read_to_end(&mut bytes);
    String::from_utf8_lossy(&bytes).into_owned()
}

/// Enables or disables a remote sandbox for `repository`, building its
/// template when enabled and destroying it when disabled.
pub async fn set_sandbox_enabled(
    repos: &RepositoryStore,
    repository: &str,
    sandbox_kind: SandboxKind,
    enabled: bool,
) -> Result<(), SplayError> {
    repos
        .set_sandbox_enabled(repository, sandbox_kind, enabled)
        .await?;
    let repo = resolve(repos, repository).await?;

    if enabled {
        ensure_built(repos, &repo, sandbox_kind).await
    } else {
        remove(repos, &repo, sandbox_kind)
    }
}

/// Sets the setup script of `repository` and rebuilds the templates built
/// with a different one.
pub async fn set_setup_script(
    repos: &RepositoryStore,
    repository: &str,
    script: &str,
) -> Result<(), SplayError> {
    repos.set_setup_script(repository, script).await?;
    let repo = resolve(repos, repository).await?;

    for sandbox_kind in repos.enabled_sandboxes(&repo).await? {
        ensure_built(repos, &repo, sandbox_kind).await?;
    }
    Ok(())
}

/// Builds the template again, unless a build is already running.
pub fn rebuild(
    repos: &RepositoryStore,
    repo: &Repository,
    sandbox_kind: SandboxKind,
) -> Result<(), SplayError> {
    if status(repos, repo, sandbox_kind).is_building() {
        return Ok(());
    }
    start_build(repos, repo, sandbox_kind)
}

/// Starts a build unless the template is up to date with the setup script or
/// one is already running. A running build picks up script changes when it
/// finishes.
async fn ensure_built(
    repos: &RepositoryStore,
    repo: &Repository,
    sandbox_kind: SandboxKind,
) -> Result<(), SplayError> {
    let current = status(repos, repo, sandbox_kind);
    if current.is_building() {
        return Ok(());
    }

    let script_hash = script_hash(repos.setup_script(&repo.canonical()).await?.as_deref());
    if current
        .ready
        .is_some_and(|ready| ready.script_hash == script_hash)
    {
        return Ok(());
    }

    start_build(repos, repo, sandbox_kind)
}

fn start_build(
    repos: &RepositoryStore,
    repo: &Repository,
    sandbox_kind: SandboxKind,
) -> Result<(), SplayError> {
    let dir = template_dir(repos, repo, sandbox_kind);
    fs::create_dir_all(&dir)?;
    let log = File::create(dir.join("build.log"))?;

    let mut builder = Command::new(supervisor_executable()?);
    builder
        .args(["repo", "build-template"])
        .arg(repo.canonical())
        .arg(sandbox_kind.as_str())
        .stdin(Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log);
    // Detach the builder so that it outlives whoever started it.
    unsafe {
        builder.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let child = builder.spawn()?;

    update_state(&dir, |state| {
        state.build = Some(TemplateBuild {
            pid: child.id(),
            name: None,
            started_at: unix_timestamp(),
            error: None,
        });
    })?;
    Ok(())
}

/// Builds the template of `repository` in `sandbox_kind`. This is the body of
/// the detached builder process; its output is the build log.
pub async fn run_build(repository: &str, sandbox_kind: SandboxKind) -> Result<(), SplayError> {
    // The index database is locked for as long as a process has it open, so
    // open it only briefly rather than for the whole build.
    let (repo, dir) = {
        let repos = RepositoryStore::open().await?;
        let repo = resolve(&repos, repository).await?;
        let dir = template_dir(&repos, &repo, sandbox_kind);
        (repo, dir)
    };
    let log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("build.log"))?;
    let provider = sandbox(sandbox_kind);
    let started_at = unix_timestamp();

    loop {
        let script = read_setup_script(&repo).await?;
        let script_hash = script_hash(script.as_deref());
        let name = provider.new_template_name(&repo);
        update_state(&dir, |state| {
            state.build = Some(TemplateBuild {
                pid: process::id(),
                name: Some(name.clone()),
                started_at,
                error: None,
            });
        })?;

        let built = provider.build_template(&TemplateRequest {
            repo: &repo,
            name: &name,
            setup_script: script.as_deref(),
            log: &log,
        });
        if let Err(err) = built {
            writeln!(&log, "==> Template build failed: {err}")?;
            if let Err(cleanup) = provider.destroy_template(&name) {
                writeln!(&log, "==> Failed to delete {name}: {cleanup}")?;
            }
            update_state(&dir, |state| {
                state.build = Some(TemplateBuild {
                    pid: process::id(),
                    name: None,
                    started_at,
                    error: Some(err.to_string()),
                });
            })?;
            return Err(err);
        }

        let mut previous = None;
        update_state(&dir, |state| {
            previous = state.ready.replace(ReadyTemplate {
                name: name.clone(),
                script_hash: script_hash.clone(),
                built_at: unix_timestamp(),
            });
        })?;
        writeln!(&log, "==> Template {name} is ready")?;
        if let Some(previous) = previous
            && let Err(err) = provider.destroy_template(&previous.name)
        {
            writeln!(
                &log,
                "==> Failed to delete previous template {}: {err}",
                previous.name
            )?;
        }

        // Build again if the script changed while this build ran.
        let script = read_setup_script(&repo).await?;
        if script_hash == self::script_hash(script.as_deref()) {
            break;
        }
        writeln!(&log, "==> The setup script changed; building again")?;
    }

    update_state(&dir, |state| state.build = None)?;
    Ok(())
}

/// Stops a running build and destroys the template and any partial build.
fn remove(
    repos: &RepositoryStore,
    repo: &Repository,
    sandbox_kind: SandboxKind,
) -> Result<(), SplayError> {
    let dir = template_dir(repos, repo, sandbox_kind);
    let state = read_state(&dir);
    let provider = sandbox(sandbox_kind);
    let mut result = Ok(());

    if let Some(build) = state.build {
        if build.error.is_none() {
            terminate_process(build.pid)?;
        }
        if let Some(name) = build.name {
            result = result.and(provider.destroy_template(&name));
        }
    }
    if let Some(ready) = state.ready {
        result = result.and(provider.destroy_template(&ready.name));
    }

    if dir.exists() {
        fs::remove_dir_all(&dir)?;
    }
    result
}

async fn read_setup_script(repo: &Repository) -> Result<Option<String>, SplayError> {
    RepositoryStore::open()
        .await?
        .setup_script(&repo.canonical())
        .await
}

async fn resolve(repos: &RepositoryStore, repository: &str) -> Result<Repository, SplayError> {
    repos
        .resolve_repository(repository)
        .await?
        .ok_or_else(|| SplayError::RepositoryNotFound(repository.to_string()))
}

fn template_dir(repos: &RepositoryStore, repo: &Repository, sandbox_kind: SandboxKind) -> PathBuf {
    repos
        .repo_dir(repo)
        .join("templates")
        .join(sandbox_kind.as_str())
}

fn read_state(dir: &Path) -> TemplateState {
    fs::read(dir.join("state.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

/// Applies `update` to the state, replacing the file atomically so that
/// readers never see a partial write.
fn update_state(dir: &Path, update: impl FnOnce(&mut TemplateState)) -> Result<(), SplayError> {
    let mut state = read_state(dir);
    update(&mut state);
    fs::create_dir_all(dir)?;
    let temporary = dir.join(format!("state.json.{}", process::id()));
    fs::write(&temporary, serde_json::to_vec_pretty(&state)?)?;
    fs::rename(&temporary, dir.join("state.json"))?;
    Ok(())
}

/// FNV-1a, which unlike `std`'s hasher is stable across Rust versions.
fn script_hash(script: Option<&str>) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in script.unwrap_or_default().bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{hash:016x}")
}

fn unix_timestamp() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::PathBuf,
        time::{SystemTime, UNIX_EPOCH},
    };

    use super::{ReadyTemplate, TemplateBuild, read_state, script_hash, update_state};

    fn unique_temp_dir() -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("splay-template-test-{nanos}"))
    }

    #[test]
    fn script_hash_is_stable_and_treats_no_script_as_empty() {
        assert_eq!(script_hash(None), script_hash(Some("")));
        assert_eq!(script_hash(Some("cargo fetch")), "ef2b90f1701fa223");
        assert_ne!(
            script_hash(Some("cargo fetch")),
            script_hash(Some("cargo build"))
        );
    }

    #[test]
    fn state_round_trips_through_its_file() {
        let dir = unique_temp_dir();
        assert!(read_state(&dir).ready.is_none());

        update_state(&dir, |state| {
            state.ready = Some(ReadyTemplate {
                name: "splay-template-1".to_string(),
                script_hash: "abc".to_string(),
                built_at: 1,
            });
            state.build = Some(TemplateBuild {
                pid: 1,
                name: None,
                started_at: 2,
                error: Some("failed".to_string()),
            });
        })
        .unwrap();

        let state = read_state(&dir);
        assert_eq!(state.ready.unwrap().name, "splay-template-1");
        assert_eq!(state.build.unwrap().error.as_deref(), Some("failed"));
        fs::remove_dir_all(&dir).unwrap();
    }
}
