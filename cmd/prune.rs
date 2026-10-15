use crate::{
    SplayError,
    api::{PruneSummary, Request},
    client,
    opts::{PruneCommand, PruneSubcommand},
};

pub async fn run(cmd: PruneCommand) -> Result<(), SplayError> {
    match cmd.command {
        PruneSubcommand::All => {
            let sessions: usize = client::call(Request::PruneSessions).await?;
            let workspaces: PruneSummary = client::call(Request::PruneWorkspaces).await?;
            println!("Pruned {} sessions", sessions);
            report_workspaces(&workspaces)?;
        }
        PruneSubcommand::Sessions => {
            let pruned: usize = client::call(Request::PruneSessions).await?;
            println!("Pruned {} sessions", pruned);
        }
        PruneSubcommand::Workspaces => {
            let workspaces: PruneSummary = client::call(Request::PruneWorkspaces).await?;
            report_workspaces(&workspaces)?;
        }
    }

    Ok(())
}

fn report_workspaces(report: &PruneSummary) -> Result<(), SplayError> {
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
    eprintln!("note: remove the directories listed above manually, then run `splay prune` again");
    Err(SplayError::PruneIncomplete(report.failed.len()))
}
