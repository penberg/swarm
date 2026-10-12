use std::{
    fs::{self, File},
    io::Write,
    process::{Command, Output, Stdio},
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use crate::{
    SplayError,
    repos::Repository,
    sandbox::{Checkout, CreateRequest, RepoPaths, Sandbox, TemplateRequest},
    sessions::default_session_command,
    workspaces::Workspace,
};

const CONTROL_HOST: &str = "exe.dev";
const VM_DOMAIN: &str = "exe.xyz";
/// Disk size of template VMs, which their copies inherit. Builds of large
/// projects outgrow exe.dev's default disk; usage beyond the plan's included
/// disk is billed, not the size.
const TEMPLATE_DISK: &str = "60GB";
const SSH_CONNECT_ATTEMPTS: u32 = 5;
const SSH_CONNECT_RETRY_DELAY: Duration = Duration::from_secs(2);
/// Exit status `ssh` reports for its own failures, as opposed to the remote command's.
const SSH_ERROR_STATUS: i32 = 255;

/// One exe.dev VM per workspace, managed over `ssh exe.dev`.
///
/// Each repository has a template VM with the repository cloned into
/// `~/<repo name>` and the setup script run. A workspace's VM is a copy of the
/// template, so creating one takes seconds. The workspace's local directory
/// stays empty and serves as the session supervisor's working directory.
pub struct ExeDev;

impl Sandbox for ExeDev {
    fn create(&self, request: &CreateRequest) -> Result<Checkout, SplayError> {
        let template = request.template.ok_or_else(|| {
            SplayError::Sandbox("exe.dev workspaces are created from a template".to_string())
        })?;
        let vm = vm_name(request.repo, request.workspace_name);
        fs::create_dir_all(request.workspace_path)?;

        (request.progress)("Copying template");
        if let Err(err) = control(["cp".to_string(), template.to_string(), vm.clone()]) {
            let _ = fs::remove_dir_all(request.workspace_path);
            return Err(err);
        }

        (request.progress)("Updating checkout");
        let dir = shell_quote(&checkout_dir(request.repo));
        let script = format!(
            "cd {dir} && GIT_SSH_COMMAND='ssh -o StrictHostKeyChecking=accept-new' git fetch -q origin && git switch -q -c {branch} origin/HEAD",
            branch = shell_quote(request.workspace_name),
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

    fn new_template_name(&self, repo: &Repository) -> String {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        vm_name(repo, &format!("template-{timestamp}"))
    }

    fn build_template(&self, request: &TemplateRequest) -> Result<(), SplayError> {
        let vm = request.name;
        let dir = shell_quote(&checkout_dir(request.repo));

        log_step(request.log, &format!("Creating VM {vm}"))?;
        control_logged(
            [
                "new".to_string(),
                format!("--name={vm}"),
                format!("--disk={TEMPLATE_DISK}"),
            ],
            request.log,
        )?;

        log_step(
            request.log,
            &format!("Cloning {}", request.repo.remote_url()),
        )?;
        vm_exec_logged(
            vm,
            &format!(
                "GIT_SSH_COMMAND='ssh -o StrictHostKeyChecking=accept-new' git clone {url} {dir}",
                url = shell_quote(&request.repo.remote_url()),
            ),
            request.log,
        )?;

        if let Some(script) = request.setup_script {
            log_step(request.log, "Running setup script")?;
            vm_exec_logged(
                vm,
                &format!("cd {dir} && bash -lc {} setup", shell_quote(script)),
                request.log,
            )?;
        }

        // Copies snapshot the disk, so anything still only in memory would
        // be missing from them, leaving e.g. a corrupt git index.
        log_step(request.log, "Flushing the disk")?;
        vm_exec_logged(vm, "sync", request.log)?;

        Ok(())
    }

    fn destroy_template(&self, name: &str) -> Result<(), SplayError> {
        control(["rm".to_string(), name.to_string()]).map(|_| ())
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

fn log_step(log: &File, step: &str) -> Result<(), SplayError> {
    writeln!(&*log, "==> {step}")?;
    Ok(())
}

/// Runs an exe.dev control command with its output going to `log`.
fn control_logged<I>(args: I, log: &File) -> Result<(), SplayError>
where
    I: IntoIterator<Item = String>,
{
    let status = Command::new("ssh")
        .args(["-o", "BatchMode=yes", CONTROL_HOST])
        .args(args)
        .stdin(Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log.try_clone()?)
        .status()?;
    if !status.success() {
        return Err(SplayError::Sandbox(format!(
            "`ssh {CONTROL_HOST}` failed with {status}"
        )));
    }
    Ok(())
}

/// Runs a shell script on the VM with its output going to `log`, retrying
/// while the VM's SSH server comes up.
fn vm_exec_logged(vm: &str, script: &str, log: &File) -> Result<(), SplayError> {
    let host = vm_host(vm);
    let mut attempt = 1;
    loop {
        let status = Command::new("ssh")
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
            .stdout(log.try_clone()?)
            .stderr(log.try_clone()?)
            .status()?;

        if status.code() == Some(SSH_ERROR_STATUS) && attempt < SSH_CONNECT_ATTEMPTS {
            attempt += 1;
            thread::sleep(SSH_CONNECT_RETRY_DELAY);
            continue;
        }
        if !status.success() {
            return Err(SplayError::Sandbox(format!(
                "command on {vm} failed with {status}"
            )));
        }
        return Ok(());
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
