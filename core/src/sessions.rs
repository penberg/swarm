use crossterm::terminal::{disable_raw_mode, enable_raw_mode};
use serde::Serialize;
use std::{
    env,
    ffi::CString,
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    os::{
        fd::{AsRawFd, RawFd},
        unix::net::{UnixListener, UnixStream},
    },
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use turso::{Builder, Connection, Row};

use crate::{
    SwarmError, database_error,
    repos::{Repository, RepositoryStore},
    toolchains,
    workspaces::{Workspace, WorkspaceStore, migrate_repo_db},
};

const SESSION_STATUS_STARTING: &str = "starting";
const SESSION_STATUS_RUNNING: &str = "running";
const SESSION_STATUS_EXITED: &str = "exited";
const SESSION_STATUS_FAILED: &str = "failed";
const SESSION_STATUS_STOPPED: &str = "stopped";
const ATTACH_DETACH_BYTE: u8 = 0x1d;

#[derive(Debug, Clone, Serialize)]
pub struct Session {
    pub id: String,
    pub repository: String,
    pub repository_alias: String,
    pub workspace: String,
    pub command: Vec<String>,
    pub path: PathBuf,
    pub log_path: PathBuf,
    pub socket_path: PathBuf,
    pub pid: Option<u32>,
    pub status: String,
    pub exit_code: Option<i32>,
    pub created_at: i64,
    pub stopped_at: Option<i64>,
}

pub struct SessionStore {
    repos: RepositoryStore,
    workspaces: WorkspaceStore,
}

pub fn default_session_command() -> Vec<String> {
    let shell = env::var("SHELL").unwrap_or_else(|_| "/bin/bash".to_string());
    vec![shell, "-l".to_string()]
}

impl SessionStore {
    pub async fn open() -> Result<Self, SwarmError> {
        Ok(Self {
            repos: RepositoryStore::open().await?,
            workspaces: WorkspaceStore::open().await?,
        })
    }

    pub async fn create(
        &self,
        workspace_ref: &str,
        command: &[String],
    ) -> Result<Session, SwarmError> {
        if command.is_empty() {
            return Err(SwarmError::InvalidSession("missing command".to_string()));
        }

        let (repo, workspace) = self.workspaces.resolve_reference(workspace_ref).await?;
        let db = self.open_repo_db(&repo).await?;
        let session_id = new_session_id();
        let session = self.prepare_session(&repo, &workspace, &session_id, command)?;

        let repo_db_path = self.repos.repo_db_path(&repo);
        db.execute(
            "INSERT INTO sessions (id, workspace_name, command_json, path, log_path, socket_path, pid, status, exit_code, created_at, stopped_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            (
                session.id.as_str(),
                session.workspace.as_str(),
                serde_json::to_string(command)?,
                path_to_string(&session.path)?,
                path_to_string(&session.log_path)?,
                path_to_string(&session.socket_path)?,
                Option::<i64>::None,
                SESSION_STATUS_STARTING,
                Option::<i64>::None,
                session.created_at,
                Option::<i64>::None,
            ),
        )
        .await
        .map_err(|err| database_error(&repo_db_path, "insert session", err))?;

        drop(db);
        self.spawn_supervisor(&repo, &workspace, &session, command)?;
        self.wait_until_ready(&repo, &session).await?;
        self.mark_session_running(&repo, &session).await?;

        self.info(&session.id).await
    }

    pub async fn list(&self, workspace_ref: Option<&str>) -> Result<Vec<Session>, SwarmError> {
        if let Some(workspace_ref) = workspace_ref {
            let (repo, workspace) = self.workspaces.resolve_reference(workspace_ref).await?;
            let db = self.open_repo_db(&repo).await?;
            return self
                .list_repo_sessions(&db, &repo, Some(workspace.name.as_str()))
                .await;
        }

        let repos = self.repos.list().await?;
        let mut sessions = Vec::new();

        for repo in repos {
            let db = self.open_repo_db(&repo).await?;
            sessions.extend(self.list_repo_sessions(&db, &repo, None).await?);
        }

        sessions.sort_by(|left, right| left.created_at.cmp(&right.created_at));
        Ok(sessions)
    }

    pub async fn info(&self, session_id: &str) -> Result<Session, SwarmError> {
        let (repo, mut session, db) = self.find_session(session_id).await?;
        self.refresh_session_status(&db, &repo, &mut session)
            .await?;
        Ok(session)
    }

    pub async fn stop(&self, session_id: &str) -> Result<Session, SwarmError> {
        let (_repo, mut session, db) = self.find_session(session_id).await?;

        if matches!(
            session.status.as_str(),
            SESSION_STATUS_RUNNING | SESSION_STATUS_STARTING
        ) {
            if let Some(pid) = session.pid {
                terminate_process(pid)?;
            }

            session.status = SESSION_STATUS_STOPPED.to_string();
            session.stopped_at = Some(unix_timestamp());
            session.pid = None;

            db.execute(
                "UPDATE sessions
                 SET pid = NULL, status = ?2, stopped_at = ?3
                 WHERE id = ?1",
                (
                    session.id.as_str(),
                    session.status.as_str(),
                    session.stopped_at,
                ),
            )
            .await?;
        }

        Ok(session)
    }

    pub async fn remove(&self, session_id: &str) -> Result<Session, SwarmError> {
        let (repo, mut session, db) = self.find_session(session_id).await?;
        self.refresh_session_status(&db, &repo, &mut session)
            .await?;

        if matches!(
            session.status.as_str(),
            SESSION_STATUS_RUNNING | SESSION_STATUS_STARTING
        ) {
            return Err(SwarmError::SessionRunning(session.id.clone()));
        }

        db.execute("DELETE FROM sessions WHERE id = ?1", [session.id.as_str()])
            .await?;

        if session.path.exists() {
            fs::remove_dir_all(&session.path)?;
        }

        Ok(session)
    }

    pub async fn prune_terminal_sessions(&self) -> Result<usize, SwarmError> {
        let sessions = self.list(None).await?;
        let mut pruned = 0;

        for session in sessions {
            if matches!(
                session.status.as_str(),
                SESSION_STATUS_STOPPED | SESSION_STATUS_FAILED | SESSION_STATUS_EXITED
            ) {
                self.remove(&session.id).await?;
                pruned += 1;
            }
        }

        Ok(pruned)
    }

    pub async fn attach(&self, session_id: &str, _follow: bool) -> Result<(), SwarmError> {
        let (repo, mut session, db) = self.find_session(session_id).await?;
        self.refresh_session_status(&db, &repo, &mut session)
            .await?;

        if session.status == SESSION_STATUS_RUNNING && session.socket_path.exists() {
            attach_to_socket(&session.socket_path)?;
            return Ok(());
        }

        print_file(&session.log_path)
    }

    fn prepare_session(
        &self,
        repo: &Repository,
        workspace: &Workspace,
        session_id: &str,
        command: &[String],
    ) -> Result<Session, SwarmError> {
        let sessions_dir = self.repos.sessions_dir(repo);
        let session_path = sessions_dir.join(session_id);
        let log_path = session_path.join("log");
        let socket_path = session_path.join("sock");
        let pid_path = session_path.join("child.pid");
        let ready_path = session_path.join("ready");
        let supervisor_log_path = session_path.join("supervisor.log");

        fs::create_dir_all(&session_path)?;
        File::create(&log_path)?;
        File::create(&pid_path)?;
        fs::remove_file(&pid_path)?;
        File::create(&ready_path)?;
        fs::remove_file(&ready_path)?;
        File::create(&supervisor_log_path)?;

        Ok(Session {
            id: session_id.to_string(),
            repository: repo.canonical(),
            repository_alias: repo.alias.clone().unwrap_or_else(|| repo.name.clone()),
            workspace: workspace.name.clone(),
            command: command.to_vec(),
            path: session_path,
            log_path,
            socket_path,
            pid: None,
            status: SESSION_STATUS_STARTING.to_string(),
            exit_code: None,
            created_at: unix_timestamp(),
            stopped_at: None,
        })
    }

    fn spawn_supervisor(
        &self,
        repo: &Repository,
        workspace: &Workspace,
        session: &Session,
        command: &[String],
    ) -> Result<(), SwarmError> {
        let exe = supervisor_executable()?;
        let supervisor_log_path = session.path.join("supervisor.log");
        let supervisor_log = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&supervisor_log_path)?;
        let supervisor_err = supervisor_log.try_clone()?;
        let mut child = Command::new(exe);
        child
            .arg("session")
            .arg("serve")
            .arg(repo.canonical())
            .arg(self.repos.repo_db_path(repo).display().to_string())
            .arg(&workspace.name)
            .arg(&session.id)
            .arg(session.path.display().to_string())
            .arg(workspace.path.display().to_string())
            .arg("--")
            .args(command)
            .stdin(Stdio::null())
            .stdout(Stdio::from(supervisor_log))
            .stderr(Stdio::from(supervisor_err));

        child.spawn()?;
        Ok(())
    }

    async fn wait_until_ready(
        &self,
        repo: &Repository,
        session: &Session,
    ) -> Result<(), SwarmError> {
        let deadline = Instant::now() + Duration::from_secs(3);
        let ready_path = session.path.join("ready");
        loop {
            if ready_path.exists() {
                return Ok(());
            }

            if Instant::now() >= deadline {
                self.mark_session_failed(repo, &session.id).await?;
                let detail = read_supervisor_log(&session.path);
                return Err(SwarmError::InvalidSession(format!(
                    "session `{}` did not become ready{}",
                    session.id, detail
                )));
            }

            std::thread::sleep(Duration::from_millis(50));
        }
    }

    async fn find_session(
        &self,
        session_id: &str,
    ) -> Result<(Repository, Session, Connection), SwarmError> {
        let repos = self.repos.list().await?;

        for repo in repos {
            let db = self.open_repo_db(&repo).await?;
            if let Some(session) = self.find_repo_session(&db, &repo, session_id).await? {
                return Ok((repo, session, db));
            }
        }

        Err(SwarmError::SessionNotFound(session_id.to_string()))
    }

    async fn list_repo_sessions(
        &self,
        db: &Connection,
        repo: &Repository,
        workspace_name: Option<&str>,
    ) -> Result<Vec<Session>, SwarmError> {
        let sql = if workspace_name.is_some() {
            "SELECT id, workspace_name, command_json, path, log_path, socket_path, pid, status, exit_code, created_at, stopped_at
             FROM sessions
             WHERE workspace_name = ?1
             ORDER BY created_at, id"
        } else {
            "SELECT id, workspace_name, command_json, path, log_path, socket_path, pid, status, exit_code, created_at, stopped_at
             FROM sessions
             ORDER BY created_at, id"
        };
        let mut stmt = db.prepare(sql).await?;
        let mut sessions = Vec::new();

        if let Some(workspace_name) = workspace_name {
            let mut rows = stmt.query([workspace_name]).await?;
            while let Some(row) = rows.next().await? {
                let mut session = session_from_row(repo, &row)?;
                self.refresh_session_status(db, &repo, &mut session).await?;
                sessions.push(session);
            }
        } else {
            let mut rows = stmt.query(()).await?;
            while let Some(row) = rows.next().await? {
                let mut session = session_from_row(repo, &row)?;
                self.refresh_session_status(db, &repo, &mut session).await?;
                sessions.push(session);
            }
        }

        Ok(sessions)
    }

    async fn find_repo_session(
        &self,
        db: &Connection,
        repo: &Repository,
        session_id: &str,
    ) -> Result<Option<Session>, SwarmError> {
        let mut stmt = db
            .prepare(
                "SELECT id, workspace_name, command_json, path, log_path, socket_path, pid, status, exit_code, created_at, stopped_at
                 FROM sessions
                 WHERE id = ?1
                 LIMIT 1",
            )
            .await?;
        let mut rows = stmt.query([session_id]).await?;

        if let Some(row) = rows.next().await? {
            return Ok(Some(session_from_row(repo, &row)?));
        }

        Ok(None)
    }

    async fn mark_session_failed(
        &self,
        repo: &Repository,
        session_id: &str,
    ) -> Result<(), SwarmError> {
        let db = self.open_repo_db(repo).await?;
        db.execute(
            "UPDATE sessions
             SET status = ?2, stopped_at = ?3
             WHERE id = ?1 AND status = ?4",
            (
                session_id,
                SESSION_STATUS_FAILED,
                Some(unix_timestamp()),
                SESSION_STATUS_STARTING,
            ),
        )
        .await?;
        Ok(())
    }

    async fn mark_session_running(
        &self,
        repo: &Repository,
        session: &Session,
    ) -> Result<(), SwarmError> {
        let pid_path = session.path.join("child.pid");
        let pid = fs::read_to_string(&pid_path)
            .ok()
            .and_then(|value| value.trim().parse::<i64>().ok());
        let db = self.open_repo_db(repo).await?;
        let repo_db_path = self.repos.repo_db_path(repo);
        db.execute(
            "UPDATE sessions
             SET pid = ?2, status = ?3, socket_path = ?4
             WHERE id = ?1",
            (
                session.id.as_str(),
                pid,
                SESSION_STATUS_RUNNING,
                path_to_string(&session.socket_path)?,
            ),
        )
        .await
        .map_err(|err| database_error(&repo_db_path, "mark session running", err))?;
        Ok(())
    }

    async fn refresh_session_status(
        &self,
        db: &Connection,
        _repo: &Repository,
        session: &mut Session,
    ) -> Result<(), SwarmError> {
        if !matches!(
            session.status.as_str(),
            SESSION_STATUS_RUNNING | SESSION_STATUS_STARTING
        ) {
            return Ok(());
        }

        let Some(pid) = session.pid else {
            return Ok(());
        };

        if process_exists(pid)? {
            return Ok(());
        }

        session.status = SESSION_STATUS_EXITED.to_string();
        session.pid = None;
        session.stopped_at = Some(session.stopped_at.unwrap_or_else(unix_timestamp));

        db.execute(
            "UPDATE sessions
             SET pid = NULL, status = ?2, stopped_at = ?3
             WHERE id = ?1",
            (
                session.id.as_str(),
                session.status.as_str(),
                session.stopped_at,
            ),
        )
        .await?;

        Ok(())
    }

    async fn open_repo_db(&self, repo: &Repository) -> Result<Connection, SwarmError> {
        let repo_db_path = self.repos.repo_db_path(repo);
        let db = Builder::new_local(path_to_string(&repo_db_path)?)
            .build()
            .await
            .map_err(|err| database_error(&repo_db_path, "open", err))?;
        let conn = db
            .connect()
            .map_err(|err| database_error(&repo_db_path, "connect", err))?;
        migrate_repo_db(&conn, &repo_db_path).await?;
        Ok(conn)
    }
}

