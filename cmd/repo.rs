use std::path::Path;

use crate::{
    SplayError,
    api::Request,
    client,
    opts::{RepoCommand, RepoSubcommand},
    repos::Repository,
    sandbox::SandboxKind,
    templates::{self, TemplateStatus},
};

pub async fn run(cmd: RepoCommand) -> Result<(), SplayError> {
    match cmd.command {
        // The detached template builder runs next to the server, not in it.
        RepoSubcommand::BuildTemplate {
            template_dir,
            remote_url,
            sandbox,
        } => {
            templates::run_build(
                Path::new(&template_dir),
                &remote_url,
                SandboxKind::parse(&sandbox)?,
            )?;
        }
        RepoSubcommand::Add { repository, alias } => {
            let repo: Repository = client::call(Request::RepoAdd { repository, alias }).await?;
            println!("Added repo {}", alias_label(&repo));
        }
        RepoSubcommand::Sync { repository } => {
            let repo: Repository = client::call(Request::RepoSync { repository }).await?;
            println!("Synced repo {}", alias_label(&repo));
        }
        RepoSubcommand::Remove { repository } => {
            let repo: Repository = client::call(Request::RepoRemove { repository }).await?;
            println!("Removed repo {}", alias_label(&repo));
        }
        RepoSubcommand::Archive { repository } => {
            let repo: Repository = client::call(Request::RepoArchive { repository }).await?;
            println!("Archived repo {}", alias_label(&repo));
        }
        RepoSubcommand::Unarchive { repository } => {
            let repo: Repository = client::call(Request::RepoUnarchive { repository }).await?;
            println!("Unarchived repo {}", alias_label(&repo));
        }
        RepoSubcommand::List { json } => {
            let repos: Vec<Repository> = client::call(Request::RepoList).await?;

            if json {
                println!("{}", serde_json::to_string_pretty(&repos)?);
                return Ok(());
            }

            println!("{:<16} REPOSITORY", "ALIAS");
            for repo in repos {
                println!(
                    "{:<16} {}{}",
                    alias_label(&repo),
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
            let _: TemplateStatus = client::call(Request::RepoSetSandboxEnabled {
                repository: repository.clone(),
                sandbox: SandboxKind::parse(&integration)?,
                enabled: true,
            })
            .await?;
            println!("Enabled {integration} for {repository}; building its template");
        }
        RepoSubcommand::Disable {
            repository,
            integration,
        } => {
            let _: TemplateStatus = client::call(Request::RepoSetSandboxEnabled {
                repository: repository.clone(),
                sandbox: SandboxKind::parse(&integration)?,
                enabled: false,
            })
            .await?;
            println!("Disabled {integration} for {repository}");
        }
        RepoSubcommand::Template {
            repository,
            sandbox,
            rebuild,
        } => {
            let status: TemplateStatus = client::call(Request::TemplateStatus {
                repository,
                sandbox: SandboxKind::parse(&sandbox)?,
                rebuild,
            })
            .await?;
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
    }

    Ok(())
}

fn alias_label(repo: &Repository) -> &str {
    repo.alias.as_deref().unwrap_or("-")
}
