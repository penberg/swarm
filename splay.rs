use clap::Parser;
use splay::{
    cmd,
    opts::{Command, Opts},
};

#[cfg(feature = "ui")]
#[path = "ui/app.rs"]
mod app;
#[cfg(feature = "ui")]
#[path = "ui/clamp.rs"]
mod clamp;
#[cfg(feature = "ui")]
#[path = "ui/data.rs"]
mod data;
#[cfg(feature = "ui")]
#[path = "ui/exec.rs"]
mod exec;
#[cfg(feature = "ui")]
#[path = "ui/ghostty.rs"]
mod ghostty;
#[cfg(feature = "ui")]
#[path = "ui/repo_settings.rs"]
mod repo_settings;
#[cfg(feature = "ui")]
#[path = "ui/workspace_panel.rs"]
mod workspace_panel;

fn main() {
    let opts = Opts::parse();
    let command = match opts.command {
        None | Some(Command::Ui) => return run_ui(),
        Some(command) => command,
    };

    let runtime = tokio::runtime::Runtime::new().expect("failed to build tokio runtime");
    let result = runtime.block_on(async {
        match command {
            Command::Ui => unreachable!("handled above"),
            Command::Server(cmd) => cmd::server::run(cmd).await,
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
}

#[cfg(feature = "ui")]
fn run_ui() {
    if let Err(err) = app::run() {
        eprintln!("error: {err}");
        std::process::exit(1);
    }
}

#[cfg(not(feature = "ui"))]
fn run_ui() {
    eprintln!("error: this build of splay has no desktop app; see `splay --help`");
    std::process::exit(1);
}
