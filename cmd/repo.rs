use crate::{
    SplayError,
    opts::{RepoCommand, RepoSubcommand},
    repos::RepositoryStore,
    sandbox::SandboxKind,
};

pub async fn run(cmd: RepoCommand) -> Result<(), SplayError> {
    let store = RepositoryStore::open().await?;

    match cmd.command {
        RepoSubcommand::Add { repository, alias } => {
            let repo = store.add(&repository, alias.as_deref()).await?;
            println!("Added repo {}", repo.alias.as_deref().unwrap_or("-"));
        }
        RepoSubcommand::Sync { repository } => {
            let repo = store.sync(&repository).await?;
            println!("Synced repo {}", repo.alias.as_deref().unwrap_or("-"));
        }
        RepoSubcommand::Remove { repository } => {
            let repo = store.remove(&repository).await?;
            println!("Removed repo {}", repo.alias.as_deref().unwrap_or("-"));
        }
        RepoSubcommand::List { json } => {
            let repos = store.list().await?;

            if json {
                println!("{}", serde_json::to_string_pretty(&repos)?);
                return Ok(());
            }

            println!("{:<16} REPOSITORY", "ALIAS");
            for repo in repos {
                println!(
                    "{:<16} {}",
                    repo.alias.as_deref().unwrap_or("-"),
                    repo.canonical()
                );
            }
        }
        RepoSubcommand::Enable {
            repository,
            integration,
        } => {
            store
                .set_sandbox_enabled(&repository, SandboxKind::parse(&integration)?, true)
                .await?;
            println!("Enabled {integration} for {repository}");
        }
        RepoSubcommand::Disable {
            repository,
            integration,
        } => {
            store
                .set_sandbox_enabled(&repository, SandboxKind::parse(&integration)?, false)
                .await?;
            println!("Disabled {integration} for {repository}");
        }
    }

    Ok(())
}
