use std::{
    fs,
    process::{Command, Output, Stdio},
    thread,
    time::Duration,
};

use crate::{
    SplayError,
    repos::Repository,
    sandbox::{Checkout, CreateRequest, RepoPaths, Sandbox},
    sessions::default_session_command,
    workspaces::Workspace,
};

const CONTROL_HOST: &str = "exe.dev";
const VM_DOMAIN: &str = "exe.xyz";
const SSH_CONNECT_ATTEMPTS: u32 = 5;
const SSH_CONNECT_RETRY_DELAY: Duration = Duration::from_secs(2);
/// Exit status `ssh` reports for its own failures, as opposed to the remote command's.
const SSH_ERROR_STATUS: i32 = 255;

/// One exe.dev VM per workspace, managed over `ssh exe.dev`.
///
/// The repository is cloned into `~/<repo name>` on the VM. The workspace's
/// local directory stays empty and serves as the session supervisor's
/// working directory.
pub struct ExeDev;

impl Sandbox for ExeDev {
    fn create(&self, request: &CreateRequest) -> Result<Checkout, SplayError> {
        let vm = vm_name(request.repo, request.workspace_name);
        fs::create_dir_all(request.workspace_path)?;

        (request.progress)("Creating VM");
        if let Err(err) = control(["new".to_string(), format!("--name={vm}")]) {
            let _ = fs::remove_dir_all(request.workspace_path);
            return Err(err);
        }

        (request.progress)("Cloning repository");
        let dir = checkout_dir(request.repo);
        let script = format!(
            "GIT_SSH_COMMAND='ssh -o StrictHostKeyChecking=accept-new' git clone {url} {dir} && git -C {dir} switch -c {branch} origin/{default_branch}",
            url = shell_quote(&request.repo.remote_url()),
            dir = shell_quote(&dir),
            branch = shell_quote(request.workspace_name),
            default_branch = shell_quote(request.default_branch),
        );
        if let Err(err) = vm_exec(&vm, &script) {
            let _ = control(["rm".to_string(), vm.clone()]);
            let _ = fs::remove_dir_all(request.workspace_path);
            return Err(err);
        }

        Ok(Checkout {
            branch: request.workspace_name.to_string(),
            sandbox_ref: Some(vm),
        })
    }

    fn destroy(&self, workspace: &Workspace, _paths: &RepoPaths) -> Result<(), SplayError> {
        control(["rm".to_string(), workspace_vm(workspace)?.to_string()])?;
        if workspace.path.exists() {
            fs::remove_dir_all(&workspace.path)?;
        }
        Ok(())
    }

    fn session_command(
        &self,
        repo: &Repository,
        workspace: &Workspace,
        command: &[String],
    ) -> Result<Vec<String>, SplayError> {
        let vm = workspace_vm(workspace)?;
        // The local default is the local user's shell, which need not exist
        // on the VM, so run the VM user's login shell instead.
        let remote_command = if command == default_session_command() {
            "exec \"$SHELL\" -l".to_string()
        } else {
            format!(
                "exec {}",
                command
                    .iter()
                    .map(|arg| shell_quote(arg))
                    .collect::<Vec<_>>()
                    .join(" ")
            )
        };

        Ok(vec![
            "ssh".to_string(),
            "-t".to_string(),
            "-A".to_string(),
            "-o".to_string(),
            "StrictHostKeyChecking=accept-new".to_string(),
            vm_host(vm),
            format!(
                "cd {} && {remote_command}",
                shell_quote(&checkout_dir(repo))
            ),
        ])
    }
}

fn workspace_vm(workspace: &Workspace) -> Result<&str, SplayError> {
    workspace.sandbox_ref.as_deref().ok_or_else(|| {
        SplayError::Sandbox(format!("workspace `{}` has no exe.dev VM", workspace.name))
    })
}

fn vm_name(repo: &Repository, workspace: &str) -> String {
    let raw = format!("{}-{workspace}", repo.name).to_lowercase();
    let mut name = String::with_capacity(raw.len());
    for ch in raw.chars() {
        let ch = if ch.is_ascii_alphanumeric() { ch } else { '-' };
        if ch == '-' && name.ends_with('-') {
            continue;
        }
        name.push(ch);
    }
    name.trim_matches('-').to_string()
}

fn vm_host(vm: &str) -> String {
    format!("{vm}.{VM_DOMAIN}")
}

/// Checkout directory on the VM, relative to the VM user's home directory.
fn checkout_dir(repo: &Repository) -> String {
    repo.name.clone()
}

/// Runs an exe.dev control command, e.g. `ssh exe.dev new --name=<vm>`.
fn control<I>(args: I) -> Result<String, SplayError>
where
    I: IntoIterator<Item = String>,
{
    let output = Command::new("ssh")
        .args(["-o", "BatchMode=yes", CONTROL_HOST])
        .args(args)
        .stdin(Stdio::null())
        .output()?;
    check_output(output)
}

/// Runs a shell script on the VM, retrying while the VM's SSH server comes up.
fn vm_exec(vm: &str, script: &str) -> Result<String, SplayError> {
    let host = vm_host(vm);
    let mut attempt = 1;
    loop {
        let output = Command::new("ssh")
            .args([
                "-A",
                "-o",
                "BatchMode=yes",
                "-o",
                "StrictHostKeyChecking=accept-new",
                &host,
                script,
            ])
            .stdin(Stdio::null())
            .output()?;

        if output.status.code() == Some(SSH_ERROR_STATUS) && attempt < SSH_CONNECT_ATTEMPTS {
            attempt += 1;
            thread::sleep(SSH_CONNECT_RETRY_DELAY);
            continue;
        }

        return check_output(output);
    }
}

fn check_output(output: Output) -> Result<String, SplayError> {
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if output.status.success() {
        return Ok(stdout);
    }

    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    Err(SplayError::Sandbox(if !stderr.is_empty() {
        stderr
    } else if !stdout.is_empty() {
        stdout
    } else {
        format!("exit status {}", output.status)
    }))
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', r"'\''"))
}

#[cfg(test)]
mod tests {
    use super::{shell_quote, vm_name};
    use crate::repos::Repository;

    #[test]
    fn vm_name_is_dns_safe() {
        let repo = Repository::parse("github.com/penberg/Splay_App", None).unwrap();
        assert_eq!(vm_name(&repo, "fix--bug.2"), "splay-app-fix-bug-2");
    }

    #[test]
    fn shell_quote_escapes_single_quotes() {
        assert_eq!(shell_quote("it's"), r"'it'\''s'");
    }
}