pub async fn serve_runtime(
    repository: &str,
    repo_db_path: &str,
    workspace: &str,
    session_id: &str,
    session_dir: &str,
    workspace_dir: &str,
    command: &[String],
) -> Result<(), SwarmError> {
    if command.is_empty() {
        return Err(SwarmError::InvalidSession("missing command".to_string()));
    }

    let session_path = PathBuf::from(session_dir);
    let socket_path = session_path.join("sock");
    let log_path = session_path.join("log");
    let pid_path = session_path.join("child.pid");
    let meta_path = session_path.join("meta.toml");
    let ready_path = session_path.join("ready");
    let workspace_path = PathBuf::from(workspace_dir);
    let repo_db_path = PathBuf::from(repo_db_path);

    fs::create_dir_all(&session_path)?;
    if socket_path.exists() {
        fs::remove_file(&socket_path)?;
    }
    if ready_path.exists() {
        fs::remove_file(&ready_path)?;
    }
    if pid_path.exists() {
        fs::remove_file(&pid_path)?;
    }

    let listener = UnixListener::bind(&socket_path)?;
    listener.set_nonblocking(true)?;
    let repo_dir = workspace_path
        .parent()
        .and_then(|path| path.parent())
        .ok_or(SwarmError::PathResolution)?;
    let (pty, child_pid) = spawn_pty_child(
        &workspace_path,
        &toolchains::session_env(repo_dir, &workspace_path),
        command,
    )?;

    fs::write(
        &meta_path,
        render_session_meta_toml(
            repository,
            workspace,
            session_id,
            command,
            &log_path,
            &socket_path,
            child_pid,
        ),
    )?;
    fs::write(&pid_path, format!("{child_pid}\n"))?;
    File::create(&ready_path)?;

    session_event_loop(
        &repo_db_path,
        session_id,
        child_pid,
        &pty,
        listener,
        &log_path,
    )
    .await
}

