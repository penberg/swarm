//! `splay server`: the one process that owns Splay's state.
//!
//! The server holds an exclusive lock on the data directory and keeps the
//! index database open for as long as it runs, so no other process can open
//! the databases. Clients reach it over a Unix socket, whose path the server
//! records in the data directory; see [`crate::api`] for the protocol.

use std::{
    env,
    fs::{self, File, OpenOptions},
    hash::{DefaultHasher, Hash, Hasher},
    io,
    os::{fd::AsRawFd, unix::fs::PermissionsExt},
    path::{Path, PathBuf},
    sync::Arc,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::{UnixListener, UnixStream, unix::OwnedWriteHalf},
    runtime::Handle,
    sync::{Notify, mpsc},
    task::{self, JoinSet},
};

use crate::{
    SplayError,
    api::{Envelope, Reply, Request, WireError, build_id},
    repos::{RepositoryStore, data_dir},
    service,
};

/// Returns where the server for the data directory listens: what the running
/// server recorded, or else where a new server would listen. A client must
/// not derive the path from its own environment alone, because the server
/// may have started with a different one.
pub fn socket_path() -> Result<PathBuf, SplayError> {
    let recorded = fs::read_to_string(data_dir()?.join(ADDR_FILE)).unwrap_or_default();
    if !recorded.trim().is_empty() {
        return Ok(PathBuf::from(recorded.trim()));
    }
    listen_path()
}

/// Records the socket path of the server for the data directory.
const ADDR_FILE: &str = "server.addr";

/// Returns where a new server listens. Socket paths must be short, so the
/// socket lives in the runtime directory when there is one, named after the
/// data directory so that each data directory gets its own server.
fn listen_path() -> Result<PathBuf, SplayError> {
    let data_dir = data_dir()?;
    let Some(runtime_dir) = env::var_os("XDG_RUNTIME_DIR").filter(|dir| !dir.is_empty()) else {
        return Ok(data_dir.join("server.sock"));
    };
    let mut hasher = DefaultHasher::new();
    data_dir.hash(&mut hasher);
    let dir = PathBuf::from(runtime_dir).join("splay");
    fs::create_dir_all(&dir)?;
    Ok(dir.join(format!("server-{:016x}.sock", hasher.finish())))
}

/// Runs the server until a client asks it to shut down. With `background`,
/// the server's output goes to `server.log` in the data directory.
pub async fn serve(background: bool) -> Result<(), SplayError> {
    let data_dir = data_dir()?;
    // Opening the store migrates a Swarm data directory and creates the data
    // directory, which must happen before anything else is put in it.
    if !data_dir.exists() {
        drop(RepositoryStore::open().await?);
    }
    let lock_path = data_dir.join("server.lock");
    let _lock = lock(&lock_path)?;
    if background {
        redirect_output(&data_dir.join("server.log"))?;
    }
    // Keep the index database open, so that the databases stay locked
    // against every other process while the server runs.
    let _owner = RepositoryStore::open().await?;

    let socket_path = listen_path()?;
    if socket_path.exists() {
        fs::remove_file(&socket_path)?;
    }
    let listener = UnixListener::bind(&socket_path)?;
    fs::set_permissions(&socket_path, fs::Permissions::from_mode(0o600))?;
    let addr_path = data_dir.join(ADDR_FILE);
    let partial_path = data_dir.join(format!("{ADDR_FILE}.partial"));
    fs::write(&partial_path, format!("{}\n", socket_path.display()))?;
    fs::rename(&partial_path, &addr_path)?;

    let build = build_id();
    eprintln!(
        "splay server {build} (pid {}) listening on {}",
        std::process::id(),
        socket_path.display()
    );

    let shutdown = Arc::new(Notify::new());
    let mut connections = JoinSet::new();
    loop {
        tokio::select! {
            accepted = listener.accept() => {
                let (stream, _) = accepted?;
                connections.spawn(handle_connection(
                    stream,
                    socket_path.clone(),
                    addr_path.clone(),
                    shutdown.clone(),
                ));
            }
            _ = shutdown.notified() => break,
            Some(_) = connections.join_next(), if !connections.is_empty() => {}
        }
    }

    // Let requests that are already running finish.
    drop(listener);
    while connections.join_next().await.is_some() {}
    eprintln!("splay server {build} stopped");
    Ok(())
}

async fn handle_connection(
    stream: UnixStream,
    socket_path: PathBuf,
    addr_path: PathBuf,
    shutdown: Arc<Notify>,
) {
    let (read, mut write) = stream.into_split();
    let mut lines = BufReader::new(read).lines();
    let Ok(Some(line)) = lines.next_line().await else {
        return;
    };
    let envelope = match serde_json::from_str::<Envelope>(&line) {
        Ok(envelope) => envelope,
        Err(err) => {
            let error = WireError::Other(format!("invalid request: {err}"));
            let _ = send(&mut write, &Reply::Error(error)).await;
            return;
        }
    };

    let is_shutdown = matches!(envelope.request, Request::Shutdown);
    let any_build = matches!(envelope.request, Request::Shutdown | Request::Status);
    if envelope.build != build_id() && !any_build {
        let _ = send(&mut write, &Reply::BuildMismatch(build_id().to_string())).await;
        return;
    }
    if is_shutdown {
        // Remove the socket and its record before replying, so that the
        // client that asked starts a new server instead of reaching this one
        // again.
        let _ = fs::remove_file(&socket_path);
        let _ = fs::remove_file(&addr_path);
    }

    let (progress_tx, mut progress_rx) = mpsc::unbounded_channel::<String>();
    let progress = move |stage: &str| {
        let _ = progress_tx.send(stage.to_string());
    };
    // The stores run git and ssh synchronously, so give each request a
    // thread of its own rather than blocking the runtime, which must keep
    // forwarding progress and serving other requests meanwhile.
    let runtime = Handle::current();
    let request = envelope.request;
    let work = task::spawn_blocking(move || runtime.block_on(service::handle(request, &progress)));
    // The channel closes when the request finishes and drops `progress`.
    while let Some(stage) = progress_rx.recv().await {
        let _ = send(&mut write, &Reply::Progress(stage)).await;
    }
    let result = work
        .await
        .unwrap_or_else(|err| Err(SplayError::Server(format!("request failed: {err}"))));

    let reply = match result {
        Ok(value) => Reply::Ok(value),
        Err(err) => Reply::Error(WireError::from(&err)),
    };
    let _ = send(&mut write, &reply).await;

    if is_shutdown {
        shutdown.notify_one();
    }
}

async fn send(write: &mut OwnedWriteHalf, reply: &Reply) -> Result<(), SplayError> {
    let mut line = serde_json::to_vec(reply)?;
    line.push(b'\n');
    write.write_all(&line).await?;
    Ok(())
}

/// Takes the lock that makes this the only server for the data directory.
/// The lock is released when the returned file is closed, including when the
/// process dies.
fn lock(path: &Path) -> Result<File, SplayError> {
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(path)?;
    let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if rc != 0 {
        let err = io::Error::last_os_error();
        if err.raw_os_error() == Some(libc::EWOULDBLOCK) {
            return Err(SplayError::ServerRunning(path.to_path_buf()));
        }
        return Err(err.into());
    }
    Ok(file)
}

fn redirect_output(log_path: &Path) -> Result<(), SplayError> {
    let log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path)?;
    for fd in [libc::STDOUT_FILENO, libc::STDERR_FILENO] {
        if unsafe { libc::dup2(log.as_raw_fd(), fd) } == -1 {
            return Err(io::Error::last_os_error().into());
        }
    }
    Ok(())
}
