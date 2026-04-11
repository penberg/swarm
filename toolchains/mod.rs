pub mod cargo;

use std::path::Path;

pub struct SessionEnvVar {
    pub key: &'static str,
    pub value: Vec<u8>,
    pub overwrite: bool,
}

pub fn session_env(repo_dir: &Path, workspace_dir: &Path) -> Vec<SessionEnvVar> {
    let mut vars = Vec::new();
    vars.extend(cargo::session_env(repo_dir, workspace_dir));
    vars
}