fn session_from_row(repo: &Repository, row: &Row) -> Result<Session, SwarmError> {
    Ok(Session {
        id: row.get::<String>(0)?,
        repository: repo.canonical(),
        repository_alias: repo.alias.clone().unwrap_or_else(|| repo.name.clone()),
        workspace: row.get::<String>(1)?,
        command: serde_json::from_str(&row.get::<String>(2)?)?,
        path: PathBuf::from(row.get::<String>(3)?),
        log_path: PathBuf::from(row.get::<String>(4)?),
        socket_path: PathBuf::from(row.get::<String>(5)?),
        pid: row.get::<Option<i64>>(6)?.map(|pid| pid as u32),
        status: row.get::<String>(7)?,
        exit_code: row.get::<Option<i64>>(8)?.map(|code| code as i32),
        created_at: row.get::<i64>(9)?,
        stopped_at: row.get::<Option<i64>>(10)?,
    })
}

fn render_session_meta_toml(
    repository: &str,
    workspace: &str,
    session_id: &str,
    command: &[String],
    log_path: &Path,
    socket_path: &Path,
    pid: u32,
) -> String {
    let mut out = String::new();
    out.push_str(&format!("id = {:?}\n", session_id));
    out.push_str(&format!("repository = {:?}\n", repository));
    out.push_str(&format!("workspace = {:?}\n", workspace));
    out.push_str(&format!("pid = {}\n", pid));
    out.push_str(&format!("log = {:?}\n", log_path.display().to_string()));
    out.push_str(&format!(
        "socket = {:?}\n",
        socket_path.display().to_string()
    ));
    out.push_str(&format!("command = {:?}\n", command));
    out
}

