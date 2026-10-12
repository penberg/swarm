use gtk::{
    Align, Box as GtkBox, Button, EventControllerKey, Image, Label, Orientation, PolicyType,
    Revealer, RevealerTransitionType, ScrolledWindow, Switch, TextBuffer, TextView, Widget,
    WrapMode, gdk, glib, prelude::*,
};
use splay::sandbox::SandboxKind;
use std::{cell::RefCell, rc::Rc};

use crate::{
    app::{AppState, apply_sandbox_enabled},
    data::{WorkspaceGroup, load_setup_script, save_setup_script, set_sandbox_enabled},
    exec,
};

/// Remote sandboxes a repository can enable, with how the page presents them.
const REMOTE_SANDBOXES: [(SandboxKind, &str, &str); 1] = [(
    SandboxKind::ExeDev,
    "exe.dev",
    "Remote workspaces, each in its own exe.dev VM.",
)];

/// The GitHub mark from Primer Octicons (MIT).
const GITHUB_MARK_SVG: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="64" height="64" viewBox="0 0 16 16"><path fill="#dfe6ef" d="M8 0c4.42 0 8 3.58 8 8a8.013 8.013 0 0 1-5.45 7.59c-.4.08-.55-.17-.55-.38 0-.27.01-1.13.01-2.2 0-.75-.25-1.23-.54-1.48 1.78-.2 3.65-.88 3.65-3.95 0-.88-.31-1.59-.82-2.15.08-.2.36-1.02-.08-2.12 0 0-.67-.22-2.2.82-.64-.18-1.32-.27-2-.27-.68 0-1.36.09-2 .27-1.53-1.03-2.2-.82-2.2-.82-.44 1.1-.16 1.92-.08 2.12-.51.56-.82 1.28-.82 2.15 0 3.06 1.86 3.75 3.64 3.95-.23.2-.44.55-.51 1.07-.46.21-1.61.55-2.33-.66-.15-.24-.6-.83-1.23-.82-.67.01-.27.38.01.53.34.19.73.9.82 1.13.16.45.68 1.31 2.69.94 0 .67.01 1.3.01 1.49 0 .21-.15.45-.55.38A7.995 7.995 0 0 1 0 8c0-4.42 3.58-8 8-8Z"/></svg>"##;

/// Builds the settings page of a repository.
pub fn build_repository_settings(state: &Rc<AppState>, group: &WorkspaceGroup) -> GtkBox {
    let page = GtkBox::new(Orientation::Vertical, 0);
    page.set_hexpand(true);
    page.set_vexpand(true);
    page.add_css_class("repo-settings");

    let title = Label::new(Some(&group.repo_label.to_uppercase()));
    title.set_xalign(0.0);
    title.add_css_class("repo-settings-title");
    page.append(&title);

    let subtitle = Label::new(Some(&group.repo_canonical));
    subtitle.set_xalign(0.0);
    subtitle.set_selectable(true);
    subtitle.add_css_class("repo-settings-subtitle");
    page.append(&subtitle);

    let setup = build_setup_script_section(&group.repo_canonical);
    let setup_revealer = Revealer::new();
    setup_revealer.set_transition_type(RevealerTransitionType::SlideDown);
    setup_revealer.set_transition_duration(180);
    setup_revealer.set_vexpand(true);
    setup_revealer.set_child(Some(&setup));
    setup_revealer.set_reveal_child(!group.remote_sandboxes.is_empty());

    page.append(&section_heading("Integrations"));
    page.append(&build_integrations_card(state, group, &setup_revealer));
    page.append(&setup_revealer);

    page
}

