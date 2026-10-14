use crate::{
    SplayError,
    api::Request,
    client,
    opts::{SessionCommand, SessionSubcommand},
    sessions::{Session, attach, serve_runtime},
};

pub async fn run(cmd: SessionCommand) -> Result<(), SplayError> {
    match cmd.command {
        SessionSubcommand::Create { workspace, command } => {
            let session: Session = client::call(Request::SessionCreate {
                workspace,
                command: Some(command),
            })
            .await?;
            println!("Created session {}", session.id);
        }
        SessionSubcommand::List { workspace, json } => {
            let sessions: Vec<Session> = client::call(Request::SessionList { workspace }).await?;

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
            let session: Session = client::call(Request::SessionInfo { session }).await?;
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
        SessionSubcommand::Attach { session, follow: _ } => {
            let session: Session = client::call(Request::SessionInfo { session }).await?;
            attach(&session)?;
        }
        SessionSubcommand::Stop { session } => {
            let session: Session = client::call(Request::SessionStop { session }).await?;
            println!("Stopped session {}", session.id);
        }
        SessionSubcommand::Remove { session } => {
            let session: Session = client::call(Request::SessionRemove { session }).await?;
            println!("Removed session {}", session.id);
        }
        // The session host runs next to the server, not in it.
        SessionSubcommand::Serve {
            repository,
            workspace,
            session,
            session_dir,
            workspace_dir,
            command,
        } => {
            serve_runtime(
                &repository,
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
