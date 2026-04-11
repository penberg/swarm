use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(name = "swarm")]
#[command(about = "Manage coding-agent repositories and workspaces")]
pub struct Opts {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    Prune(PruneCommand),
    Repo(RepoCommand),
    Session(SessionCommand),
    #[command(alias = "ws")]
    Workspace(WorkspaceCommand),
}

#[derive(Debug, Parser)]
pub struct RepoCommand {
    #[command(subcommand)]
    pub command: RepoSubcommand,
}

#[derive(Debug, Subcommand)]
pub enum RepoSubcommand {
    Add {
        repository: String,
        #[arg(long)]
        alias: Option<String>,
    },
    Sync {
        repository: String,
    },
    Remove {
        repository: String,
    },
    List {
        #[arg(long)]
        json: bool,
    },
}

#[derive(Debug, Parser)]
pub struct WorkspaceCommand {
    #[command(subcommand)]
    pub command: WorkspaceSubcommand,
}

#[derive(Debug, Subcommand)]
pub enum WorkspaceSubcommand {
    Create {
        repository: String,
        name: Option<String>,
    },
    Clone {
        workspace: String,
        name: String,
    },
    List {
        repository: String,
        #[arg(long)]
        json: bool,
    },
    Info {
        workspace: String,
    },
    Remove {
        workspace: String,
    },
}

#[derive(Debug, Parser)]
pub struct SessionCommand {
    #[command(subcommand)]
    pub command: SessionSubcommand,
}

#[derive(Debug, Subcommand)]
pub enum SessionSubcommand {
    Create {
        workspace: String,
        #[arg(required = true, last = true)]
        command: Vec<String>,
    },
    List {
        workspace: Option<String>,
        #[arg(long)]
        json: bool,
    },
    Info {
        session: String,
    },
    Attach {
        session: String,
        #[arg(long)]
        follow: bool,
    },
    Stop {
        session: String,
    },
    Remove {
        session: String,
    },
    #[command(hide = true)]
    Serve {
        repository: String,
        repo_db_path: String,
        workspace: String,
        session: String,
        session_dir: String,
        workspace_dir: String,
        #[arg(required = true, last = true)]
        command: Vec<String>,
    },
}

#[derive(Debug, Parser)]
pub struct PruneCommand {
    #[command(subcommand)]
    pub command: PruneSubcommand,
}

#[derive(Debug, Subcommand)]
pub enum PruneSubcommand {
    All,
    Sessions,
    Workspaces,
}
