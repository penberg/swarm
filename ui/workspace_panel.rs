use gtk::{
    glib, prelude::*, Align, Box as GtkBox, Button, Image, Label, LinkButton, Orientation, Stack,
};
use std::{
    cell::RefCell,
    collections::HashMap,
    path::Path,
    rc::Rc,
    sync::{mpsc, Arc, Mutex},
    time::Duration,
};
use swarm::forges::github::{self, PullRequestStatus};

use crate::{
    app::{
        clear_box, clear_workspace_pr_status, current_groups, preferred_session_for_workspace,
        remember_selected_session, render_selected_workspace_detail, request_workspace_pr_status,
        schedule_render_current_ui, workspace_pr_snapshot, workspace_ref, AppState,
        WorkspacePrState,
    },
    data::{close_session, create_session, foreground_program, SessionEntry, WorkspaceEntry},
    ghostty,
};

#[derive(Clone)]
pub struct DetailWidgets {
    pub container: GtkBox,
    session_toolbar: GtkBox,
    session_tabs: GtkBox,
    session_toolbar_spacer: GtkBox,
    pub session_stack: Stack,
    terminal_cache: Rc<RefCell<HashMap<String, GtkBox>>>,
    tracked_sessions: TrackedSessions,
}

/// Sessions whose foreground program the refresh worker polls, as
/// `(session id, pid, fallback program)`.
type TrackedSessions = Arc<Mutex<Vec<(String, Option<u32>, String)>>>;

impl DetailWidgets {
    pub fn new(state: &Rc<AppState>) -> Self {
        let container = GtkBox::new(Orientation::Vertical, 0);
        container.set_hexpand(true);
        container.set_vexpand(true);

        let session_toolbar = GtkBox::new(Orientation::Horizontal, 8);
        session_toolbar.set_halign(Align::Fill);
        session_toolbar.set_hexpand(true);
        session_toolbar.add_css_class("session-toolbar");

        let session_stack = Stack::new();
        session_stack.set_hexpand(true);
        session_stack.set_vexpand(true);

        let session_tabs = GtkBox::new(Orientation::Horizontal, 6);
        session_tabs.set_halign(Align::Start);
        session_tabs.add_css_class("session-tabs");

        let session_toolbar_spacer = GtkBox::new(Orientation::Horizontal, 0);
        session_toolbar_spacer.set_hexpand(true);
        session_toolbar_spacer.add_css_class("session-toolbar-spacer");

        {
            let state = state.clone();
            let session_tabs = session_tabs.clone();
            session_stack.connect_visible_child_name_notify(move |stack| {
                let selected_session = stack.visible_child_name().map(|name| name.to_string());
                if let Some(workspace_ref) = state.selected_workspace.borrow().clone() {
                    remember_selected_session(&state, &workspace_ref, selected_session.as_deref());
                } else {
                    *state.selected_session.borrow_mut() = selected_session.clone();
                }
                sync_session_tab_active_state(&session_tabs, selected_session.as_deref());
                focus_visible_terminal(stack);
            });
        }

        session_toolbar.append(&session_tabs);
        container.append(&session_toolbar);
        container.append(&session_stack);

        let tracked_sessions: TrackedSessions = Arc::new(Mutex::new(Vec::new()));
        install_session_program_refresh(&session_tabs, tracked_sessions.clone());

        Self {
            container,
            session_toolbar,
            session_tabs,
            session_toolbar_spacer,
            session_stack,
            terminal_cache: Rc::new(RefCell::new(HashMap::new())),
            tracked_sessions,
        }
    }

    pub fn render_empty(&self) {
        clear_box(&self.session_toolbar);
        clear_stack(&self.session_stack);
        self.track_session_programs(&[]);

        let placeholder = build_no_workspace_placeholder();
        self.session_stack
            .add_titled(&placeholder, Some("empty"), "empty");
    }