fn build_integrations_card(
    state: &Rc<AppState>,
    group: &WorkspaceGroup,
    setup_revealer: &Revealer,
) -> GtkBox {
    let card = GtkBox::new(Orientation::Vertical, 0);
    card.add_css_class("settings-card");

    let always_on = Label::new(Some("Always on"));
    always_on.set_valign(Align::Center);
    always_on.add_css_class("settings-pill");
    card.append(&integration_row(
        github_mark(),
        "GitHub",
        "Hosts the repository. Pull request status and creation.",
        &always_on,
    ));

    let enabled = Rc::new(RefCell::new(group.remote_sandboxes.clone()));
    for (sandbox, name, description) in REMOTE_SANDBOXES {
        let toggle = Switch::new();
        toggle.set_valign(Align::Center);
        toggle.set_active(group.remote_sandboxes.contains(&sandbox));

        let icon = Image::from_icon_name("weather-overcast-symbolic");
        icon.add_css_class("settings-tile-remote");
        let row = integration_row(icon.upcast(), name, description, &toggle);
        row.add_css_class("settings-row-divided");
        card.append(&row);

        let state = state.clone();
        let repo_canonical = group.repo_canonical.clone();
        let enabled = enabled.clone();
        let setup_revealer = setup_revealer.clone();
        toggle.connect_state_set(move |toggle, active| {
            // The switch shows `active` right away; its state follows once saved.
            let toggle = toggle.clone();
            let state = state.clone();
            let repo_canonical = repo_canonical.clone();
            let enabled = enabled.clone();
            let setup_revealer = setup_revealer.clone();
            exec::dispatch(
                set_sandbox_enabled(repo_canonical.clone(), sandbox, active),
                move |result| match result {
                    Ok(()) => {
                        toggle.set_state(active);
                        let mut enabled = enabled.borrow_mut();
                        enabled.retain(|kind| *kind != sandbox);
                        if active {
                            enabled.push(sandbox);
                        }
                        setup_revealer.set_reveal_child(!enabled.is_empty());
                        apply_sandbox_enabled(&state, &repo_canonical, sandbox, active);
                    }
                    Err(err) => {
                        eprintln!("failed to update {sandbox} for {repo_canonical}: {err}");
                        toggle.set_active(toggle.state());
                    }
                },
            );
            glib::Propagation::Stop
        });
    }

    card
}

fn integration_row(
    icon: Widget,
    name: &str,
    description: &str,
    trailing: &impl IsA<Widget>,
) -> GtkBox {
    let row = GtkBox::new(Orientation::Horizontal, 14);
    row.add_css_class("settings-row");

    let tile = GtkBox::new(Orientation::Horizontal, 0);
    tile.set_valign(Align::Center);
    // Keep the centered icon's expansion from stretching the tile.
    tile.set_hexpand(false);
    tile.add_css_class("settings-tile");
    icon.set_halign(Align::Center);
    icon.set_valign(Align::Center);
    icon.set_hexpand(true);
    if let Some(image) = icon.downcast_ref::<Image>() {
        image.set_pixel_size(18);
    }
    tile.append(&icon);
    row.append(&tile);

    let text = GtkBox::new(Orientation::Vertical, 2);
    text.set_hexpand(true);
    text.set_valign(Align::Center);
    let name_label = Label::new(Some(name));
    name_label.set_xalign(0.0);
    name_label.add_css_class("settings-row-title");
    let description_label = Label::new(Some(description));
    description_label.set_xalign(0.0);
    description_label.set_wrap(true);
    description_label.add_css_class("settings-row-description");
    text.append(&name_label);
    text.append(&description_label);
    row.append(&text);

    row.append(trailing);
    row
}

fn github_mark() -> Widget {
    let bytes = glib::Bytes::from_static(GITHUB_MARK_SVG.as_bytes());
    match gdk::Texture::from_bytes(&bytes) {
        Ok(texture) => Image::from_paintable(Some(&texture)).upcast(),
        Err(_) => Image::from_icon_name("folder-remote-symbolic").upcast(),
    }
}

fn section_heading(text: &str) -> Label {
    let heading = Label::new(Some(&text.to_uppercase()));
    heading.set_xalign(0.0);
    heading.add_css_class("settings-section-heading");
    heading
}

fn buffer_text(buffer: &TextBuffer) -> String {
    buffer
        .text(&buffer.start_iter(), &buffer.end_iter(), false)
        .to_string()
}

