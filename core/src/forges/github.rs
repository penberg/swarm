use serde_json::Value;
use std::{path::Path, process::Command};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PullRequestStatusState {
    Success,
    Pending,
    Failure,
    Merged,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PullRequestStatus {
    pub number: u64,
    pub state: PullRequestStatusState,
    pub summary: String,
    pub url: Option<String>,
}

pub fn workspace_pull_request_status(workspace_path: &Path) -> Option<PullRequestStatus> {
    if !is_github_workspace(workspace_path) {
        return None;
    }

    let output = Command::new("gh")
        .current_dir(workspace_path)
        .args(["pr", "view", "--json", "number,state,statusCheckRollup,url"])
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    parse_pull_request_status(&output.stdout).ok().flatten()
}

/// Returns the number of commits the workspace branch is ahead of its base
/// branch (e.g. `origin/main`). Returns `None` if the base branch cannot be
/// determined or the count cannot be computed.
pub fn workspace_commits_ahead(workspace_path: &Path) -> Option<u32> {
    let base = workspace_base_ref(workspace_path)?;
    let range = format!("{base}..HEAD");
    let output = Command::new("git")
        .current_dir(workspace_path)
        .args(["rev-list", "--count", range.as_str()])
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    String::from_utf8_lossy(&output.stdout).trim().parse().ok()
}

/// Creates a pull request for the workspace branch using `gh pr create --fill`.
/// Returns `Ok(())` on success or an error message captured from `gh`.
pub fn create_pull_request(workspace_path: &Path) -> Result<(), String> {
    if !is_github_workspace(workspace_path) {
        return Err("not a GitHub workspace".to_string());
    }

    let branch = workspace_branch_name(workspace_path)?;
    let base = workspace_base_branch(workspace_path);
    push_workspace_branch(workspace_path)?;

    let mut command = Command::new("gh");
    command.current_dir(workspace_path);
    command.args(["pr", "create", "--fill", "--head", branch.as_str()]);
    if let Some(base) = base.as_deref() {
        command.args(["--base", base]);
    }

    let output = command
        .output()
        .map_err(|err| format!("failed to spawn gh: {err}"))?;

    if !output.status.success() {
        let stderr = format_command_stderr(&output.stderr);
        if stderr.is_empty() {
            return Err(format!("gh pr create failed with status {}", output.status));
        }
        return Err(stderr);
    }

    Ok(())
}

fn push_workspace_branch(workspace_path: &Path) -> Result<(), String> {
    let output = Command::new("git")
        .current_dir(workspace_path)
        .args(["push", "--set-upstream", "origin", "HEAD"])
        .output()
        .map_err(|err| format!("failed to spawn git push: {err}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        if stderr.is_empty() {
            return Err(format!("git push failed with status {}", output.status));
        }
        return Err(stderr);
    }

    Ok(())
}

fn workspace_branch_name(workspace_path: &Path) -> Result<String, String> {
    let output = Command::new("git")
        .current_dir(workspace_path)
        .args(["branch", "--show-current"])
        .output()
        .map_err(|err| format!("failed to spawn git branch: {err}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        if stderr.is_empty() {
            return Err(format!("git branch failed with status {}", output.status));
        }
        return Err(stderr);
    }

    let branch = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if branch.is_empty() {
        return Err("unable to determine current branch".to_string());
    }

    Ok(branch)
}

fn workspace_base_branch(workspace_path: &Path) -> Option<String> {
    workspace_base_ref(workspace_path)
        .and_then(|base| base.strip_prefix("origin/").map(str::to_string))
}

fn workspace_base_ref(workspace_path: &Path) -> Option<String> {
    let output = Command::new("git")
        .current_dir(workspace_path)
        .args(["rev-parse", "--abbrev-ref", "origin/HEAD"])
        .output()
        .ok()?;

    if output.status.success() {
        let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !value.is_empty() && value != "HEAD" {
            return Some(value);
        }
    }

    for fallback in ["origin/main", "origin/master"] {
        let exists = Command::new("git")
            .current_dir(workspace_path)
            .args(["rev-parse", "--verify", "--quiet", fallback])
            .output()
            .ok()?;
        if exists.status.success() {
            return Some(fallback.to_string());
        }
    }

    None
}

fn format_command_stderr(stderr: &[u8]) -> String {
    let lines: Vec<String> = String::from_utf8_lossy(stderr)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect();

    let meaningful: Vec<String> = lines
        .iter()
        .filter(|line| !line.starts_with("Warning:"))
        .cloned()
        .collect();

    if meaningful.is_empty() {
        lines.join("\n")
    } else {
        meaningful.join("\n")
    }
}

pub fn is_github_workspace(workspace_path: &Path) -> bool {
    let output = Command::new("git")
        .current_dir(workspace_path)
        .args(["config", "--get", "remote.origin.url"])
        .output();
    let Ok(output) = output else {
        return false;
    };

    if !output.status.success() {
        return false;
    }

    let remote = String::from_utf8_lossy(&output.stdout);
    let remote = remote.trim();

    remote.contains("github.com")
        || remote.starts_with("git@github:")
        || remote.starts_with("ssh://git@github.com/")
}

fn parse_pull_request_status(bytes: &[u8]) -> Result<Option<PullRequestStatus>, serde_json::Error> {
    let payload: Value = serde_json::from_slice(bytes)?;
    let Some(number) = payload.get("number").and_then(Value::as_u64) else {
        return Ok(None);
    };
    let Some(pr_state) = payload.get("state").and_then(Value::as_str) else {
        return Ok(None);
    };
    if pr_state == "MERGED" {
        return Ok(Some(PullRequestStatus {
            number,
            state: PullRequestStatusState::Merged,
            summary: format!("PR #{number} merged"),
            url: payload
                .get("url")
                .and_then(Value::as_str)
                .map(str::to_string),
        }));
    }
    if pr_state != "OPEN" {
        return Ok(None);
    }

    let checks = payload
        .get("statusCheckRollup")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let state = summarize_checks(&checks);
    let summary = match state {
        PullRequestStatusState::Success => format!("PR #{number} checks passing"),
        PullRequestStatusState::Pending => format!("PR #{number} checks pending"),
        PullRequestStatusState::Failure => format!("PR #{number} checks failing"),
        PullRequestStatusState::Merged => format!("PR #{number} merged"),
    };

    Ok(Some(PullRequestStatus {
        number,
        state,
        summary,
        url: payload
            .get("url")
            .and_then(Value::as_str)
            .map(str::to_string),
    }))
}

fn summarize_checks(checks: &[Value]) -> PullRequestStatusState {
    let mut has_pending = false;

    for check in checks {
        if is_failed_check(check) {
            return PullRequestStatusState::Failure;
        }

        if is_pending_check(check) {
            has_pending = true;
        }
    }

    if has_pending || checks.is_empty() {
        PullRequestStatusState::Pending
    } else {
        PullRequestStatusState::Success
    }
}

fn is_failed_check(check: &Value) -> bool {
    let conclusion = check.get("conclusion").and_then(Value::as_str);
    let status = check
        .get("status")
        .and_then(Value::as_str)
        .or_else(|| check.get("state").and_then(Value::as_str));

    matches!(
        conclusion,
        Some(
            "ACTION_REQUIRED" | "CANCELLED" | "FAILURE" | "STALE" | "STARTUP_FAILURE" | "TIMED_OUT"
        )
    ) || matches!(status, Some("FAILURE" | "ERROR"))
}

fn is_pending_check(check: &Value) -> bool {
    let conclusion = check.get("conclusion").and_then(Value::as_str);
    let status = check
        .get("status")
        .and_then(Value::as_str)
        .or_else(|| check.get("state").and_then(Value::as_str));

    conclusion.is_none()
        || matches!(
            status,
            Some("EXPECTED" | "IN_PROGRESS" | "PENDING" | "QUEUED" | "REQUESTED" | "WAITING")
        )
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{
        PullRequestStatusState, format_command_stderr, parse_pull_request_status, summarize_checks,
    };

    #[test]
    fn reports_failing_checks() {
        let checks = vec![
            json!({ "conclusion": "SUCCESS", "status": "COMPLETED" }),
            json!({ "conclusion": "FAILURE", "status": "COMPLETED" }),
        ];

        assert_eq!(summarize_checks(&checks), PullRequestStatusState::Failure);
    }

    #[test]
    fn reports_pending_checks() {
        let checks = vec![json!({ "conclusion": null, "status": "IN_PROGRESS" })];

        assert_eq!(summarize_checks(&checks), PullRequestStatusState::Pending);
    }

    #[test]
    fn reports_successful_checks() {
        let checks = vec![
            json!({ "conclusion": "SUCCESS", "status": "COMPLETED" }),
            json!({ "conclusion": "SKIPPED", "status": "COMPLETED" }),
        ];

        assert_eq!(summarize_checks(&checks), PullRequestStatusState::Success);
    }

    #[test]
    fn parses_open_pull_request_payload() {
        let payload = json!({
            "number": 42,
            "state": "OPEN",
            "statusCheckRollup": [
                { "conclusion": "SUCCESS", "status": "COMPLETED" }
            ],
            "url": "https://github.com/example/repo/pull/42"
        });

        let status = parse_pull_request_status(payload.to_string().as_bytes())
            .expect("payload should parse")
            .expect("status should exist");

        assert_eq!(status.number, 42);
        assert_eq!(status.state, PullRequestStatusState::Success);
        assert_eq!(status.summary, "PR #42 checks passing");
    }

    #[test]
    fn parses_merged_pull_request_payload() {
        let payload = json!({
            "number": 42,
            "state": "MERGED",
            "statusCheckRollup": []
        });

        let status = parse_pull_request_status(payload.to_string().as_bytes())
            .expect("payload should parse")
            .expect("status should exist");

        assert_eq!(status.number, 42);
        assert_eq!(status.state, PullRequestStatusState::Merged);
        assert_eq!(status.summary, "PR #42 merged");
    }

    #[test]
    fn ignores_closed_pull_requests() {
        let payload = json!({
            "number": 42,
            "state": "CLOSED",
            "statusCheckRollup": []
        });

        let status = parse_pull_request_status(payload.to_string().as_bytes())
            .expect("payload should parse");

        assert!(status.is_none());
    }

    #[test]
    fn drops_warning_lines_when_real_error_exists() {
        let stderr = br#"
Warning: 1 uncommitted change
aborted: you must first push the current branch to a remote, or use the --head flag
"#;

        assert_eq!(
            format_command_stderr(stderr),
            "aborted: you must first push the current branch to a remote, or use the --head flag"
        );
    }

    #[test]
    fn keeps_warning_when_it_is_the_only_output() {
        let stderr = b"Warning: 1 uncommitted change\n";

        assert_eq!(
            format_command_stderr(stderr),
            "Warning: 1 uncommitted change"
        );
    }
}