    pub fn render_workspace(
        &self,
        workspace: &WorkspaceEntry,
        state: &Rc<AppState>,
        preferred_session: Option<&str>,
    ) {
        // Fast path: if the session set already matches what's rendered,
        // avoid clearing the stack. Removing and re-adding the terminal
        // DrawingArea drops keyboard focus on every periodic refresh (e.g.
        // PR status polling), which causes the active terminal to lose
        // focus every few seconds.
        if self.refresh_in_place(workspace, state, preferred_session) {
            return;
        }

        clear_box(&self.session_toolbar);
        clear_stack(&self.session_stack);

        clear_box(&self.session_tabs);
        self.session_toolbar.append(&self.session_tabs);

        let add_button = Button::with_label("+");
        add_button.set_valign(Align::Center);
        add_button.add_css_class("session-add");
        {
            let state = state.clone();
            let workspace_ref = workspace_ref(workspace);
            add_button.connect_clicked(move |_| {
                create_and_select_session(&state, &workspace_ref);
            });
        }
        self.session_toolbar.append(&add_button);
        self.session_toolbar.append(&self.session_toolbar_spacer);

        match workspace_pr_snapshot(state, workspace) {
            WorkspacePrState::Ready(status) => {
                if let Some(link) = build_workspace_pr_link(&status) {
                    self.session_toolbar.append(&link);
                }
            }
            WorkspacePrState::None => {
                if let Some(button) = build_workspace_create_pr_button(state, workspace) {
                    self.session_toolbar.append(&button);
                }
            }
            WorkspacePrState::Loading => {}
        }

        if workspace.sessions.is_empty() {
            self.track_session_programs(&[]);
            let empty = build_empty_session_placeholder(state, workspace);
            self.session_stack
                .add_titled(&empty, Some("empty"), "empty");
            return;
        }

        let mut cache = self.terminal_cache.borrow_mut();
        for session in &workspace.sessions {
            let host = cache
                .entry(session.id.clone())
                .or_insert_with(|| ghostty::terminal_host(session));
            self.session_stack
                .add_titled(host, Some(&session.id), &session.program);
        }
        drop(cache);

        let workspace_ref = workspace_ref(workspace);
        let selected_session =
            preferred_session_for_workspace(state, &workspace_ref, preferred_session)
                .filter(|selected| {
                    workspace
                        .sessions
                        .iter()
                        .any(|session| session.id == *selected)
                })
                .unwrap_or_else(|| workspace.sessions[0].id.clone());
        remember_selected_session(state, &workspace_ref, Some(&selected_session));
        self.session_stack.set_visible_child_name(&selected_session);

        let session_ids = workspace
            .sessions
            .iter()
            .map(|session| session.id.clone())
            .collect::<Vec<_>>();
        for session in &workspace.sessions {
            let tab = build_session_tab(
                state,
                &self.session_tabs,
                &workspace_ref,
                &session_ids,
                session,
                session.id == selected_session,
                &self.session_stack,
            );
            self.session_tabs.append(&tab);
        }
        sync_session_tab_active_state(&self.session_tabs, Some(&selected_session));
        self.track_session_programs(&workspace.sessions);
    }

    fn track_session_programs(&self, sessions: &[SessionEntry]) {
        let Ok(mut tracked) = self.tracked_sessions.lock() else {
            return;
        };

        *tracked = sessions
            .iter()
            .map(|session| (session.id.clone(), session.pid, session.program.clone()))
            .collect();
    }

    pub fn evict_terminals<'a>(&self, session_ids: impl IntoIterator<Item = &'a str>) {
        let mut cache = self.terminal_cache.borrow_mut();
        for session_id in session_ids {
            cache.remove(session_id);
        }
    }

    fn refresh_in_place(
        &self,
        workspace: &WorkspaceEntry,
        state: &Rc<AppState>,
        preferred_session: Option<&str>,
    ) -> bool {
        if workspace.sessions.is_empty() {
            return false;
        }

        let mut stack_child_count = 0usize;
        let mut child = self.session_stack.first_child();
        while let Some(widget) = child {
            stack_child_count += 1;
            child = widget.next_sibling();
        }
        if stack_child_count != workspace.sessions.len() {
            return false;
        }

        for session in &workspace.sessions {
            if self.session_stack.child_by_name(&session.id).is_none() {
                return false;
            }
        }

        self.refresh_pr_link(workspace, state);

        let workspace_ref = workspace_ref(workspace);
        if let Some(requested) = preferred_session {
            if workspace.sessions.iter().any(|s| s.id == requested) {
                remember_selected_session(state, &workspace_ref, Some(requested));
                let currently_visible = self
                    .session_stack
                    .visible_child_name()
                    .map(|name| name.to_string());
                if currently_visible.as_deref() != Some(requested) {
                    self.session_stack.set_visible_child_name(requested);
                }
            }
        }

        true
    }

    fn refresh_pr_link(&self, workspace: &WorkspaceEntry, state: &Rc<AppState>) {
        let mut child = self.session_toolbar.first_child();
        while let Some(widget) = child {
            let next = widget.next_sibling();
            if widget.has_css_class("session-pr-link") || widget.has_css_class("session-pr-create")
            {
                self.session_toolbar.remove(&widget);
            }
            child = next;
        }

        match workspace_pr_snapshot(state, workspace) {
            WorkspacePrState::Ready(status) => {
                if let Some(link) = build_workspace_pr_link(&status) {
                    self.session_toolbar.append(&link);
                }
            }
            WorkspacePrState::None => {
                if let Some(button) = build_workspace_create_pr_button(state, workspace) {
                    self.session_toolbar.append(&button);
                }
            }
            WorkspacePrState::Loading => {}
        }
    }
}

