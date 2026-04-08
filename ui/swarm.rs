use clap::Parser;
use swarm::{
    cmd,
    opts::{Command, Opts},
};

mod app;
mod data;
mod exec;
mod ghostty;
mod workspace_panel;

fn main() {
    if should_run_cli() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("failed to build tokio runtime");
        let opts = Opts::parse();

        let result = runtime.block_on(async {
            match opts.command {
                Command::Prune(cmd) => cmd::prune::run(cmd).await,
                Command::Repo(cmd) => cmd::repo::run(cmd).await,
                Command::Session(cmd) => cmd::session::run(cmd).await,
                Command::Workspace(cmd) => cmd::workspace::run(cmd).await,
            }
        });

        if let Err(err) = result {
            eprintln!("error: {err}");
            std::process::exit(1);
        }
        return;
    }

    if let Err(err) = app::run() {
        eprintln!("error: {err}");
        std::process::exit(1);
    }
}

fn should_run_cli() -> bool {
    matches!(
        std::env::args().nth(1).as_deref(),
        Some("prune")
            | Some("repo")
            | Some("session")
            | Some("workspace")
            | Some("ws")
            | Some("--help")
            | Some("-h")
            | Some("--version")
            | Some("-V")
    )
}
