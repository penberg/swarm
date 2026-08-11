mod app;
mod data;
mod exec;
mod ghostty;
mod workspace_panel;

fn main() {
    if let Err(err) = app::run() {
        eprintln!("error: {err}");
        std::process::exit(1);
    }
}
