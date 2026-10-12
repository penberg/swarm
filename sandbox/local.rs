use std::fs;

use crate::{
    SplayError,
    repos::Repository,
    sandbox::{Checkout, CreateRequest, RepoPaths, Sandbox},
    workspaces::{Workspace, build_worktree_add_args, run_git},
};

/// Git worktree on this machine; sessions run as local processes.
pub struct Local;

impl Sandbox for Local {
    fn create(&self, request: &CreateRequest) -> Result<Checkout, SplayError> {
        (request.progress)("Creating worktree");
        let args = build_worktree_add_args(
            request.paths.bare_repo_path,
            request.workspace_path,
            request.workspace_name,
            request.default_branch,
        )?;
        run_git(request.workspace_path.parent(), args)?;

        Ok(Checkout {
            branch: request.workspace_name.to_string(),
            sandbox_ref: None,
        })
    }

    fn destroy(&self, workspace: &Workspace, paths: &RepoPaths) -> Result<(), SplayError> {
        let git_dir = format!("--git-dir={}", paths.bare_repo_path.display());
        let removed = run_git(
            Some(paths.repo_dir),
            [
                git_dir.clone(),
                "worktree".to_string(),
                "remove".to_string(),
                "--force".to_string(),
                workspace.path.display().to_string(),
            ],
        );

        if removed.is_err() {
            // Git refuses to remove a worktree it no longer recognizes,
            // e.g. when the directory was deleted out from under it.
            // Clean up whatever is left on disk and let `git worktree
            // prune` discard the stale bookkeeping.
            if workspace.path.exists() {
                fs::remove_dir_all(&workspace.path)?;
            }
            let _ = run_git(
                Some(paths.repo_dir),
                [git_dir, "worktree".to_string(), "prune".to_string()],
            );
        }

        Ok(())
    }

    fn session_command(
        &self,
        _repo: &Repository,
        _workspace: &Workspace,
        command: &[String],
    ) -> Result<Vec<String>, SplayError> {
        Ok(command.to_vec())
    }
}