async fn session_event_loop(
    repo_db_path: &Path,
    session_id: &str,
    child_pid: u32,
    pty: &Pty,
    listener: UnixListener,
    log_path: &Path,
) -> Result<(), SwarmError> {
    let listener_fd = listener.as_raw_fd();
    let master_fd = pty.master_fd;
    let mut clients: Vec<UnixStream> = Vec::new();
    let mut log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path)?;
    loop {
        let polled_client_count = clients.len();
        let mut poll_fds = Vec::with_capacity(2 + clients.len());
        poll_fds.push(libc::pollfd {
            fd: listener_fd,
            events: libc::POLLIN,
            revents: 0,
        });
        poll_fds.push(libc::pollfd {
            fd: master_fd,
            events: libc::POLLIN,
            revents: 0,
        });

        for client in &clients {
            poll_fds.push(libc::pollfd {
                fd: client.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            });
        }

        let rc = unsafe { libc::poll(poll_fds.as_mut_ptr(), poll_fds.len() as libc::nfds_t, 100) };
        if rc < 0 {
            let err = io::Error::last_os_error();
            if err.kind() != io::ErrorKind::Interrupted {
                return Err(err.into());
            }
        }

        if poll_fds[1].revents & libc::POLLIN != 0 {
            let mut buffer = [0_u8; 4096];
            let read_len =
                unsafe { libc::read(master_fd, buffer.as_mut_ptr().cast(), buffer.len()) };
            if read_len > 0 {
                let chunk = &buffer[..read_len as usize];
                log.write_all(chunk)?;
                log.flush()?;

                let mut alive = Vec::with_capacity(clients.len());
                for mut client in clients.drain(..) {
                    if client.write_all(chunk).is_ok() {
                        alive.push(client);
                    }
                }
                clients = alive;
            }
        }

        for index in (0..polled_client_count).rev() {
            let revents = poll_fds[index + 2].revents;
            if revents & libc::POLLIN == 0 {
                continue;
            }

            let mut buffer = [0_u8; 4096];
            match clients[index].read(&mut buffer) {
                Ok(0) => {
                    clients.remove(index);
                }
                Ok(len) => {
                    let _ = write_fd(master_fd, &buffer[..len]);
                }
                Err(err) if err.kind() == io::ErrorKind::WouldBlock => {}
                Err(_) => {
                    clients.remove(index);
                }
            }
        }

        if poll_fds[0].revents & libc::POLLIN != 0 {
            while let Ok((stream, _)) = listener.accept() {
                stream.set_nonblocking(true)?;
                clients.push(stream);
            }
        }

        if let Some(status) = reap_process(child_pid)? {
            let exit_code = decode_wait_status(status);
            let db = open_repo_db_at_path(repo_db_path).await?;
            db.execute(
                "UPDATE sessions
                 SET pid = NULL, status = ?2, exit_code = ?3, stopped_at = ?4
                 WHERE id = ?1",
                (
                    session_id,
                    SESSION_STATUS_EXITED,
                    exit_code.map(|code| code as i64),
                    Some(unix_timestamp()),
                ),
            )
            .await
            .map_err(|err| database_error(repo_db_path, "mark session exited", err))?;

            return Ok(());
        }
    }
}

