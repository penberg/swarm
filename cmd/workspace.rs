use crate::{
    SplayError,
    api::{Request, WorkspaceWithSessions},
    client,
    opts::{WorkspaceCommand, WorkspaceSubcommand},
    sandbox::SandboxKind,
    workspaces::Workspace,
};

pub async fn run(cmd: WorkspaceCommand) -> Result<(), SplayError> {
    match cmd.command {
        WorkspaceSubcommand::Create {
            repository,
            name,
            sandbox,
        } => {
            let created: WorkspaceWithSessions = client::call_with_progress(
                Request::WorkspaceCreate {
                    repository,
                    name,
                    sandbox: SandboxKind::parse(&sandbox)?,
                },
                &|stage| eprintln!("{stage}..."),
            )
            .await?;
            let session = first_session(&created)?;
            println!(
                "Created workspace {} for {}",
                created.workspace.name, created.workspace.repository_alias
            );
            println!("Created session {session}");
        }
        WorkspaceSubcommand::Clone { workspace, name } => {
            let cloned: WorkspaceWithSessions = client::call(Request::WorkspaceClone {
                workspace: workspace.clone(),
                name,
            })
            .await?;
            let session = first_session(&cloned)?;
            println!(
                "Cloned workspace {} to {} for {}",
                workspace, cloned.workspace.name, cloned.workspace.repository_alias
            );
            println!("Created session {session}");
        }
        WorkspaceSubcommand::List { repository, json } => {
            let workspaces: Vec<Workspace> =
                client::call(Request::WorkspaceList { repository }).await?;

            if json {
                println!("{}", serde_json::to_string_pretty(&workspaces)?);
                return Ok(());
            }

            println!("{:<20} {:<20} PATH", "WORKSPACE", "BRANCH");
            for workspace in workspaces {
                println!(
                    "{:<20} {:<20} {}",
                    workspace.name,
                    workspace.branch,
                    workspace.path.display()
                );
            }
        }
        WorkspaceSubcommand::Info { workspace } => {
            let workspace: Workspace = client::call(Request::WorkspaceInfo { workspace }).await?;
            println!("Repository: {}", workspace.repository);
            println!("Workspace: {}", workspace.name);
            println!("Branch: {}", workspace.branch);
            println!("Path: {}", workspace.path.display());
            println!("Sandbox: {}", workspace.sandbox);
            if let Some(sandbox_ref) = &workspace.sandbox_ref {
                println!("Sandbox reference: {sandbox_ref}");
            }
            println!("Created: {}", workspace.created_at);
        }
        WorkspaceSubcommand::Remove { workspace } => {
            let workspace: Workspace = client::call(Request::WorkspaceRemove {
                workspace,
                stop_sessions: false,
            })
            .await?;
            println!(
                "Removed workspace {} from {}",
                workspace.name, workspace.repository_alias
            );
        }
    }

    Ok(())
}

/// Returns the session the workspace started with, or why it did not start.
fn first_session(created: &WorkspaceWithSessions) -> Result<&str, SplayError> {
    if let Some(error) = &created.session_error {
        return Err(SplayError::Server(error.clone()));
    }
    created
        .sessions
        .last()
        .map(|session| session.id.as_str())
        .ok_or_else(|| SplayError::InvalidSession("workspace started no session".to_string()))
}