fn build_placeholder(title: &str, subtitle: &str) -> GtkBox {
    let container = GtkBox::new(Orientation::Vertical, 0);
    container.set_hexpand(true);
    container.set_vexpand(true);
    container.set_halign(Align::Center);
    container.set_valign(Align::Center);
    container.add_css_class("terminal-placeholder");

    let icon = Image::from_icon_name("utilities-terminal-symbolic");
    icon.set_pixel_size(28);
    icon.set_halign(Align::Center);
    icon.add_css_class("terminal-placeholder-icon");

    let title_label = Label::new(Some(title));
    title_label.add_css_class("terminal-placeholder-title");

    let subtitle_label = Label::new(Some(subtitle));
    subtitle_label.add_css_class("terminal-placeholder-subtitle");

    container.append(&icon);
    container.append(&title_label);
    container.append(&subtitle_label);
    container
}

fn build_empty_session_placeholder(state: &Rc<AppState>, workspace: &WorkspaceEntry) -> GtkBox {
    let label = if workspace.branch.is_empty() {
        workspace.name.as_str()
    } else {
        workspace.branch.as_str()
    };
    let container = build_placeholder("No terminal open", &format!("Start a terminal in {label}."));

    let new_terminal = Button::new();
    new_terminal.set_halign(Align::Center);
    new_terminal.add_css_class("terminal-placeholder-action");

    let button_content = GtkBox::new(Orientation::Horizontal, 8);
    button_content.set_halign(Align::Center);
    let button_icon = Image::from_icon_name("utilities-terminal-symbolic");
    button_icon.set_pixel_size(14);
    button_content.append(&button_icon);
    button_content.append(&Label::new(Some("New Terminal")));
    new_terminal.set_child(Some(&button_content));

    {
        let state = state.clone();
        let workspace_id = workspace_ref(workspace);
        new_terminal.connect_clicked(move |_| {
            create_and_select_session(&state, &workspace_id);
        });
    }

    container.append(&new_terminal);
    container
}

fn build_no_workspace_placeholder() -> GtkBox {
    build_placeholder(
        "No workspace selected",
        "Pick a workspace in the sidebar to open a terminal.",
    )
}

fn build_workspace_pr_link(status: &PullRequestStatus) -> Option<LinkButton> {
    let url = status.url.as_deref()?;
    let link = LinkButton::builder()
        .uri(url)
        .label(format!("PR #{}", status.number))
        .halign(Align::End)
        .valign(Align::Center)
        .build();
    link.add_css_class("session-pr-link");
    link.set_tooltip_text(Some(url));
    Some(link)
}

fn build_workspace_create_pr_button(
    state: &Rc<AppState>,
    workspace: &WorkspaceEntry,
) -> Option<Button> {
    let workspace_path = Path::new(&workspace.path);
    if !github::is_github_workspace(workspace_path) {
        return None;
    }

    let workspace_id = workspace_ref(workspace);
    let in_flight = state
        .creating_pr_workspaces
        .borrow()
        .contains(&workspace_id);
    let commits_ahead = github::workspace_commits_ahead(workspace_path);

    let (label, tooltip, enabled) = if in_flight {
        ("Creating PR…", "Creating pull request".to_string(), false)
    } else {
        match commits_ahead {
            Some(0) => (
                "Create PR",
                "No commits ahead of base branch".to_string(),
                false,
            ),
            Some(n) => (
                "Create PR",
                format!(
                    "Create pull request ({n} commit{} ahead)",
                    if n == 1 { "" } else { "s" }
                ),
                true,
            ),
            None => (
                "Create PR",
                "Unable to determine branch status".to_string(),
                false,
            ),
        }
    };

    let button = Button::with_label(label);
    button.set_halign(Align::End);
    button.set_valign(Align::Center);
    button.add_css_class("session-pr-create");
    button.set_tooltip_text(Some(&tooltip));
    button.set_sensitive(enabled);

    if enabled {
        let state = state.clone();
        let workspace = workspace.clone();
        button.connect_clicked(move |_| {
            start_pr_creation(&state, &workspace);
        });
    }

    Some(button)
}

