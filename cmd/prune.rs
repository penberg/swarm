use crate::{
    SwarmError,
    opts::{PruneCommand, PruneSubcommand},
    prune::PruneStore,
    workspaces::PruneReport,
};

pub async fn run(cmd: PruneCommand) -> Result<(), SwarmError> {
    let store = PruneStore::open().await?;

    match cmd.command {
        PruneSubcommand::All => {
            let sessions = store.sessions().await?;
            let workspaces = store.workspaces().await?;
            println!("Pruned {} sessions", sessions);
            report_workspaces(&workspaces)?;
        }
        PruneSubcommand::Sessions => {
            let pruned = store.sessions().await?;
            println!("Pruned {} sessions", pruned);
        }
        PruneSubcommand::Workspaces => {
            let workspaces = store.workspaces().await?;
            report_workspaces(&workspaces)?;
        }
    }

    Ok(())
}

fn report_workspaces(report: &PruneReport) -> Result<(), SwarmError> {
    println!("Pruned {} archived workspaces", report.pruned.len());

    if report.failed.is_empty() {
        return Ok(());
    }
    for failure in &report.failed {
        eprintln!(
            "warning: workspace `{}`: cannot remove `{}`: {}",
            failure.name,
            failure.path.display(),
            failure.error
        );
    }
    eprintln!(
        "note: remove the directories listed above manually, then run `swarmctl prune` again"
    );
    Err(SwarmError::PruneIncomplete(report.failed.len()))
}
