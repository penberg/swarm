mod cmd;
mod opts;

use clap::Parser;

use crate::opts::{Command, Opts};

#[tokio::main]
async fn main() {
    let opts = Opts::parse();

    let result = match opts.command {
        Command::Prune(cmd) => cmd::prune::run(cmd).await,
        Command::Repo(cmd) => cmd::repo::run(cmd).await,
        Command::Session(cmd) => cmd::session::run(cmd).await,
        Command::Workspace(cmd) => cmd::workspace::run(cmd).await,
    };

    if let Err(err) = result {
        eprintln!("error: {err}");
        std::process::exit(1);
    }
}