fn start_pr_creation(state: &Rc<AppState>, workspace: &WorkspaceEntry) {
    let workspace_id = workspace_ref(workspace);
    {
        let mut in_flight = state.creating_pr_workspaces.borrow_mut();
        if in_flight.contains(&workspace_id) {
            return;
        }
        in_flight.insert(workspace_id.clone());
    }

    if let Some(selected) = state.selected_workspace.borrow().clone() {
        if selected == workspace_id {
            render_selected_workspace_detail(state, &selected, None);
        }
    }

    let workspace_path = workspace.path.clone();
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = sender.send(github::create_pull_request(Path::new(&workspace_path)));
    });

    let state_for_completion = state.clone();
    let workspace_id_for_completion = workspace_id.clone();
    let workspace_for_completion = workspace.clone();
    glib::timeout_add_local(Duration::from_millis(50), move || {
        match receiver.try_recv() {
            Ok(result) => {
                state_for_completion
                    .creating_pr_workspaces
                    .borrow_mut()
                    .remove(&workspace_id_for_completion);

                if let Err(err) = &result {
                    eprintln!(
                        "failed to create pull request for {workspace_id_for_completion}: {err}"
                    );
                }

                clear_workspace_pr_status(&state_for_completion, &workspace_id_for_completion);
                request_workspace_pr_status(&state_for_completion, &workspace_for_completion, true);

                if let Some(selected) = state_for_completion.selected_workspace.borrow().clone() {
                    if selected == workspace_id_for_completion {
                        render_selected_workspace_detail(&state_for_completion, &selected, None);
                    }
                }

                glib::ControlFlow::Break
            }
            Err(mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
            Err(mpsc::TryRecvError::Disconnected) => {
                state_for_completion
                    .creating_pr_workspaces
                    .borrow_mut()
                    .remove(&workspace_id_for_completion);
                eprintln!(
                    "failed to create pull request for {workspace_id_for_completion}: worker disconnected"
                );

                if let Some(selected) = state_for_completion.selected_workspace.borrow().clone() {
                    if selected == workspace_id_for_completion {
                        render_selected_workspace_detail(&state_for_completion, &selected, None);
                    }
                }

                glib::ControlFlow::Break
            }
        }
    });
}

fn clear_stack(stack: &Stack) {
    while let Some(child) = stack.first_child() {
        stack.remove(&child);
    }
}

fn sync_session_tab_active_state(session_tabs: &GtkBox, selected_session: Option<&str>) {
    let mut child = session_tabs.first_child();
    while let Some(widget) = child {
        let next = widget.next_sibling();
        if widget.has_css_class("session-tab") {
            let is_active =
                selected_session.is_some_and(|selected| widget.widget_name() == selected);
            if is_active {
                widget.add_css_class("session-tab-active");
            } else {
                widget.remove_css_class("session-tab-active");
            }
        }
        child = next;
    }
}

fn focus_visible_terminal(session_stack: &Stack) {
    let Some(host) = session_stack.visible_child() else {
        return;
    };

    glib::idle_add_local_once(move || {
        if let Some(area) = host.first_child() {
            area.queue_draw();
            area.grab_focus();
        } else {
            host.grab_focus();
        }
    });
}

fn build_session_tab(
    state: &Rc<AppState>,
    session_tabs: &GtkBox,
    workspace_ref: &str,
    session_ids: &[String],
    session: &SessionEntry,
    active: bool,
    session_stack: &Stack,
) -> GtkBox {
    let tab = GtkBox::new(Orientation::Horizontal, 0);
    tab.add_css_class("session-tab");
    tab.set_widget_name(&session.id);
    tab.set_tooltip_text(Some(&session.id));
    if active {
        tab.add_css_class("session-tab-active");
    }

    let select_button = Button::with_label(&session.program);
    select_button.set_tooltip_text(Some(&session.id));
    select_button.set_valign(Align::Center);
    select_button.add_css_class("session-tab-select");
    {
        let state = state.clone();
        let workspace_ref = workspace_ref.to_string();
        let session_id = session.id.clone();
        let session_tabs = session_tabs.clone();
        let session_stack = session_stack.clone();
        select_button.connect_clicked(move |_| {
            remember_selected_session(&state, &workspace_ref, Some(&session_id));
            session_stack.set_visible_child_name(&session_id);
            sync_session_tab_active_state(&session_tabs, Some(&session_id));
        });
    }

    let close_button = Button::with_label("X");
    close_button.set_focus_on_click(false);
    close_button.set_valign(Align::Center);
    close_button.add_css_class("session-close");
    {
        let state = state.clone();
        let workspace_ref = workspace_ref.to_string();
        let session_ids = session_ids.to_vec();
        let session_id = session.id.clone();
        close_button.connect_clicked(move |_| {
            close_specific_session(&state, &workspace_ref, &session_ids, &session_id);
        });
    }

    tab.append(&select_button);
    tab.append(&close_button);
    tab
}

