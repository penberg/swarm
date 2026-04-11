use gtk::{
    gdk,
    gio::{self, FileMonitor, FileMonitorFlags},
    glib,
    prelude::*,
    Align, Application, ApplicationWindow, Box as GtkBox, Button, CssProvider, Entry,
    EventControllerKey, EventControllerMotion, Grid, Image, Label, ListBox, ListBoxRow,
    Orientation, PolicyType, PropagationPhase, ScrolledWindow, SelectionMode, Spinner, Widget,
    STYLE_PROVIDER_PRIORITY_APPLICATION,
};
use std::{
    cell::{Cell, RefCell},
    collections::{HashMap, HashSet},
    path::Path,
    rc::Rc,
    sync::mpsc,
    time::{Duration, Instant},
};
use swarm::forges::github::{self, PullRequestStatus, PullRequestStatusState};

use crate::{
    data::{
        add_repository, clone_workspace, collapse_repository, create_workspace,
        current_workspace_branch, current_workspace_head, expand_repository, load_workspace_groups,
        remove_workspace, rename_workspace, sync_repository, workspace_head_path, WorkspaceEntry,
        WorkspaceGroup,
    },
    workspace_panel::DetailWidgets,
};

const APP_ID: &str = "com.penberg.swarm.ui";
const MAX_PENDING_PR_LOOKUPS: usize = 2;
const SELECTED_PR_STATUS_TTL: Duration = Duration::from_secs(5);
const PENDING_PR_STATUS_TTL: Duration = Duration::from_secs(15);
const SETTLED_PR_STATUS_TTL: Duration = Duration::from_secs(180);
const MERGED_PR_STATUS_TTL: Duration = Duration::from_secs(300);
const NO_PR_STATUS_TTL: Duration = Duration::from_secs(300);
const LOADING_PR_STATUS_TTL: Duration = Duration::from_secs(10);
const STYLE: &str = r#"
window {
  background: #0f1115;
}