async fn open_repo_db_at_path(path: &Path) -> Result<Connection, SwarmError> {
    let db = Builder::new_local(path_to_string(path)?)
        .build()
        .await
        .map_err(|err| database_error(path, "open", err))?;
    let conn = db
        .connect()
        .map_err(|err| database_error(path, "connect", err))?;
    migrate_repo_db(&conn, path).await?;
    Ok(conn)
}

fn supervisor_executable() -> Result<PathBuf, SwarmError> {
    let exe = env::current_exe()?;
    let candidate = exe.with_file_name("swarmctl");

    if candidate.exists() {
        return Ok(candidate);
    }

    Ok(exe)
}

fn attach_to_socket(socket_path: &Path) -> Result<(), SwarmError> {
    let mut socket = UnixStream::connect(socket_path)?;
    let socket_fd = socket.as_raw_fd();
    let stdin_fd = io::stdin().as_raw_fd();
    let stdout = io::stdout();
    let mut stdout = stdout.lock();
    let mut last_output_byte = None;

    enable_raw_mode()?;
    let _ = stdout.write_all(b"[attached, press Ctrl-] to detach]\r\n");
    let _ = stdout.flush();
    let result = (|| -> Result<(), SwarmError> {
        loop {
            let mut poll_fds = [
                libc::pollfd {
                    fd: stdin_fd,
                    events: libc::POLLIN,
                    revents: 0,
                },
                libc::pollfd {
                    fd: socket_fd,
                    events: libc::POLLIN,
                    revents: 0,
                },
            ];

            let rc =
                unsafe { libc::poll(poll_fds.as_mut_ptr(), poll_fds.len() as libc::nfds_t, -1) };
            if rc < 0 {
                let err = io::Error::last_os_error();
                if err.kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(err.into());
            }

            if poll_fds[0].revents & libc::POLLIN != 0 {
                let mut buffer = [0_u8; 4096];
                let len = unsafe { libc::read(stdin_fd, buffer.as_mut_ptr().cast(), buffer.len()) };
                if len <= 0 {
                    break;
                }

                let input = &buffer[..len as usize];
                if let Some(position) = input.iter().position(|byte| *byte == ATTACH_DETACH_BYTE) {
                    if position > 0 {
                        socket.write_all(&input[..position])?;
                    }
                    break;
                }

                socket.write_all(input)?;
            }

            if poll_fds[1].revents & libc::POLLIN != 0 {
                let mut buffer = [0_u8; 4096];
                let len = socket.read(&mut buffer)?;
                if len == 0 {
                    break;
                }
                stdout.write_all(&buffer[..len])?;
                stdout.flush()?;
                last_output_byte = buffer[..len].last().copied();
            }
        }

        Ok(())
    })();
    let _ = disable_raw_mode();

    if !matches!(last_output_byte, Some(b'\n' | b'\r')) {
        let _ = stdout.write_all(b"\r\n");
        let _ = stdout.flush();
    }

    result
}

