use crate::opts::{RepoCommand, RepoSubcommand};
use swarm_core::{SwarmError, repos::RepositoryStore};

pub async fn run(cmd: RepoCommand) -> Result<(), SwarmError> {
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
    }

    Ok(())
}