fn install_session_program_refresh(session_tabs: &GtkBox, tracked: TrackedSessions) {
    let session_tabs = session_tabs.downgrade();
    let (tx, rx) = mpsc::channel::<Vec<(String, String)>>();

    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_secs(1));
        let Ok(sessions) = tracked.lock().map(|sessions| sessions.clone()) else {
            break;
        };
        if sessions.is_empty() {
            continue;
        }

        let programs: Vec<(String, String)> = sessions
            .iter()
            .map(|(id, pid, fallback)| {
                let program = foreground_program(*pid).unwrap_or_else(|| fallback.clone());
                (id.clone(), program)
            })
            .collect();
        if tx.send(programs).is_err() {
            break;
        }
    });

    glib::timeout_add_local(Duration::from_millis(250), move || {
        let Some(session_tabs) = session_tabs.upgrade() else {
            return glib::ControlFlow::Break;
        };
        let mut latest = None;
        while let Ok(programs) = rx.try_recv() {
            latest = Some(programs);
        }
        if let Some(programs) = latest {
            sync_session_tab_labels(&session_tabs, &programs);
        }
        glib::ControlFlow::Continue
    });
}

fn sync_session_tab_labels(session_tabs: &GtkBox, programs: &[(String, String)]) {
    let mut child = session_tabs.first_child();
    while let Some(widget) = child {
        let next = widget.next_sibling();
        let session_id = widget.widget_name().to_string();
        if let Some((_, program)) = programs.iter().find(|(id, _)| id == &session_id) {
            if let Some(button) = widget
                .first_child()
                .and_then(|child| child.downcast::<Button>().ok())
            {
                if button.label().as_deref() != Some(program.as_str()) {
                    button.set_label(program);
                }
            }
        }
        child = next;
    }
}

fn create_and_select_session(state: &Rc<AppState>, workspace_id: &str) {
    match create_session(workspace_id) {
        Ok(session) => {
            let mut next_groups = current_groups(state);
            for group in &mut next_groups {
                if let Some(workspace) = group
                    .workspaces
                    .iter_mut()
                    .find(|workspace| workspace_ref(workspace) == workspace_id)
                {
                    workspace.sessions.push(session.clone());
                    break;
                }
            }
            *state.workspace_groups.borrow_mut() = next_groups.clone();
            *state.selected_workspace.borrow_mut() = Some(workspace_id.to_string());
            remember_selected_session(state, workspace_id, Some(&session.id));
            schedule_render_current_ui(state, Some(workspace_id.to_string()), Some(session.id));
        }
        Err(err) => {
            eprintln!("failed to create session: {err}");
        }
    }
}

fn close_specific_session(
    state: &Rc<AppState>,
    workspace_id: &str,
    session_ids: &[String],
    session_id: &str,
) {
    let next_selected = session_ids
        .iter()
        .find(|id| id.as_str() != session_id)
        .cloned();

    match close_session(session_id) {
        Ok(_) => {
            if let Some(detail_widgets) = state.detail_widgets.borrow().as_ref() {
                detail_widgets.evict_terminals([session_id]);
            }
            let preferred_session = next_selected.clone();
            let mut next_groups = current_groups(state);
            for group in &mut next_groups {
                if let Some(workspace) = group
                    .workspaces
                    .iter_mut()
                    .find(|workspace| workspace_ref(workspace) == workspace_id)
                {
                    workspace
                        .sessions
                        .retain(|session| session.id != session_id);
                    break;
                }
            }
            *state.workspace_groups.borrow_mut() = next_groups.clone();
            *state.selected_workspace.borrow_mut() = Some(workspace_id.to_string());
            remember_selected_session(state, workspace_id, next_selected.as_deref());
            schedule_render_current_ui(state, Some(workspace_id.to_string()), preferred_session);
        }
        Err(err) => {
            eprintln!("failed to close session: {err}");
        }
    }
}