fn print_file(path: &Path) -> Result<(), SwarmError> {
    let mut file = File::open(path)?;
    let mut stdout = io::stdout();
    io::copy(&mut file, &mut stdout)?;
    stdout.flush()?;
    Ok(())
}

fn terminate_process(pid: u32) -> Result<(), SwarmError> {
    let rc = unsafe { libc::kill(pid as i32, libc::SIGTERM) };
    if rc == 0 || io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH) {
        return Ok(());
    }

    Err(io::Error::last_os_error().into())
}

fn process_exists(pid: u32) -> Result<bool, SwarmError> {
    let rc = unsafe { libc::kill(pid as i32, 0) };
    if rc == 0 {
        return Ok(true);
    }

    match io::Error::last_os_error().raw_os_error() {
        Some(libc::ESRCH) => Ok(false),
        Some(libc::EPERM) => Ok(true),
        _ => Err(io::Error::last_os_error().into()),
    }
}

fn reap_process(pid: u32) -> Result<Option<i32>, SwarmError> {
    let mut status = 0_i32;
    let rc = unsafe { libc::waitpid(pid as i32, &mut status, libc::WNOHANG) };
    if rc == 0 {
        return Ok(None);
    }
    if rc < 0 {
        let err = io::Error::last_os_error();
        if err.raw_os_error() == Some(libc::ECHILD) {
            return Ok(Some(0));
        }
        return Err(err.into());
    }
    Ok(Some(status))
}