.app-shell {
  background: linear-gradient(180deg, #11151c 0%, #0d1016 100%);
}

.sidebar {
  background: #0a0d12;
  border-right: 1px solid rgba(255, 255, 255, 0.08);
  padding: 14px 10px;
}

.sidebar-toolbar {
  margin-bottom: 10px;
}

.sidebar-primary-action {
  min-height: 28px;
  padding: 0 10px;
  background: transparent;
  color: #cfd8e4;
  border: 1px solid rgba(255, 255, 255, 0.08);
  border-radius: 9px;
  font-size: 12px;
  font-weight: 700;
}

.sidebar-primary-action:hover {
  background: rgba(255, 255, 255, 0.04);
  color: #f3f7fb;
}

.repo-compose {
  margin-bottom: 12px;
  padding: 2px 0 4px 0;
}

.repo-compose-entry {
  min-height: 32px;
  padding: 0 9px;
  background: rgba(255, 255, 255, 0.025);
  color: #eef4fb;
  border: 1px solid rgba(255, 255, 255, 0.08);
  border-radius: 9px;
}

.repo-compose-entry text {
  color: #eef4fb;
}

.repo-compose-error {
  color: #ff9b9b;
  font-size: 11px;
  font-weight: 600;
}

.repo-compose-submit {
  min-height: 26px;
  padding: 0 10px;
  background: transparent;
  color: #eef7ff;
  border: 1px solid rgba(255, 255, 255, 0.12);
  border-radius: 9px;
  font-size: 11px;
  font-weight: 700;
}

.repo-compose-submit:hover {
  background: rgba(255, 255, 255, 0.05);
}

.repo-compose-cancel {
  min-height: 26px;
  padding: 0 10px;
  background: transparent;
  color: #7f8b9d;
  border: none;
  border-radius: 9px;
  font-size: 11px;
  font-weight: 700;
}

.repo-compose-cancel:hover {
  background: rgba(255, 255, 255, 0.05);
  color: #f3f5f7;
}

.content {
  padding: 6px 10px;
}

.repo-row {
  margin-top: 14px;
  margin-bottom: 8px;
}

.repo-add {
  min-width: 24px;
  min-height: 24px;
  padding: 0;
  background: transparent;
  border: none;
  border-radius: 8px;
  color: #7a8698;
  font-size: 16px;
  font-weight: 500;
}

.repo-toggle {
  min-width: 24px;
  min-height: 24px;
  padding: 0;
  background: transparent;
  border: none;
  border-radius: 8px;
  color: #7a8698;
}

.repo-toggle:hover {
  background: rgba(126, 203, 255, 0.08);
  color: #e7f3ff;
}

.repo-add:hover {
  background: rgba(126, 203, 255, 0.08);
  color: #e7f3ff;
}

.repo-sync {
  min-width: 24px;
  min-height: 24px;
  padding: 0;
  background: transparent;
  border: none;
  border-radius: 8px;
  color: #7a8698;
}

.repo-sync:hover {
  background: rgba(126, 203, 255, 0.08);
  color: #e7f3ff;
}

.repo-sync:disabled {
  color: #a9d8ff;
  opacity: 1;
}

.repo-header {
  color: #566172;
  font-size: 10px;
  font-weight: 800;
  letter-spacing: 0.09em;
}

.repo-status {
  color: #6f7887;
  font-size: 10px;
  font-weight: 700;
  letter-spacing: 0.02em;
  opacity: 0;
}

.repo-row-hover .repo-status {
  opacity: 1;
}

.workspace-list {
  background: transparent;
}

.workspace-row {
  background: transparent;
  border-radius: 10px;
}

.workspace-row:selected,
.workspace-row:selected:focus {
  background: rgba(126, 203, 255, 0.10);
}

.workspace-card {
  padding: 9px 10px 9px 0;
}

.workspace-action-slot-end {
  min-width: 20px;
}

.workspace-status-slot {
  min-width: 8px;
}

.workspace-pr-indicator {
  min-width: 8px;
  min-height: 8px;
  border-radius: 999px;
  background: #6f7887;
}

.workspace-pr-success {
  background: #57c26c;
}

.workspace-pr-pending {
  background: #d8a341;
}

.workspace-pr-failure {
  background: #e06767;
}

.workspace-pr-merged {
  background: #a371f7;
}

.workspace-action {
  min-width: 18px;
  min-height: 18px;
  padding: 0;
  background: transparent;
  border: none;
  border-radius: 6px;
  color: #7a8698;
}

.workspace-delete {
}

.workspace-delete:hover {
  background: rgba(255, 133, 133, 0.16);
  color: #ffd9d9;
}

.workspace-clone {
}

.workspace-clone:hover {
  background: rgba(126, 203, 255, 0.08);
  color: #e7f3ff;
}

.workspace-name {
  color: #f3f5f7;
  font-size: 13px;
  font-weight: 700;
}

.workspace-meta {
  color: #6f7887;
  font-size: 11px;
}

.workspace-name-entry {
  color: #6f7887;
  font-size: 11px;
  font-weight: 400;
  padding: 0;
  background: transparent;
  border: none;
  box-shadow: none;
}

.session-toolbar {
  margin-bottom: 6px;
}

.session-toolbar-spacer {
  min-width: 0;
}

.session-tabs {
  spacing: 0;
}

.session-pr-link {
  color: #8fcaff;
  font-size: 11px;
  font-weight: 700;
  text-decoration: none;
}

.session-pr-link:hover {
  color: #d7eeff;
}

.session-pr-create {
  min-height: 24px;
  padding: 0 10px;
  background: transparent;
  color: #8fcaff;
  border: 1px solid rgba(143, 202, 255, 0.35);
  border-radius: 9px;
  font-size: 11px;
  font-weight: 700;
}

.session-pr-create:hover {
  background: rgba(143, 202, 255, 0.08);
  color: #d7eeff;
  border-color: rgba(143, 202, 255, 0.55);
}

.session-pr-create:disabled {
  color: #5a6575;
  border-color: rgba(255, 255, 255, 0.08);
  background: transparent;
}

.session-tab {
  background: transparent;
  border-radius: 10px;
  padding: 0 4px 0 0;
}

.session-tab:hover {
  background: rgba(126, 203, 255, 0.08);
}

.session-tab-active {
  background: rgba(126, 203, 255, 0.12);
}

.session-tab-select {
  background: transparent;
  border: none;
  border-radius: 10px;
  color: #8d98a8;
  font-size: 11px;
  font-weight: 700;
  padding: 4px 9px;
}

.session-tab-active .session-tab-select {
  color: #ecf6ff;
}

.session-add {
  min-width: 26px;
  min-height: 26px;
  padding: 0;
  background: transparent;
  border: none;
  border-radius: 8px;
  color: #7a8698;
  font-size: 16px;
  font-weight: 500;
}

.session-add:hover {
  background: rgba(126, 203, 255, 0.08);
  color: #e7f3ff;
}

.session-close {
  min-width: 18px;
  min-height: 18px;
  margin: 0 6px 0 0;
  padding: 0;
  background: transparent;
  border: none;
  border-radius: 999px;
  color: #7a8698;
  font-size: 11px;
  font-weight: 700;
  opacity: 0;
}

.session-close:hover {
  background: rgba(255, 133, 133, 0.16);
  color: #ffd9d9;
}

.session-tab:hover .session-close,
.session-tab-active .session-close {
  opacity: 1;
}

.terminal-host {
  background: linear-gradient(180deg, rgba(19, 25, 36, 0.96) 0%, rgba(12, 17, 26, 0.98) 100%);
  border: 1px solid rgba(154, 182, 255, 0.14);
  border-radius: 12px;
  min-height: 560px;
  padding: 2px 4px 2px 0;
  box-shadow: inset 0 1px rgba(255, 255, 255, 0.04), 0 18px 48px rgba(0, 0, 0, 0.24);
}

.terminal-view {
  background: transparent;
  color: #e6edf3;
  font-family: "JetBrainsMono Nerd Font", "JetBrains Mono", "Iosevka Term", "SF Mono", "Menlo", monospace;
  font-size: 10px;
}

.terminal-empty {
  color: #74839a;
  font-size: 13px;
}

.terminal-host scrollbar {
  background: transparent;
  margin: 2px 4px 2px 0;
  min-width: 10px;
}

.terminal-host scrollbar slider {
  background: rgba(143, 175, 255, 0.24);
  border-radius: 999px;
  min-width: 8px;
}

.terminal-host scrollbar slider:hover {
  background: rgba(143, 175, 255, 0.34);
}

"#;

pub fn run() -> Result<(), Box<dyn std::error::Error>> {
    let app = Application::builder().application_id(APP_ID).build();
    app.connect_activate(build_ui);
    app.run();
    Ok(())
}

fn build_ui(app: &Application) {
    install_css();

    let window = ApplicationWindow::builder()
        .application(app)
        .title("swarm")
        .default_width(1480)
        .default_height(920)
        .build();

    let shell = GtkBox::new(Orientation::Horizontal, 0);
    shell.add_css_class("app-shell");
    let sidebar_host = GtkBox::new(Orientation::Vertical, 0);
    let content_host = GtkBox::new(Orientation::Vertical, 0);
    content_host.set_hexpand(true);
    content_host.set_vexpand(true);
    let (pr_status_sender, pr_status_receiver) = mpsc::channel();

    let state = Rc::new(AppState {
        sidebar_host: sidebar_host.clone(),
        sidebar_widgets: RefCell::new(None),
        detail_widgets: RefCell::new(None),
        workspace_groups: RefCell::new(Vec::new()),
        selected_workspace: RefCell::new(None),
        editing_workspace: RefCell::new(None),
        selected_session: RefCell::new(None),
        selected_sessions: RefCell::new(HashMap::new()),
        repository_form: RefCell::new(RepositoryFormState::default()),
        branch_monitors: RefCell::new(Vec::new()),
        syncing_repositories: RefCell::new(HashSet::new()),
        pr_statuses: RefCell::new(HashMap::new()),
        pending_pr_lookups: RefCell::new(HashSet::new()),
        pr_status_sender,
        pr_status_receiver: RefCell::new(pr_status_receiver),
        creating_pr_workspaces: RefCell::new(HashSet::new()),
    });

    let sidebar_widgets = build_sidebar_widgets(&state);
    sidebar_host.append(&sidebar_widgets.container);
    *state.sidebar_widgets.borrow_mut() = Some(sidebar_widgets);

    let detail_widgets = DetailWidgets::new(&state);
    let content = build_content(detail_widgets.container.clone());
    *state.detail_widgets.borrow_mut() = Some(detail_widgets);
    content_host.append(&content);
    shell.append(&sidebar_host);
    shell.append(&content_host);
    window.set_child(Some(&shell));

    install_session_cycle_shortcuts(&window, &state);
    install_pr_status_pump(&state);
    install_pr_status_scheduler(&state);
    refresh_ui(&state, None, None);

    window.present();
}

pub struct AppState {
    sidebar_host: GtkBox,
    sidebar_widgets: RefCell<Option<SidebarWidgets>>,
    pub detail_widgets: RefCell<Option<DetailWidgets>>,
    pub workspace_groups: RefCell<Vec<WorkspaceGroup>>,
    pub selected_workspace: RefCell<Option<String>>,
    editing_workspace: RefCell<Option<String>>,
    pub selected_session: RefCell<Option<String>>,
    selected_sessions: RefCell<HashMap<String, String>>,
    repository_form: RefCell<RepositoryFormState>,
    branch_monitors: RefCell<Vec<FileMonitor>>,
    syncing_repositories: RefCell<HashSet<String>>,
    pr_statuses: RefCell<HashMap<String, CachedPrStatus>>,
    pending_pr_lookups: RefCell<HashSet<String>>,
    pr_status_sender: mpsc::Sender<WorkspacePrUpdate>,
    pr_status_receiver: RefCell<mpsc::Receiver<WorkspacePrUpdate>>,
    pub(crate) creating_pr_workspaces: RefCell<HashSet<String>>,
}

#[derive(Clone)]
struct SidebarWidgets {
    container: GtkBox,
    add_repository_button: Button,
    repository_form: RepositoryFormWidgets,
    list_host: GtkBox,
}

#[derive(Clone)]
struct RepositoryFormWidgets {
    container: GtkBox,
    repository_entry: Entry,
    alias_entry: Entry,
    error_label: Label,
    cancel_button: Button,
}

pub fn remember_selected_session(
    state: &Rc<AppState>,
    workspace_ref: &str,
    session_id: Option<&str>,
) {
    *state.selected_session.borrow_mut() = session_id.map(str::to_string);
    let mut selected_sessions = state.selected_sessions.borrow_mut();
    match session_id {
        Some(session_id) => {
            selected_sessions.insert(workspace_ref.to_string(), session_id.to_string());
        }
        None => {
            selected_sessions.remove(workspace_ref);
        }
    }
}

pub fn preferred_session_for_workspace(
    state: &Rc<AppState>,
    workspace_ref: &str,
    preferred_session: Option<&str>,
) -> Option<String> {
    preferred_session
        .map(str::to_string)
        .or_else(|| state.selected_sessions.borrow().get(workspace_ref).cloned())
}

#[derive(Clone, Default)]
struct RepositoryFormState {
    expanded: bool,
    repository: String,
    alias: String,
    focused_field: RepositoryFormField,
    error: Option<String>,
}

#[derive(Clone, Copy, Default, Eq, PartialEq)]
enum RepositoryFormField {
    #[default]
    Repository,
    Alias,
}

#[derive(Clone)]
struct CachedPrStatus {
    state: WorkspacePrState,
    fetched_at: Instant,
    head: Option<String>,
}

#[derive(Clone)]
pub enum WorkspacePrState {
    Loading,
    None,
    Ready(PullRequestStatus),
}

struct WorkspacePrUpdate {
    workspace_ref: String,
    status: Option<PullRequestStatus>,
    head: Option<String>,
}

fn refresh_ui(
    state: &Rc<AppState>,
    preferred_workspace: Option<String>,
    preferred_session: Option<String>,
) {
    let groups = match load_workspace_groups() {
        Ok(groups) => groups,
        Err(err) => {
            eprintln!("failed to load workspaces: {err}");
            return;
        }
    };
    *state.workspace_groups.borrow_mut() = groups.clone();
    render_ui(state, &groups, preferred_workspace, preferred_session);
}

fn render_ui(
    state: &Rc<AppState>,
    groups: &[WorkspaceGroup],
    preferred_workspace: Option<String>,
    preferred_session: Option<String>,
) {
    let selected_workspace = preferred_workspace
        .or_else(|| state.selected_workspace.borrow().clone())
        .and_then(|workspace_ref| find_workspace_by_ref(groups, &workspace_ref))
        .or_else(|| first_workspace(groups));

    let detail_widgets = state
        .detail_widgets
        .borrow()
        .as_ref()
        .cloned()
        .expect("detail widgets initialized");
    if let Some(workspace) = selected_workspace.as_ref() {
        let workspace_ref = workspace_ref(workspace);
        *state.selected_workspace.borrow_mut() = Some(workspace_ref);
        detail_widgets.render_workspace(workspace, state, preferred_session.as_deref());
    } else {
        detail_widgets.render_empty();
        *state.selected_workspace.borrow_mut() = None;
        *state.editing_workspace.borrow_mut() = None;
        *state.selected_session.borrow_mut() = None;
    }

    render_sidebar_content(groups, state, detail_widgets.clone(), selected_workspace);
}

fn schedule_refresh(
    state: &Rc<AppState>,
    preferred_workspace: Option<String>,
    preferred_session: Option<String>,
) {
    let state = state.clone();
    glib::idle_add_local_once(move || {
        refresh_ui(&state, preferred_workspace, preferred_session);
    });
}

pub fn current_groups(state: &Rc<AppState>) -> Vec<WorkspaceGroup> {
    state.workspace_groups.borrow().clone()
}

fn render_current_ui(
    state: &Rc<AppState>,
    preferred_workspace: Option<String>,
    preferred_session: Option<String>,
) {
    let groups = current_groups(state);
    render_ui(state, &groups, preferred_workspace, preferred_session);
}

pub(crate) fn render_selected_workspace_detail(
    state: &Rc<AppState>,
    workspace_ref: &str,
    preferred_session: Option<String>,
) {
    let groups = current_groups(state);
    let Some(workspace) = find_workspace_by_ref(&groups, workspace_ref) else {
        return;
    };

    let detail_widgets = state
        .detail_widgets
        .borrow()
        .as_ref()
        .cloned()
        .expect("detail widgets initialized");
    detail_widgets.render_workspace(&workspace, state, preferred_session.as_deref());
}

fn render_sidebar_only(state: &Rc<AppState>) {
    let groups = current_groups(state);
    let selected_workspace = state
        .selected_workspace
        .borrow()
        .clone()
        .and_then(|workspace_ref| find_workspace_by_ref(&groups, &workspace_ref))
        .or_else(|| first_workspace(&groups));
    let detail_widgets = state
        .detail_widgets
        .borrow()
        .as_ref()
        .cloned()
        .expect("detail widgets initialized");
    render_sidebar_content(&groups, state, detail_widgets, selected_workspace);
}

fn schedule_render_sidebar_only(state: &Rc<AppState>) {
    let state = state.clone();
    glib::idle_add_local_once(move || {
        render_sidebar_only(&state);
    });
}

fn schedule_background_sidebar_refresh(state: &Rc<AppState>) {
    if state.repository_form.borrow().expanded {
        return;
    }

    schedule_render_sidebar_only(state);
}

pub fn schedule_render_current_ui(
    state: &Rc<AppState>,
    preferred_workspace: Option<String>,
    preferred_session: Option<String>,
) {
    let state = state.clone();
    glib::idle_add_local_once(move || {
        render_current_ui(&state, preferred_workspace, preferred_session);
    });
}

fn install_pr_status_pump(state: &Rc<AppState>) {
    let state = state.clone();
    glib::timeout_add_local(Duration::from_millis(50), move || {
        let mut changed = false;
        let mut selected_workspace_changed = false;
        let selected_workspace = state.selected_workspace.borrow().clone();
        {
            let receiver = state.pr_status_receiver.borrow_mut();
            while let Ok(update) = receiver.try_recv() {
                if selected_workspace.as_deref() == Some(update.workspace_ref.as_str()) {
                    selected_workspace_changed = true;
                }
                state
                    .pending_pr_lookups
                    .borrow_mut()
                    .remove(&update.workspace_ref);
                state.pr_statuses.borrow_mut().insert(
                    update.workspace_ref,
                    CachedPrStatus {
                        state: update
                            .status
                            .map(WorkspacePrState::Ready)
                            .unwrap_or(WorkspacePrState::None),
                        fetched_at: Instant::now(),
                        head: update.head,
                    },
                );
                changed = true;
            }
        }

        if changed {
            if selected_workspace_changed {
                if let Some(selected_workspace) = selected_workspace.as_deref() {
                    render_selected_workspace_detail(&state, selected_workspace, None);
                }
            } else if !state.repository_form.borrow().expanded {
                render_sidebar_only(&state);
            }
        }

        glib::ControlFlow::Continue
    });
}

fn install_pr_status_scheduler(state: &Rc<AppState>) {
    let state = state.clone();
    glib::timeout_add_local(Duration::from_secs(1), move || {
        schedule_due_pr_status_lookups(&state);
        glib::ControlFlow::Continue
    });
}

pub fn workspace_pr_snapshot(state: &Rc<AppState>, workspace: &WorkspaceEntry) -> WorkspacePrState {
    let workspace_id = workspace_ref(workspace);
    let cached = state.pr_statuses.borrow().get(&workspace_id).cloned();
    let is_selected = state
        .selected_workspace
        .borrow()
        .as_ref()
        .is_some_and(|selected| selected == &workspace_id);
    let head_changed = is_selected
        && cached
            .as_ref()
            .is_some_and(|entry| workspace_pr_head_changed(entry, workspace));
    let expired = cached.as_ref().is_some_and(|entry| {
        entry.fetched_at.elapsed() >= pr_status_ttl(&entry.state, is_selected)
    });

    if cached.is_none() || expired || head_changed {
        request_workspace_pr_status(state, workspace, cached.is_none());
    }

    cached
        .map(|entry| entry.state)
        .unwrap_or(WorkspacePrState::Loading)
}

fn schedule_due_pr_status_lookups(state: &Rc<AppState>) {
    let available_slots =
        MAX_PENDING_PR_LOOKUPS.saturating_sub(state.pending_pr_lookups.borrow().len());
    if available_slots == 0 {
        return;
    }

    let selected_workspace = state.selected_workspace.borrow().clone();
    let groups = current_groups(state);
    let due_workspaces = due_pr_status_workspaces(&groups, state, selected_workspace.as_deref());
    for workspace in due_workspaces.into_iter().take(available_slots) {
        request_workspace_pr_status(state, &workspace, false);
    }
}

fn due_pr_status_workspaces(
    groups: &[WorkspaceGroup],
    state: &Rc<AppState>,
    selected_workspace: Option<&str>,
) -> Vec<WorkspaceEntry> {
    let statuses = state.pr_statuses.borrow();
    let pending = state.pending_pr_lookups.borrow();
    let mut selected = Vec::new();
    let mut rest = Vec::new();

    for group in groups {
        for workspace in &group.workspaces {
            let workspace_id = workspace_ref(workspace);
            if pending.contains(&workspace_id) {
                continue;
            }

            let is_selected =
                selected_workspace.is_some_and(|selected| selected == workspace_id.as_str());
            let is_due = match statuses.get(&workspace_id) {
                Some(entry) => {
                    if is_selected && workspace_pr_head_changed(entry, workspace) {
                        true
                    } else {
                        entry.fetched_at.elapsed() >= pr_status_ttl(&entry.state, is_selected)
                    }
                }
                None => true,
            };
            if !is_due {
                continue;
            }

            if is_selected {
                selected.push(workspace.clone());
            } else {
                rest.push(workspace.clone());
            }
        }
    }

    selected.extend(rest);
    selected
}

fn pr_status_ttl(state: &WorkspacePrState, is_selected: bool) -> Duration {
    if is_selected {
        return SELECTED_PR_STATUS_TTL;
    }

    match state {
        WorkspacePrState::Loading => LOADING_PR_STATUS_TTL,
        WorkspacePrState::None => NO_PR_STATUS_TTL,
        WorkspacePrState::Ready(status) => match status.state {
            PullRequestStatusState::Pending => PENDING_PR_STATUS_TTL,
            PullRequestStatusState::Success | PullRequestStatusState::Failure => {
                SETTLED_PR_STATUS_TTL
            }
            PullRequestStatusState::Merged => MERGED_PR_STATUS_TTL,
        },
    }
}

fn workspace_pr_head_changed(cached: &CachedPrStatus, workspace: &WorkspaceEntry) -> bool {
    let Ok(current_head) = current_workspace_head(&workspace.path) else {
        return false;
    };

    cached.head.as_deref() != Some(current_head.as_str())
}

pub(crate) fn request_workspace_pr_status(
    state: &Rc<AppState>,
    workspace: &WorkspaceEntry,
    mark_loading: bool,
) {
    let workspace_id = workspace_ref(workspace);
    if state.pending_pr_lookups.borrow().contains(&workspace_id) {
        return;
    }

    state
        .pending_pr_lookups
        .borrow_mut()
        .insert(workspace_id.clone());
    if mark_loading {
        state.pr_statuses.borrow_mut().insert(
            workspace_id.clone(),
            CachedPrStatus {
                state: WorkspacePrState::Loading,
                fetched_at: Instant::now(),
                head: None,
            },
        );
    }

    let sender = state.pr_status_sender.clone();
    let workspace_path = workspace.path.clone();
    std::thread::spawn(move || {
        let head = current_workspace_head(&workspace_path).ok();
        let status = github::workspace_pull_request_status(Path::new(&workspace_path));
        let _ = sender.send(WorkspacePrUpdate {
            workspace_ref: workspace_id,
            status,
            head,
        });
    });
}

pub(crate) fn clear_workspace_pr_status(state: &Rc<AppState>, workspace_ref: &str) {
    state.pr_statuses.borrow_mut().remove(workspace_ref);
    state.pending_pr_lookups.borrow_mut().remove(workspace_ref);
}

fn update_cached_workspace_branch(state: &Rc<AppState>, workspace_id: &str, branch: &str) {
    let mut next_groups = current_groups(state);
    for group in &mut next_groups {
        if let Some(workspace) = group
            .workspaces
            .iter_mut()
            .find(|workspace| workspace_ref(workspace) == workspace_id)
        {
            workspace.branch = branch.to_string();
            break;
        }
    }
    *state.workspace_groups.borrow_mut() = next_groups;
}

fn install_session_cycle_shortcuts(window: &ApplicationWindow, state: &Rc<AppState>) {
    let controller = EventControllerKey::new();
    controller.set_propagation_phase(PropagationPhase::Capture);
    {
        let state = state.clone();
        controller.connect_key_pressed(move |_controller, key, _keycode, modifiers| {
            let control = modifiers.contains(gdk::ModifierType::CONTROL_MASK);
            if !control {
                return glib::Propagation::Proceed;
            }

            let direction = match key {
                gdk::Key::Page_Up => -1,
                gdk::Key::Page_Down => 1,
                _ => return glib::Propagation::Proceed,
            };

            cycle_selected_session(&state, direction);
            glib::Propagation::Stop
        });
    }
    window.add_controller(controller);
}

fn cycle_selected_session(state: &Rc<AppState>, direction: isize) {
    let Some(workspace_ref) = state.selected_workspace.borrow().clone() else {
        return;
    };

    let groups = current_groups(state);

    let Some(workspace) = find_workspace_by_ref(&groups, &workspace_ref) else {
        return;
    };

    if workspace.sessions.len() < 2 {
        return;
    }

    let current_index = state
        .selected_session
        .borrow()
        .as_ref()
        .and_then(|selected| {
            workspace
                .sessions
                .iter()
                .position(|session| session.id == *selected)
        })
        .unwrap_or(0);
    let session_count = workspace.sessions.len() as isize;
    let next_index = (current_index as isize + direction).rem_euclid(session_count) as usize;
    let next_session = workspace.sessions[next_index].id.clone();

    remember_selected_session(state, &workspace_ref, Some(&next_session));

    // Switch the visible child in the existing stack instead of destroying
    // and recreating all terminal widgets via schedule_refresh. Rebuilding
    // from scratch causes garbled output because terminals are re-populated
    // from log files while the socket pump races to deliver new data.
    if let Some(detail_widgets) = state.detail_widgets.borrow().as_ref() {
        detail_widgets
            .session_stack
            .set_visible_child_name(&next_session);
    }
}

fn build_sidebar_widgets(state: &Rc<AppState>) -> SidebarWidgets {
    let container = GtkBox::new(Orientation::Vertical, 0);
    container.set_size_request(250, -1);
    container.set_hexpand(false);
    container.set_vexpand(true);
    container.add_css_class("sidebar");

    let toolbar = GtkBox::new(Orientation::Horizontal, 8);
    toolbar.set_halign(Align::Fill);
    toolbar.set_hexpand(true);
    toolbar.add_css_class("sidebar-toolbar");

    let add_repository_button = Button::with_label("Add repository");
    add_repository_button.set_valign(Align::Center);
    add_repository_button.add_css_class("sidebar-primary-action");
    {
        let state = state.clone();
        add_repository_button.connect_clicked(move |_| {
            {
                let mut form = state.repository_form.borrow_mut();
                form.expanded = true;
                form.focused_field = RepositoryFormField::Repository;
                form.error = None;
            }
            sync_repository_form_widgets(&state, false);
            focus_repository_form_field(&state);
        });
    }
    toolbar.append(&add_repository_button);

    let repository_form = build_repository_form_widgets(state);
    let list_host = GtkBox::new(Orientation::Vertical, 0);
    list_host.set_hexpand(true);
    list_host.set_vexpand(true);

    container.append(&toolbar);
    container.append(&repository_form.container);
    container.append(&list_host);

    SidebarWidgets {
        container,
        add_repository_button,
        repository_form,
        list_host,
    }
}

fn render_sidebar_content(
    groups: &[WorkspaceGroup],
    state: &Rc<AppState>,
    detail_widgets: DetailWidgets,
    selected_workspace: Option<WorkspaceEntry>,
) {
    clear_branch_monitors(state);

    let groups_empty = groups.is_empty();
    sync_repository_form_widgets(state, groups_empty);

    let scroller = ScrolledWindow::new();
    scroller.set_policy(PolicyType::Never, PolicyType::Automatic);
    scroller.set_hexpand(true);
    scroller.set_vexpand(true);

    let list = ListBox::new();
    list.set_selection_mode(SelectionMode::Single);
    list.add_css_class("workspace-list");
    scroller.set_child(Some(&list));

    let rows = Rc::new(RefCell::new(Vec::<WorkspaceRow>::new()));

    for group in groups {
        let is_syncing = state
            .syncing_repositories
            .borrow()
            .contains(&group.repo_canonical);
        let repo_row = build_repo_row(
            &state,
            &group.repo_label,
            &group.repo_canonical,
            group.repo_status.as_deref(),
            is_syncing,
            group.collapsed,
        );
        sidebar_append_static_row(&list, &repo_row);

        if !group.collapsed {
            for workspace in &group.workspaces {
                let is_editing = state
                    .editing_workspace
                    .borrow()
                    .as_ref()
                    .is_some_and(|editing| editing == &workspace_ref(workspace));
                let is_selected = selected_workspace
                    .as_ref()
                    .is_some_and(|selected| selected.path == workspace.path);
                let workspace_row = build_workspace_row(workspace, &state, is_editing, is_selected);
                let row = workspace_row.row.clone();
                if is_selected {
                    list.select_row(Some(&row));
                }
                rows.borrow_mut().push(workspace_row);
                list.append(&row);
            }
        }
    }

    if groups_empty {
        let empty = Label::new(Some("Add a repository to start creating workspaces."));
        empty.set_wrap(true);
        empty.set_wrap_mode(gtk::pango::WrapMode::WordChar);
        empty.set_xalign(0.0);
        empty.add_css_class("terminal-empty");
        sidebar_append_static_row(&list, &empty);
    }

    {
        let rows_for_signal = rows.clone();
        let state = state.clone();
        let detail_widgets = detail_widgets.clone();
        list.connect_row_selected(move |_list, row| {
            let Some(row) = row else {
                return;
            };
            let selected_row = row.clone();

            if !row.is_selectable() {
                return;
            }

            let selected_workspace = {
                let rows = rows_for_signal.borrow();
                for candidate in rows.iter() {
                    let is_selected = candidate.row == selected_row;
                    candidate.clone_button.set_visible(is_selected);
                    candidate.clone_button.set_sensitive(is_selected);
                    candidate.delete_button.set_visible(is_selected);
                    candidate.delete_button.set_sensitive(is_selected);
                }
                rows.iter()
                    .find(|workspace_row| workspace_row.row == selected_row)
                    .map(|workspace_row| workspace_row.workspace.clone())
            };

            if let Some(workspace) = selected_workspace {
                let next_workspace_ref = workspace_ref(&workspace);
                let preserve_session = state
                    .selected_workspace
                    .borrow()
                    .as_ref()
                    .is_some_and(|selected| selected == &next_workspace_ref);
                if preserve_session {
                    return;
                }

                let preferred_session =
                    preferred_session_for_workspace(&state, &next_workspace_ref, None);
                *state.selected_workspace.borrow_mut() = Some(next_workspace_ref);
                *state.selected_session.borrow_mut() = preferred_session.clone();
                detail_widgets.render_workspace(&workspace, &state, preferred_session.as_deref());
            }
        });
    }

    let sidebar_widgets = state
        .sidebar_widgets
        .borrow()
        .as_ref()
        .cloned()
        .expect("sidebar widgets initialized");
    clear_box(&sidebar_widgets.list_host);
    sidebar_widgets.list_host.append(&scroller);
}

fn clear_branch_monitors(state: &Rc<AppState>) {
    let mut monitors = state.branch_monitors.borrow_mut();
    for monitor in monitors.iter() {
        monitor.cancel();
    }
    monitors.clear();
}

fn build_repository_form_widgets(state: &Rc<AppState>) -> RepositoryFormWidgets {
    let card = GtkBox::new(Orientation::Vertical, 6);
    card.add_css_class("repo-compose");

    let repository_entry = Entry::new();
    repository_entry.set_placeholder_text(Some("github.com/owner/name"));
    repository_entry.add_css_class("repo-compose-entry");
    card.append(&repository_entry);

    let alias_entry = Entry::new();
    alias_entry.set_placeholder_text(Some("Optional alias"));
    alias_entry.add_css_class("repo-compose-entry");
    card.append(&alias_entry);

    let error_label = Label::new(None);
    error_label.set_xalign(0.0);
    error_label.set_wrap(true);
    error_label.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    error_label.add_css_class("repo-compose-error");
    error_label.set_visible(false);
    card.append(&error_label);

    let actions = GtkBox::new(Orientation::Horizontal, 6);

    let submit = Button::with_label("Add");
    submit.add_css_class("repo-compose-submit");
    actions.append(&submit);

    let cancel = Button::with_label("Cancel");
    cancel.add_css_class("repo-compose-cancel");
    {
        let state = state.clone();
        cancel.connect_clicked(move |_| {
            {
                let mut form = state.repository_form.borrow_mut();
                form.expanded = false;
                form.focused_field = RepositoryFormField::Repository;
                form.error = None;
            }
            sync_repository_form_widgets(&state, false);
        });
    }
    actions.append(&cancel);

    card.append(&actions);

    {
        let state = state.clone();
        repository_entry.connect_changed(move |entry| {
            let mut form = state.repository_form.borrow_mut();
            form.repository = entry.text().to_string();
            form.error = None;
        });
    }

    {
        let state = state.clone();
        alias_entry.connect_changed(move |entry| {
            let mut form = state.repository_form.borrow_mut();
            form.alias = entry.text().to_string();
            form.error = None;
        });
    }

    {
        let state = state.clone();
        repository_entry.connect_has_focus_notify(move |entry| {
            if entry.has_focus() {
                state.repository_form.borrow_mut().focused_field = RepositoryFormField::Repository;
            }
        });
    }

    {
        let state = state.clone();
        alias_entry.connect_has_focus_notify(move |entry| {
            if entry.has_focus() {
                state.repository_form.borrow_mut().focused_field = RepositoryFormField::Alias;
            }
        });
    }

    {
        let state = state.clone();
        let repository_entry = repository_entry.clone();
        let alias_entry = alias_entry.clone();
        submit.connect_clicked(move |_| {
            submit_repository_form(&state, &repository_entry, &alias_entry);
        });
    }

    {
        let state = state.clone();
        let submit_repository_entry = repository_entry.clone();
        let submit_alias_entry = alias_entry.clone();
        repository_entry.clone().connect_activate(move |_| {
            submit_repository_form(&state, &submit_repository_entry, &submit_alias_entry);
        });
    }

    {
        let state = state.clone();
        let submit_repository_entry = repository_entry.clone();
        let submit_alias_entry = alias_entry.clone();
        alias_entry.clone().connect_activate(move |_| {
            submit_repository_form(&state, &submit_repository_entry, &submit_alias_entry);
        });
    }

    RepositoryFormWidgets {
        container: card,
        repository_entry,
        alias_entry,
        error_label,
        cancel_button: cancel,
    }
}

fn sync_repository_form_widgets(state: &Rc<AppState>, groups_empty: bool) {
    let form_state = state.repository_form.borrow().clone();
    let sidebar_widgets = state
        .sidebar_widgets
        .borrow()
        .as_ref()
        .cloned()
        .expect("sidebar widgets initialized");
    let widgets = &sidebar_widgets.repository_form;

    sidebar_widgets
        .add_repository_button
        .set_visible(!groups_empty);
    widgets
        .container
        .set_visible(form_state.expanded || groups_empty);
    widgets.cancel_button.set_visible(!groups_empty);

    if widgets.repository_entry.text().as_str() != form_state.repository.as_str() {
        widgets.repository_entry.set_text(&form_state.repository);
    }

    if widgets.alias_entry.text().as_str() != form_state.alias.as_str() {
        widgets.alias_entry.set_text(&form_state.alias);
    }

    match form_state.error {
        Some(error) => {
            widgets.error_label.set_label(&error);
            widgets.error_label.set_visible(true);
        }
        None => {
            widgets.error_label.set_label("");
            widgets.error_label.set_visible(false);
        }
    }
}

fn focus_repository_form_field(state: &Rc<AppState>) {
    let focused_field = state.repository_form.borrow().focused_field;
    let sidebar_widgets = state
        .sidebar_widgets
        .borrow()
        .as_ref()
        .cloned()
        .expect("sidebar widgets initialized");
    let widgets = sidebar_widgets.repository_form.clone();

    glib::idle_add_local_once(move || match focused_field {
        RepositoryFormField::Repository => {
            widgets.repository_entry.grab_focus();
        }
        RepositoryFormField::Alias => {
            widgets.alias_entry.grab_focus();
        }
    });
}

fn build_repo_row(
    state: &Rc<AppState>,
    repo_label: &str,
    repo_canonical: &str,
    repo_status: Option<&str>,
    is_syncing: bool,
    collapsed: bool,
) -> GtkBox {
    let row = GtkBox::new(Orientation::Horizontal, 0);
    row.set_halign(Align::Fill);
    row.set_hexpand(true);
    row.set_valign(Align::Center);
    row.set_spacing(6);
    row.add_css_class("repo-row");

    let copy = GtkBox::new(Orientation::Vertical, 0);
    copy.set_hexpand(true);
    copy.set_valign(Align::Center);

    let label = Label::new(Some(&repo_label.to_uppercase()));
    label.set_xalign(0.0);
    label.set_valign(Align::Center);
    label.add_css_class("repo-header");
    copy.append(&label);
    row.append(&copy);

    if is_syncing || repo_status.is_some() {
        let meta_text = if is_syncing {
            "Syncing ...".to_string()
        } else {
            repo_status
                .expect("repo status should exist when not syncing")
                .to_string()
        };
        let meta_tooltip = if is_syncing {
            "Syncing ...".to_string()
        } else {
            format!("Last synced {meta_text}")
        };
        let meta = Label::new(Some(&meta_text));
        meta.set_xalign(1.0);
        meta.set_valign(Align::Center);
        meta.add_css_class("repo-status");
        meta.set_tooltip_text(Some(&meta_tooltip));
        row.append(&meta);
    }

    let sync_button = Button::new();
    sync_button.set_valign(Align::Center);
    sync_button.add_css_class("repo-sync");
    sync_button.set_sensitive(!is_syncing);
    sync_button.set_tooltip_text(Some(if is_syncing {
        "Syncing ..."
    } else {
        "Sync repository"
    }));
    if is_syncing {
        let spinner = Spinner::new();
        spinner.start();
        sync_button.set_child(Some(&spinner));
    } else {
        let sync_icon = Image::from_icon_name("view-refresh-symbolic");
        sync_icon.set_pixel_size(14);
        sync_button.set_child(Some(&sync_icon));
    }

    {
        let state = state.clone();
        let repo_canonical = repo_canonical.to_string();
        sync_button.connect_clicked(move |_| {
            sync_repo_and_refresh(&state, &repo_canonical);
        });
    }

    let button = Button::with_label("+");
    button.set_valign(Align::Center);
    button.add_css_class("repo-add");
    button.set_tooltip_text(Some("New workspace"));

    {
        let state = state.clone();
        let repo_canonical = repo_canonical.to_string();
        button.connect_clicked(move |_| {
            create_and_edit_workspace(&state, &repo_canonical);
        });
    }

    let toggle_button = Button::new();
    toggle_button.set_valign(Align::Center);
    toggle_button.add_css_class("repo-toggle");
    toggle_button.set_tooltip_text(Some(if collapsed {
        "Expand repository"
    } else {
        "Collapse repository"
    }));
    let toggle_icon = Image::from_icon_name(if collapsed {
        "pan-end-symbolic"
    } else {
        "pan-down-symbolic"
    });
    toggle_icon.set_pixel_size(14);
    toggle_button.set_child(Some(&toggle_icon));

    {
        let state = state.clone();
        let repo_canonical = repo_canonical.to_string();
        toggle_button.connect_clicked(move |_| {
            toggle_repo_collapsed_and_refresh(&state, &repo_canonical, collapsed);
        });
    }

    let hover = EventControllerMotion::new();
    {
        let row = row.clone();
        hover.connect_enter(move |_, _, _| {
            row.add_css_class("repo-row-hover");
        });
    }
    {
        let row = row.clone();
        hover.connect_leave(move |_| {
            row.remove_css_class("repo-row-hover");
        });
    }
    row.add_controller(hover);

    row.append(&sync_button);
    row.append(&button);
    row.append(&toggle_button);
    row
}

fn build_workspace_row(
    workspace: &WorkspaceEntry,
    state: &Rc<AppState>,
    is_editing: bool,
    is_selected: bool,
) -> WorkspaceRow {
    let row = ListBoxRow::new();
    row.set_selectable(true);
    row.set_activatable(true);
    row.add_css_class("workspace-row");

    let container = Grid::new();
    container.set_hexpand(true);
    container.set_column_spacing(16);
    container.set_row_spacing(4);
    container.set_margin_start(4);

    let status_slot = GtkBox::new(Orientation::Horizontal, 0);
    status_slot.set_valign(Align::Center);
    status_slot.set_halign(Align::Center);
    status_slot.add_css_class("workspace-status-slot");

    let clone_slot = GtkBox::new(Orientation::Horizontal, 0);
    clone_slot.set_valign(Align::Center);
    clone_slot.set_halign(Align::Center);
    clone_slot.add_css_class("workspace-action-slot-end");

    let clone_button = Button::new();
    clone_button.set_valign(Align::Center);
    clone_button.set_tooltip_text(Some("Clone workspace"));
    clone_button.add_css_class("workspace-action");
    clone_button.add_css_class("workspace-clone");
    clone_button.set_visible(is_selected);
    clone_button.set_sensitive(is_selected);

    let clone_icon = Image::from_icon_name("edit-copy-symbolic");
    clone_icon.set_pixel_size(12);
    clone_button.set_child(Some(&clone_icon));

    {
        let state = state.clone();
        let workspace = workspace.clone();
        clone_button.connect_clicked(move |_| {
            clone_and_edit_workspace(&state, &workspace);
        });
    }

    let delete_button = Button::new();
    delete_button.set_valign(Align::Center);
    delete_button.set_tooltip_text(Some("Remove workspace"));
    delete_button.add_css_class("workspace-action");
    delete_button.add_css_class("workspace-delete");
    delete_button.set_visible(is_selected);
    delete_button.set_sensitive(is_selected);
    delete_button.set_margin_end(8);

    let icon = Image::from_icon_name("user-trash-symbolic");
    icon.set_pixel_size(12);
    delete_button.set_child(Some(&icon));

    {
        let state = state.clone();
        let workspace = workspace.clone();
        delete_button.connect_clicked(move |_| {
            remove_selected_workspace(&state, &workspace);
        });
    }

    clone_slot.append(&delete_button);
    clone_slot.append(&clone_button);

    let branch = Label::new(Some(&workspace.branch));
    branch.set_xalign(0.0);
    branch.set_hexpand(true);
    branch.add_css_class("workspace-name");

    if is_editing {
        let entry = Entry::new();
        entry.set_hexpand(true);
        entry.set_text(&workspace.name);
        entry.select_region(0, -1);
        entry.add_css_class("workspace-name-entry");
        install_workspace_rename_handlers(&entry, state, workspace);
        container.attach(&entry, 1, 1, 1, 1);
        glib::idle_add_local_once(move || {
            entry.grab_focus();
        });
    } else {
        let meta = Label::new(Some(&workspace_meta_text(
            &workspace.name,
            workspace.sessions.len(),
        )));
        meta.set_xalign(0.0);
        meta.set_hexpand(true);
        meta.add_css_class("workspace-meta");
        container.attach(&meta, 1, 1, 1, 1);
    }

    install_branch_monitor(state, workspace, &branch);

    status_slot.append(&build_workspace_status_indicator(state, workspace));
    container.attach(&status_slot, 0, 0, 1, 2);
    container.attach(&branch, 1, 0, 1, 1);
    container.attach(&clone_slot, 2, 0, 1, 2);
    row.set_child(Some(&container));

    WorkspaceRow {
        row,
        workspace: workspace.clone(),
        clone_button,
        delete_button,
    }
}

fn build_workspace_status_indicator(state: &Rc<AppState>, workspace: &WorkspaceEntry) -> Widget {
    let indicator = GtkBox::new(Orientation::Horizontal, 0);
    indicator.add_css_class("workspace-pr-indicator");
    match workspace_pr_snapshot(state, workspace) {
        WorkspacePrState::Loading => {
            indicator.set_tooltip_text(Some("Loading pull request status..."));
        }
        WorkspacePrState::None => {
            indicator.set_tooltip_text(Some("No pull request"));
        }
        WorkspacePrState::Ready(status) => {
            indicator.add_css_class(match status.state {
                PullRequestStatusState::Success => "workspace-pr-success",
                PullRequestStatusState::Pending => "workspace-pr-pending",
                PullRequestStatusState::Failure => "workspace-pr-failure",
                PullRequestStatusState::Merged => "workspace-pr-merged",
            });
            indicator.set_tooltip_text(Some(&status.summary));
            if let Some(url) = &status.url {
                indicator.set_tooltip_text(Some(&format!("{}\n{}", status.summary, url)));
            }
        }
    }
    indicator.upcast()
}

fn install_branch_monitor(state: &Rc<AppState>, workspace: &WorkspaceEntry, branch: &Label) {
    let head_path = match workspace_head_path(&workspace.path) {
        Ok(path) => path,
        Err(err) => {
            eprintln!("failed to resolve HEAD for {}: {err}", workspace.path);
            return;
        }
    };

    let monitor = match gio::File::for_path(head_path)
        .monitor_file(FileMonitorFlags::NONE, None::<&gio::Cancellable>)
    {
        Ok(monitor) => monitor,
        Err(err) => {
            eprintln!(
                "failed to watch workspace branch for {}: {err}",
                workspace.path
            );
            return;
        }
    };

    let label = branch.clone();
    let monitor_state = state.clone();
    let workspace_ref = workspace_ref(workspace);
    let workspace_path = workspace.path.clone();
    monitor.connect_changed(
        move |_, _, _, _| match current_workspace_branch(&workspace_path) {
            Ok(branch) => {
                clear_workspace_pr_status(&monitor_state, &workspace_ref);
                update_cached_workspace_branch(&monitor_state, &workspace_ref, &branch);
                label.set_text(&branch);
                schedule_background_sidebar_refresh(&monitor_state);
            }
            Err(err) => eprintln!("failed to refresh branch for {workspace_path}: {err}"),
        },
    );

    state.branch_monitors.borrow_mut().push(monitor);
}

fn workspace_meta_text(name: &str, session_count: usize) -> String {
    format!("{name}  •  {session_count} sessions")
}

fn build_content(detail_container: GtkBox) -> GtkBox {
    let content = GtkBox::new(Orientation::Vertical, 0);
    content.set_hexpand(true);
    content.set_vexpand(true);
    content.add_css_class("content");
    content.append(&detail_container);
    content
}

fn create_and_edit_workspace(state: &Rc<AppState>, repo_canonical: &str) {
    match create_workspace(repo_canonical, None) {
        Ok(workspace) => {
            let workspace_ref = workspace_ref(&workspace);
            let selected_session = workspace.sessions.first().map(|session| session.id.clone());
            let preferred_session = selected_session.clone();
            let mut next_groups = current_groups(state);
            if let Some(group) = next_groups
                .iter_mut()
                .find(|group| group.repo_canonical == workspace.repo_canonical)
            {
                group.workspaces.push(workspace);
                group.workspace_count = group.workspaces.len();
            }
            *state.workspace_groups.borrow_mut() = next_groups.clone();
            *state.selected_workspace.borrow_mut() = Some(workspace_ref.clone());
            *state.editing_workspace.borrow_mut() = Some(workspace_ref.clone());
            remember_selected_session(state, &workspace_ref, selected_session.as_deref());
            schedule_render_current_ui(state, Some(workspace_ref), preferred_session);
        }
        Err(err) => {
            eprintln!("failed to create workspace: {err}");
        }
    }
}

fn clone_and_edit_workspace(state: &Rc<AppState>, workspace: &WorkspaceEntry) {
    let source_workspace_ref = workspace_ref(workspace);
    match clone_workspace(&source_workspace_ref, &workspace.name) {
        Ok(workspace) => {
            let workspace_ref = workspace_ref(&workspace);
            let selected_session = workspace.sessions.first().map(|session| session.id.clone());
            let preferred_session = selected_session.clone();
            let mut next_groups = current_groups(state);
            if let Some(group) = next_groups
                .iter_mut()
                .find(|group| group.repo_canonical == workspace.repo_canonical)
            {
                group.workspaces.push(workspace);
                group.workspace_count = group.workspaces.len();
            }
            *state.workspace_groups.borrow_mut() = next_groups.clone();
            *state.selected_workspace.borrow_mut() = Some(workspace_ref.clone());
            *state.editing_workspace.borrow_mut() = Some(workspace_ref.clone());
            remember_selected_session(state, &workspace_ref, selected_session.as_deref());
            schedule_render_current_ui(state, Some(workspace_ref), preferred_session);
        }
        Err(err) => {
            eprintln!("failed to clone workspace: {err}");
            schedule_render_current_ui(state, Some(source_workspace_ref), None);
        }
    }
}

fn sync_repo_and_refresh(state: &Rc<AppState>, repo_canonical: &str) {
    if !state
        .syncing_repositories
        .borrow_mut()
        .insert(repo_canonical.to_string())
    {
        return;
    }

    let preferred_workspace = state.selected_workspace.borrow().clone();
    let preferred_session = state.selected_session.borrow().clone();
    let repo_canonical = repo_canonical.to_string();
    schedule_refresh(
        state,
        preferred_workspace.clone(),
        preferred_session.clone(),
    );

    let (sender, receiver) = mpsc::channel();
    {
        let repo_canonical = repo_canonical.clone();
        std::thread::spawn(move || {
            let _ = sender.send(sync_repository(&repo_canonical));
        });
    }

    let state = state.clone();
    glib::timeout_add_local(Duration::from_millis(50), move || {
        match receiver.try_recv() {
            Ok(result) => {
                state
                    .syncing_repositories
                    .borrow_mut()
                    .remove(&repo_canonical);
                match result {
                    Ok(()) => {
                        schedule_refresh(
                            &state,
                            preferred_workspace.clone(),
                            preferred_session.clone(),
                        );
                    }
                    Err(err) => {
                        eprintln!("failed to sync repository: {err}");
                        schedule_refresh(
                            &state,
                            preferred_workspace.clone(),
                            preferred_session.clone(),
                        );
                    }
                }
                glib::ControlFlow::Break
            }
            Err(mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
            Err(mpsc::TryRecvError::Disconnected) => {
                state
                    .syncing_repositories
                    .borrow_mut()
                    .remove(&repo_canonical);
                schedule_refresh(
                    &state,
                    preferred_workspace.clone(),
                    preferred_session.clone(),
                );
                glib::ControlFlow::Break
            }
        }
    });
}

fn toggle_repo_collapsed_and_refresh(state: &Rc<AppState>, repo_canonical: &str, collapsed: bool) {
    let result = if collapsed {
        expand_repository(repo_canonical)
    } else {
        collapse_repository(repo_canonical)
    };

    match result {
        Ok(()) => {
            let mut next_groups = current_groups(state);
            if let Some(group) = next_groups
                .iter_mut()
                .find(|group| group.repo_canonical == repo_canonical)
            {
                group.collapsed = !collapsed;
            }
            *state.workspace_groups.borrow_mut() = next_groups.clone();
            *state.editing_workspace.borrow_mut() = None;
            *state.selected_session.borrow_mut() = None;
            schedule_render_current_ui(state, None, None);
        }
        Err(err) => {
            eprintln!("failed to toggle repository collapse: {err}");
            let preferred_workspace = state.selected_workspace.borrow().clone();
            schedule_render_current_ui(state, preferred_workspace, None);
        }
    }
}

fn sidebar_append_static_row<W: IsA<Widget>>(list: &ListBox, widget: &W) {
    let row = ListBoxRow::new();
    row.set_selectable(false);
    row.set_activatable(false);
    row.set_child(Some(widget));
    list.append(&row);
}

fn first_workspace(groups: &[WorkspaceGroup]) -> Option<WorkspaceEntry> {
    groups
        .iter()
        .find_map(|group| group.workspaces.first().cloned())
}

fn find_workspace_by_ref(groups: &[WorkspaceGroup], selected_ref: &str) -> Option<WorkspaceEntry> {
    groups.iter().find_map(|group| {
        group.workspaces.iter().find_map(|workspace| {
            if workspace_ref(workspace) == selected_ref {
                Some(workspace.clone())
            } else {
                None
            }
        })
    })
}

pub fn workspace_ref(workspace: &WorkspaceEntry) -> String {
    format!("{}:{}", workspace.repo_canonical, workspace.name)
}

fn install_workspace_rename_handlers(
    entry: &Entry,
    state: &Rc<AppState>,
    workspace: &WorkspaceEntry,
) {
    let workspace_ref = workspace_ref(workspace);
    let committed = Rc::new(Cell::new(false));

    {
        let state = state.clone();
        let workspace_ref = workspace_ref.clone();
        let committed = committed.clone();
        entry.connect_activate(move |entry| {
            committed.set(true);
            commit_workspace_rename(&state, &workspace_ref, &entry.text());
        });
    }

    {
        let state = state.clone();
        let workspace_ref = workspace_ref.clone();
        let committed = committed.clone();
        entry.connect_has_focus_notify(move |entry| {
            if entry.has_focus() || committed.get() {
                return;
            }

            committed.set(true);
            commit_workspace_rename(&state, &workspace_ref, &entry.text());
        });
    }
}

fn commit_workspace_rename(state: &Rc<AppState>, current_workspace_ref: &str, next_name: &str) {
    let next_name = next_name.trim();
    *state.editing_workspace.borrow_mut() = None;

    if next_name.is_empty() {
        schedule_render_current_ui(state, Some(current_workspace_ref.to_string()), None);
        return;
    }

    match rename_workspace(current_workspace_ref, next_name) {
        Ok(workspace) => {
            let next_workspace_ref = workspace_ref(&workspace);
            clear_workspace_pr_status(state, current_workspace_ref);
            let mut next_groups = current_groups(state);
            for group in &mut next_groups {
                if group.repo_canonical != workspace.repo_canonical {
                    continue;
                }
                if let Some(candidate) = group
                    .workspaces
                    .iter_mut()
                    .find(|candidate| workspace_ref(candidate) == current_workspace_ref)
                {
                    *candidate = workspace;
                    break;
                }
            }
            *state.workspace_groups.borrow_mut() = next_groups.clone();
            *state.selected_workspace.borrow_mut() = Some(next_workspace_ref.clone());
            let remembered_session = state
                .selected_sessions
                .borrow_mut()
                .remove(current_workspace_ref);
            remember_selected_session(state, &next_workspace_ref, remembered_session.as_deref());
            schedule_render_current_ui(state, Some(next_workspace_ref), None);
        }
        Err(err) => {
            eprintln!("failed to rename workspace: {err}");
            *state.editing_workspace.borrow_mut() = Some(current_workspace_ref.to_string());
            schedule_render_current_ui(state, Some(current_workspace_ref.to_string()), None);
        }
    }
}

fn remove_selected_workspace(state: &Rc<AppState>, workspace: &WorkspaceEntry) {
    let removed_workspace_ref = workspace_ref(workspace);
    match remove_workspace(&removed_workspace_ref) {
        Ok(_) => {
            clear_workspace_pr_status(state, &removed_workspace_ref);
            let mut next_groups = state.workspace_groups.borrow().clone();
            for group in &mut next_groups {
                group
                    .workspaces
                    .retain(|candidate| workspace_ref(candidate) != removed_workspace_ref);
                group.workspace_count = group.workspaces.len();
            }

            let next_workspace = state
                .selected_workspace
                .borrow()
                .clone()
                .filter(|selected| selected != &removed_workspace_ref)
                .or_else(|| {
                    first_workspace(&next_groups).map(|workspace| workspace_ref(&workspace))
                });

            *state.workspace_groups.borrow_mut() = next_groups.clone();
            *state.selected_workspace.borrow_mut() = next_workspace.clone();
            *state.editing_workspace.borrow_mut() = None;
            state
                .selected_sessions
                .borrow_mut()
                .remove(&removed_workspace_ref);
            *state.selected_session.borrow_mut() = None;
            schedule_render_current_ui(state, next_workspace, None);
        }
        Err(err) => {
            eprintln!("failed to remove workspace: {err}");
            schedule_refresh(state, Some(removed_workspace_ref), None);
        }
    }
}

fn submit_repository_form(state: &Rc<AppState>, repository_entry: &Entry, alias_entry: &Entry) {
    let repository = repository_entry.text().trim().to_string();
    let alias_text = alias_entry.text().trim().to_string();
    let focused_field = if alias_entry.has_focus() {
        RepositoryFormField::Alias
    } else {
        RepositoryFormField::Repository
    };

    {
        let mut form = state.repository_form.borrow_mut();
        form.repository = repository.clone();
        form.alias = alias_text.clone();
        form.focused_field = focused_field;
        form.error = None;
    }

    let alias = (!alias_text.is_empty()).then_some(alias_text.as_str());
    let preferred_workspace = state.selected_workspace.borrow().clone();

    match add_repository(&repository, alias) {
        Ok(_) => {
            *state.repository_form.borrow_mut() = RepositoryFormState::default();
            schedule_refresh(state, preferred_workspace, None);
        }
        Err(err) => {
            state.repository_form.borrow_mut().error = Some(err.to_string());
            sync_repository_form_widgets(state, false);
            focus_repository_form_field(state);
        }
    }
}

struct WorkspaceRow {
    row: ListBoxRow,
    workspace: WorkspaceEntry,
    clone_button: Button,
    delete_button: Button,
}

pub(crate) fn clear_box(container: &GtkBox) {
    while let Some(child) = container.first_child() {
        container.remove(&child);
    }
}

fn install_css() {
    let provider = CssProvider::new();
    provider.load_from_string(STYLE);
    gtk::style_context_add_provider_for_display(
        &gdk::Display::default().expect("missing display"),
        &provider,
        STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
}
