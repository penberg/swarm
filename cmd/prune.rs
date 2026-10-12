use crate::{
    SplayError,
    opts::{PruneCommand, PruneSubcommand},
    prune::PruneStore,
    workspaces::PruneReport,
};

pub async fn run(cmd: PruneCommand) -> Result<(), SplayError> {
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

fn report_workspaces(report: &PruneReport) -> Result<(), SplayError> {
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
        "note: remove the directories listed above manually, then run `splayctl prune` again"
    );
    Err(SplayError::PruneIncomplete(report.failed.len()))
}
