use crate::{
    SplayError, client,
    opts::{ServerCommand, ServerSubcommand},
    server,
};

pub async fn run(cmd: ServerCommand) -> Result<(), SplayError> {
    match cmd.command {
        None => server::serve(cmd.background).await,
        Some(ServerSubcommand::Status) => {
            match client::status().await? {
                Some(status) => {
                    println!("Running: pid {}", status.pid);
                    println!("Build: {}", status.build);
                    println!("Data: {}", status.data_dir.display());
                }
                None => println!("Not running"),
            }
            Ok(())
        }
        Some(ServerSubcommand::Stop) => {
            if client::stop().await? {
                println!("Stopping server");
            } else {
                println!("Not running");
            }
            Ok(())
        }
    }
}