fn build_setup_script_section(repository: &str) -> GtkBox {
    let section = GtkBox::new(Orientation::Vertical, 0);
    section.set_vexpand(true);

    section.append(&section_heading("Sandbox setup script"));

    let description = Label::new(Some(
        "Runs with bash in every new remote workspace, from the repository \
         checkout, before the workspace's shell starts. Use it to install \
         toolchains and system packages.",
    ));
    description.set_xalign(0.0);
    description.set_wrap(true);
    description.add_css_class("settings-section-description");
    section.append(&description);

    let card = GtkBox::new(Orientation::Vertical, 0);
    card.set_vexpand(true);
    card.add_css_class("settings-card");
    card.add_css_class("settings-editor-card");

    let header = GtkBox::new(Orientation::Horizontal, 8);
    header.add_css_class("settings-editor-header");
    let language = Label::new(Some("bash"));
    language.set_hexpand(true);
    language.set_xalign(0.0);
    language.add_css_class("settings-editor-language");
    let status = Label::new(Some("Loading ..."));
    status.add_css_class("settings-editor-status");
    header.append(&language);
    header.append(&status);
    card.append(&header);

    let buffer = TextBuffer::new(None);
    let editor = TextView::with_buffer(&buffer);
    editor.set_monospace(true);
    editor.set_wrap_mode(WrapMode::None);
    editor.set_top_margin(12);
    editor.set_bottom_margin(12);
    editor.set_left_margin(14);
    editor.set_right_margin(14);
    editor.set_sensitive(false);
    editor.add_css_class("settings-editor");

    let scroller = ScrolledWindow::new();
    scroller.set_policy(PolicyType::Automatic, PolicyType::Automatic);
    scroller.set_vexpand(true);
    scroller.set_min_content_height(180);
    scroller.set_child(Some(&editor));
    card.append(&scroller);

    let footer = GtkBox::new(Orientation::Horizontal, 10);
    footer.add_css_class("settings-editor-footer");
    let hint = Label::new(Some("Ctrl+S to save"));
    hint.set_hexpand(true);
    hint.set_xalign(0.0);
    hint.add_css_class("settings-editor-hint");
    let discard_button = Button::with_label("Discard");
    discard_button.set_sensitive(false);
    discard_button.set_tooltip_text(Some("Revert to the saved script"));
    discard_button.add_css_class("settings-discard");
    let save_button = Button::with_label("Save");
    save_button.set_sensitive(false);
    save_button.add_css_class("settings-save");
    footer.append(&hint);
    footer.append(&discard_button);
    footer.append(&save_button);
    card.append(&footer);

    section.append(&card);

    // The script as last loaded or saved, which Discard reverts to.
    let saved_script = Rc::new(RefCell::new(String::new()));
    let mark_saved: Rc<dyn Fn()> = {
        let buffer = buffer.clone();
        let saved_script = saved_script.clone();
        let save_button = save_button.clone();
        let discard_button = discard_button.clone();
        let status = status.clone();
        Rc::new(move || {
            if buffer_text(&buffer) != *saved_script.borrow() {
                return;
            }
            save_button.set_sensitive(false);
            discard_button.set_sensitive(false);
            status.set_text(if saved_script.borrow().is_empty() {
                "Empty"
            } else {
                "Saved"
            });
        })
    };

    let save: Rc<dyn Fn()> = {
        let repository = repository.to_string();
        let buffer = buffer.clone();
        let saved_script = saved_script.clone();
        let save_button = save_button.clone();
        let status = status.clone();
        let mark_saved = mark_saved.clone();
        Rc::new(move || {
            if !save_button.is_sensitive() {
                return;
            }
            save_button.set_sensitive(false);
            status.set_text("Saving ...");
            let script = buffer_text(&buffer);
            let saved_script = saved_script.clone();
            let save_button = save_button.clone();
            let status = status.clone();
            let mark_saved = mark_saved.clone();
            exec::dispatch(
                save_setup_script(repository.clone(), script.clone()),
                move |result| match result {
                    Ok(()) => {
                        *saved_script.borrow_mut() = script;
                        // Edits made while saving stay unsaved.
                        mark_saved();
                    }
                    Err(err) => {
                        status.set_text(&format!("Failed to save: {err}"));
                        save_button.set_sensitive(true);
                    }
                },
            );
        })
    };

    {
        let save = save.clone();
        save_button.connect_clicked(move |_| save());
    }

    {
        let buffer = buffer.clone();
        let saved_script = saved_script.clone();
        let mark_saved = mark_saved.clone();
        discard_button.connect_clicked(move |_| {
            let script = saved_script.borrow().clone();
            buffer.set_text(&script);
            mark_saved();
        });
    }

    let keys = EventControllerKey::new();
    keys.connect_key_pressed(move |_, key, _, modifiers| {
        if modifiers.contains(gdk::ModifierType::CONTROL_MASK)
            && matches!(key, gdk::Key::s | gdk::Key::S)
        {
            save();
            return glib::Propagation::Stop;
        }
        glib::Propagation::Proceed
    });
    editor.add_controller(keys);

    exec::dispatch(
        load_setup_script(repository.to_string()),
        move |result| match result {
            Ok(script) => {
                buffer.set_text(&script);
                *saved_script.borrow_mut() = script;
                mark_saved();
                editor.set_sensitive(true);
                // Connect after loading so that filling the buffer does not count as an edit.
                buffer.connect_changed(move |_| {
                    save_button.set_sensitive(true);
                    discard_button.set_sensitive(true);
                    status.set_text("Unsaved changes");
                });
            }
            Err(err) => status.set_text(&format!("Failed to load: {err}")),
        },
    );

    section
}
