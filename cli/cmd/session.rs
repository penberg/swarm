use crate::opts::{SessionCommand, SessionSubcommand};
use swarm_core::{
    SwarmError,
    sessions::{SessionStore, serve_runtime},
};

pub async fn run(cmd: SessionCommand) -> Result<(), SwarmError> {
    match cmd.command {
        SessionSubcommand::Create { workspace, command } => {
            let store = SessionStore::open().await?;
            let session = store.create(&workspace, &command).await?;
            println!("Created session {}", session.id);
        }
        SessionSubcommand::List { workspace, json } => {
            let store = SessionStore::open().await?;
            let sessions = store.list(workspace.as_deref()).await?;

            if json {
                println!("{}", serde_json::to_string_pretty(&sessions)?);
                return Ok(());
            }

            println!(
                "{:<18} {:<12} {:<12} {:<20} COMMAND",
                "SESSION", "STATUS", "WORKSPACE", "PID"
            );
            for session in sessions {
                let pid = session
                    .pid
                    .map(|pid| pid.to_string())
                    .unwrap_or_else(|| "-".to_string());
                println!(
                    "{:<18} {:<12} {:<12} {:<20} {}",
                    session.id,
                    session.status,
                    session.workspace,
                    pid,
                    session.command.join(" ")
                );
            }
        }
        SessionSubcommand::Info { session } => {
            let store = SessionStore::open().await?;
            let session = store.info(&session).await?;
            println!("Session: {}", session.id);
            println!("Repository: {}", session.repository);
            println!("Workspace: {}", session.workspace);
            println!("Status: {}", session.status);
            println!(
                "PID: {}",
                session.pid.map_or("-".to_string(), |pid| pid.to_string())
            );
            println!("Path: {}", session.path.display());
            println!("Log: {}", session.log_path.display());
            println!("Command: {}", session.command.join(" "));
            println!("Created: {}", session.created_at);
            println!(
                "Exited: {}",
                session
                    .exit_code
                    .map_or("-".to_string(), |code| code.to_string())
            );
        }
        SessionSubcommand::Attach { session, follow } => {
            let store = SessionStore::open().await?;
            store.attach(&session, follow).await?;
        }
        SessionSubcommand::Stop { session } => {
            let store = SessionStore::open().await?;
            let session = store.stop(&session).await?;
            println!("Stopped session {}", session.id);
        }
        SessionSubcommand::Remove { session } => {
            let store = SessionStore::open().await?;
            let session = store.remove(&session).await?;
            println!("Removed session {}", session.id);
        }
        SessionSubcommand::Serve {
            repository,
            repo_db_path,
            workspace,
            session,
            session_dir,
            workspace_dir,
            command,
        } => {
            serve_runtime(
                &repository,
                &repo_db_path,
                &workspace,
                &session,
                &session_dir,
                &workspace_dir,
                &command,
            )
            .await?;
        }
    }

    Ok(())
}