fn decode_wait_status(status: i32) -> Option<i32> {
    if libc::WIFEXITED(status) {
        return Some(libc::WEXITSTATUS(status));
    }

    if libc::WIFSIGNALED(status) {
        return Some(128 + libc::WTERMSIG(status));
    }

    None
}

fn write_fd(fd: RawFd, mut bytes: &[u8]) -> io::Result<()> {
    while !bytes.is_empty() {
        let written = unsafe { libc::write(fd, bytes.as_ptr().cast(), bytes.len()) };
        if written < 0 {
            let err = io::Error::last_os_error();
            if err.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(err);
        }
        bytes = &bytes[written as usize..];
    }

    Ok(())
}

struct Pty {
    master_fd: RawFd,
}

impl Drop for Pty {
    fn drop(&mut self) {
        if self.master_fd >= 0 {
            unsafe {
                libc::close(self.master_fd);
            }
        }
    }
}

fn spawn_pty_child(
    cwd: &Path,
    session_env: &[toolchains::SessionEnvVar],
    command: &[String],
) -> Result<(Pty, u32), SwarmError> {
    let mut winsize = libc::winsize {
        ws_row: 24,
        ws_col: 80,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    let stdout_fd = io::stdout().as_raw_fd();
    let rc = unsafe { libc::ioctl(stdout_fd, libc::TIOCGWINSZ, &mut winsize) };
    if rc != 0 {
        winsize = libc::winsize {
            ws_row: 24,
            ws_col: 80,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
    }

    let mut master_fd = -1;
    let pid = unsafe {
        libc::forkpty(
            &mut master_fd,
            std::ptr::null_mut(),
            std::ptr::null(),
            &winsize,
        )
    };
    if pid < 0 {
        return Err(io::Error::last_os_error().into());
    }

    if pid == 0 {
        if let Ok(dir) = CString::new(cwd.as_os_str().as_encoded_bytes()) {
            unsafe {
                libc::chdir(dir.as_ptr());
            }
        }

        if let Some(prompt) = workspace_prompt(cwd) {
            if let (Ok(name), Ok(value)) = (CString::new("PS1"), CString::new(prompt)) {
                unsafe {
                    libc::setenv(name.as_ptr(), value.as_ptr(), 1);
                }
            }
        }

        for env_var in session_env {
            if let (Ok(key), Ok(value)) = (
                CString::new(env_var.key),
                CString::new(env_var.value.as_slice()),
            ) {
                unsafe {
                    libc::setenv(key.as_ptr(), value.as_ptr(), i32::from(env_var.overwrite));
                }
            }
        }

        let mut cstrings = Vec::with_capacity(command.len());
        for arg in command {
            match CString::new(arg.as_str()) {
                Ok(value) => cstrings.push(value),
                Err(_) => unsafe { libc::_exit(127) },
            }
        }

        let mut argv: Vec<*const libc::c_char> = cstrings.iter().map(|arg| arg.as_ptr()).collect();
        argv.push(std::ptr::null());

        unsafe {
            libc::execvp(cstrings[0].as_ptr(), argv.as_ptr());
            libc::_exit(127);
        }
    }

    if master_fd < 0 {
        return Err(SwarmError::InvalidSession(
            "forkpty did not return a master fd".to_string(),
        ));
    }

    Ok((Pty { master_fd }, pid as u32))
}

fn workspace_prompt(cwd: &Path) -> Option<String> {
    let workspace = cwd.file_name()?.to_str()?;
    Some(format!("{workspace}> "))
}

fn new_session_id() -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    format!("s{:x}{:x}", now.as_secs(), now.subsec_millis())
}

fn path_to_string(path: &Path) -> Result<&str, SwarmError> {
    path.to_str().ok_or(SwarmError::PathResolution)
}

fn unix_timestamp() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

fn read_supervisor_log(session_path: &Path) -> String {
    let log_path = session_path.join("supervisor.log");
    match fs::read_to_string(log_path) {
        Ok(contents) => {
            let trimmed = contents.trim();
            if trimmed.is_empty() {
                String::new()
            } else {
                format!(": {}", trimmed.lines().last().unwrap_or(trimmed))
            }
        }
        Err(_) => String::new(),
    }
}
