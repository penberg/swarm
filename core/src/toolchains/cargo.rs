use std::path::Path;

use crate::toolchains::SessionEnvVar;

pub fn session_env(repo_dir: &Path, workspace_dir: &Path) -> Vec<SessionEnvVar> {
    if !workspace_dir.join("Cargo.toml").exists() {
        return Vec::new();
    }

    vec![SessionEnvVar {
        key: "CARGO_TARGET_DIR",
        value: repo_dir
            .join("target")
            .into_os_string()
            .into_encoded_bytes(),
        overwrite: false,
    }]
}
