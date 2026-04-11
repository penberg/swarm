use crate::opts::{PruneCommand, PruneSubcommand};
use swarm_core::{SwarmError, prune::PruneStore};

pub async fn run(cmd: PruneCommand) -> Result<(), SwarmError> {
    let store = PruneStore::open().await?;

    match cmd.command {
        PruneSubcommand::All => {
            let sessions = store.sessions().await?;
            let workspaces = store.workspaces().await?;
            println!("Pruned {} sessions", sessions);
            println!("Pruned {} archived workspaces", workspaces);
        }
        PruneSubcommand::Sessions => {
            let pruned = store.sessions().await?;
            println!("Pruned {} sessions", pruned);
        }
        PruneSubcommand::Workspaces => {
            let pruned = store.workspaces().await?;
            println!("Pruned {} archived workspaces", pruned);
        }
    }

    Ok(())
}
