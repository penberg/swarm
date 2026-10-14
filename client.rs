//! Talks to `splay server`, starting it when it is not running and
//! restarting it when it runs a different build.

use serde::de::DeserializeOwned;
use std::{
    io,
    os::unix::process::CommandExt,
    process::Stdio,
    thread,
    time::{Duration, Instant},
};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::UnixStream,
};

use crate::{
    SplayError,
    api::{Envelope, Reply, Request, ServerStatus, build_id},
    self_command,
    server::socket_path,
};

/// How long to wait for a server to start accepting connections.
const START_TIMEOUT: Duration = Duration::from_secs(10);
/// How often to try starting a server while none accepts connections. A new
/// server cannot start until a stopping one has released its lock.
const START_INTERVAL: Duration = Duration::from_millis(500);
const CONNECT_RETRY_DELAY: Duration = Duration::from_millis(25);
/// How many times to restart a server that runs a different build, in case
/// another client keeps restarting it with its own build.
const MAX_RESTARTS: u32 = 3;

pub async fn call<T: DeserializeOwned>(request: Request) -> Result<T, SplayError> {
    call_with_progress(request, &|_| {}).await
}

/// Sends `request` and returns its reply, passing each progress update the
/// server sends to `progress`.
pub async fn call_with_progress<T: DeserializeOwned>(
    request: Request,
    progress: &(dyn Fn(&str) + Send + Sync),
) -> Result<T, SplayError> {
    let mut restarts = 0;
    loop {
        let stream = connect_or_start().await?;
        let envelope = Envelope {
            build: build_id().to_string(),
            request: request.clone(),
        };
        match exchange(stream, &envelope, progress).await? {
            Reply::Ok(value) => return Ok(serde_json::from_value(value)?),
            Reply::Error(err) => return Err(err.into()),
            Reply::BuildMismatch(server_build) => {
                if restarts == MAX_RESTARTS {
                    return Err(SplayError::ServerUnavailable(format!(
                        "the server keeps running build {server_build}, not {}",
                        build_id()
                    )));
                }
                restarts += 1;
                // Sessions run in their own processes and survive this.
                stop().await?;
            }
            Reply::Progress(_) => unreachable!("exchange returns only final replies"),
        }
    }
}

/// Returns the running server's status without starting one.
pub async fn status() -> Result<Option<ServerStatus>, SplayError> {
    let Ok(stream) = UnixStream::connect(socket_path()?).await else {
        return Ok(None);
    };
    let envelope = Envelope {
        build: build_id().to_string(),
        request: Request::Status,
    };
    match exchange(stream, &envelope, &|_| {}).await? {
        Reply::Ok(value) => Ok(Some(serde_json::from_value(value)?)),
        Reply::Error(err) => Err(err.into()),
        _ => Err(SplayError::ServerUnavailable(
            "unexpected reply".to_string(),
        )),
    }
}

/// Asks the running server to shut down once its running requests finish.
/// Returns whether a server was running.
pub async fn stop() -> Result<bool, SplayError> {
    let Ok(stream) = UnixStream::connect(socket_path()?).await else {
        return Ok(false);
    };
    let envelope = Envelope {
        build: build_id().to_string(),
        request: Request::Shutdown,
    };
    match exchange(stream, &envelope, &|_| {}).await? {
        Reply::Ok(_) => Ok(true),
        Reply::Error(err) => Err(err.into()),
        _ => Err(SplayError::ServerUnavailable(
            "unexpected reply".to_string(),
        )),
    }
}

async fn exchange(
    stream: UnixStream,
    envelope: &Envelope,
    progress: &(dyn Fn(&str) + Send + Sync),
) -> Result<Reply, SplayError> {
    let (read, mut write) = stream.into_split();
    let mut line = serde_json::to_vec(envelope)?;
    line.push(b'\n');
    write.write_all(&line).await?;

    let mut lines = BufReader::new(read).lines();
    while let Some(line) = lines.next_line().await? {
        match serde_json::from_str::<Reply>(&line)? {
            Reply::Progress(stage) => progress(&stage),
            reply => return Ok(reply),
        }
    }

    Err(SplayError::ServerUnavailable(
        "the server closed the connection without replying".to_string(),
    ))
}

async fn connect_or_start() -> Result<UnixStream, SplayError> {
    let deadline = Instant::now() + START_TIMEOUT;
    let mut started_at: Option<Instant> = None;
    loop {
        // Look the path up every time: a server that has just started may
        // listen somewhere else than the one before it.
        let path = socket_path()?;
        match UnixStream::connect(&path).await {
            Ok(stream) => return Ok(stream),
            Err(err) if Instant::now() >= deadline => {
                return Err(SplayError::ServerUnavailable(format!(
                    "cannot connect to {}: {err}; see server.log next to it",
                    path.display()
                )));
            }
            Err(_) => {
                if started_at.is_none_or(|at| at.elapsed() >= START_INTERVAL) {
                    start()?;
                    started_at = Some(Instant::now());
                }
                tokio::time::sleep(CONNECT_RETRY_DELAY).await;
            }
        }
    }
}

/// Starts a detached server. If another one is already starting, this one
/// fails to take the lock and exits.
fn start() -> Result<(), SplayError> {
    let mut server = self_command();
    server
        .args(["server", "--background"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    // Detach the server so that it outlives the client that started it.
    unsafe {
        server.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut child = server.spawn()?;
    // Reap the server when it exits, so a long-running client such as the
    // UI does not collect zombies.
    thread::spawn(move || child.wait());
    Ok(())
}
