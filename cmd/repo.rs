use std::path::Path;

use crate::{
    SplayError,
    opts::{RepoCommand, RepoSubcommand},
    repos::RepositoryStore,
    sandbox::SandboxKind,
    templates,
};

pub async fn run(cmd: RepoCommand) -> Result<(), SplayError> {
    // The template builder runs for minutes and must not hold the index
    // database open meanwhile, as that locks out every other process.
    if let RepoSubcommand::BuildTemplate {
        template_dir,
        remote_url,
        sandbox,
    } = &cmd.command
    {
        return templates::run_build(
            Path::new(template_dir),
            remote_url,
            SandboxKind::parse(sandbox)?,
        );
    }

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
        RepoSubcommand::Archive { repository } => {
            let repo = store.archive(&repository).await?;
            println!("Archived repo {}", repo.alias.as_deref().unwrap_or("-"));
        }
        RepoSubcommand::Unarchive { repository } => {
            let repo = store.unarchive(&repository).await?;
            println!("Unarchived repo {}", repo.alias.as_deref().unwrap_or("-"));
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
                    "{:<16} {}{}",
                    repo.alias.as_deref().unwrap_or("-"),
                    repo.canonical(),
                    if repo.archived_at.is_some() {
                        " (archived)"
                    } else {
                        ""
                    }
                );
            }
        }
        RepoSubcommand::Enable {
            repository,
            integration,
        } => {
            templates::set_sandbox_enabled(
                &store,
                &repository,
                SandboxKind::parse(&integration)?,
                true,
            )
            .await?;
            println!("Enabled {integration} for {repository}; building its template");
        }
        RepoSubcommand::Disable {
            repository,
            integration,
        } => {
            templates::set_sandbox_enabled(
                &store,
                &repository,
                SandboxKind::parse(&integration)?,
                false,
            )
            .await?;
            println!("Disabled {integration} for {repository}");
        }
        RepoSubcommand::Template {
            repository,
            sandbox,
            rebuild,
        } => {
            let sandbox = SandboxKind::parse(&sandbox)?;
            let repo = store
                .resolve_repository(&repository)
                .await?
                .ok_or_else(|| SplayError::RepositoryNotFound(repository.clone()))?;
            if rebuild {
                templates::rebuild(&store, &repo, sandbox).await?;
            }

            let status = templates::status(&store, &repo, sandbox);
            match &status.ready {
                Some(ready) => println!("Template: {} (built at {})", ready.name, ready.built_at),
                None => println!("Template: none"),
            }
            if let Some(since) = status.building_since {
                println!("Building since {since}");
            }
            if let Some(error) = &status.error {
                println!("Last build failed: {error}");
            }
        }
        RepoSubcommand::BuildTemplate { .. } => unreachable!("handled above"),
    }

    Ok(())
}
