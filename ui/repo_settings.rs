use gtk::{
    Align, Box as GtkBox, Button, EventControllerKey, Label, Orientation, PolicyType,
    ScrolledWindow, TextBuffer, TextView, WrapMode, gdk, glib, prelude::*,
};
use std::rc::Rc;

use crate::{
    data::{WorkspaceGroup, load_setup_script, save_setup_script},
    exec,
};

/// Builds the settings page of a repository.
pub fn build_repository_settings(group: &WorkspaceGroup) -> GtkBox {
    let page = GtkBox::new(Orientation::Vertical, 0);
    page.set_hexpand(true);
    page.set_vexpand(true);
    page.add_css_class("repo-settings");

    let title = Label::new(Some(&group.repo_label));
    title.set_xalign(0.0);
    title.add_css_class("repo-settings-title");
    page.append(&title);

    let subtitle = Label::new(Some(&group.repo_canonical));
    subtitle.set_xalign(0.0);
    subtitle.set_selectable(true);
    subtitle.add_css_class("repo-settings-subtitle");
    page.append(&subtitle);

    let heading = Label::new(Some("exe.dev setup script"));
    heading.set_xalign(0.0);
    heading.add_css_class("repo-settings-heading");
    page.append(&heading);

    let description = Label::new(Some(
        "Runs with bash in a \"setup\" session of every new exe.dev workspace, \
         from the repository checkout, after the repository is cloned. \
         Use it to install toolchains and system packages.",
    ));
    description.set_xalign(0.0);
    description.set_wrap(true);
    description.add_css_class("repo-settings-description");
    page.append(&description);

    let buffer = TextBuffer::new(None);
    let editor = TextView::with_buffer(&buffer);
    editor.set_monospace(true);
    editor.set_wrap_mode(WrapMode::None);
    editor.set_top_margin(10);
    editor.set_bottom_margin(10);
    editor.set_left_margin(12);
    editor.set_right_margin(12);
    editor.set_sensitive(false);
    editor.add_css_class("repo-settings-script");

    let scroller = ScrolledWindow::new();
    scroller.set_policy(PolicyType::Automatic, PolicyType::Automatic);
    scroller.set_vexpand(true);
    scroller.set_child(Some(&editor));
    scroller.add_css_class("repo-settings-script-frame");
    page.append(&scroller);

    let actions = GtkBox::new(Orientation::Horizontal, 10);
    actions.set_halign(Align::Start);
    actions.add_css_class("repo-settings-actions");

    let save_button = Button::with_label("Save");
    save_button.set_sensitive(false);
    save_button.set_tooltip_text(Some("Save (Ctrl+S)"));
    save_button.add_css_class("terminal-placeholder-action");

    let status = Label::new(Some("Loading ..."));
    status.add_css_class("repo-settings-status");

    actions.append(&save_button);
    actions.append(&status);
    page.append(&actions);

    let save: Rc<dyn Fn()> = {
        let repository = group.repo_canonical.clone();
        let buffer = buffer.clone();
        let save_button = save_button.clone();
        let status = status.clone();
        Rc::new(move || {
            if !save_button.is_sensitive() {
                return;
            }
            save_button.set_sensitive(false);
            status.set_text("Saving ...");
            let script = buffer
                .text(&buffer.start_iter(), &buffer.end_iter(), false)
                .to_string();
            let save_button = save_button.clone();
            let status = status.clone();
            exec::dispatch(
                save_setup_script(repository.clone(), script),
                move |result| match result {
                    Ok(()) => status.set_text("Saved"),
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
        load_setup_script(group.repo_canonical.clone()),
        move |result| match result {
            Ok(script) => {
                buffer.set_text(&script);
                status.set_text(if script.is_empty() {
                    "No setup script"
                } else {
                    ""
                });
                editor.set_sensitive(true);
                // Connect after loading so that filling the buffer does not count as an edit.
                buffer.connect_changed(move |_| {
                    save_button.set_sensitive(true);
                    status.set_text("Unsaved changes");
                });
            }
            Err(err) => status.set_text(&format!("Failed to load: {err}")),
        },
    );

    page
}
