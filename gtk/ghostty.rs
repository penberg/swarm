use gtk::{
    Adjustment, Box as GtkBox, DrawingArea, EventControllerKey, EventControllerScroll,
    EventControllerScrollFlags, GestureClick, GestureDrag, IMMulticontext, Orientation,
    PopoverMenu, Scrollbar, gdk, gio, glib, pango, prelude::*,
};
use libghostty_vt::{
    RenderState, Terminal, TerminalOptions, ffi,
    render::{CellIterator, CursorVisualStyle, RowIterator},
    style::{RgbColor, Style},
    terminal::ScrollViewport,
};
use pangocairo::functions::{create_layout, show_layout};
use std::{
    cell::RefCell,
    f64,
    fmt::Write as _,
    fs::{self, File},
    io::{self, Read, Write},
    os::unix::{fs::OpenOptionsExt, io::AsRawFd, net::UnixStream},
    path::Path,
    rc::Rc,
    time::Duration,
};

use crate::data::SessionEntry;

const DEFAULT_COLS: u16 = 120;
const DEFAULT_ROWS: u16 = 40;
const MAX_SCROLLBACK: usize = 20_000;
const FONT_SIZE: f64 = 10.0;
const FONT_FAMILIES: [&str; 6] = [
    "JetBrainsMono Nerd Font",
    "JetBrains Mono",
    "Iosevka Term",
    "SF Mono",
    "Menlo",
    "Monospace",
];
const SCROLL_LINES_PER_TICK: isize = 3;
const HORIZONTAL_PADDING: f64 = 18.0;
const VERTICAL_PADDING: f64 = 18.0;

const THEME_BACKGROUND: RgbColor = rgb(0x10, 0x14, 0x1c);
const THEME_FOREGROUND: RgbColor = rgb(0xe6, 0xed, 0xf3);
const THEME_CURSOR: RgbColor = rgb(0x8b, 0xc2, 0xff);
const THEME_PALETTE: [RgbColor; 16] = [
    rgb(0x22, 0x28, 0x33),
    rgb(0xf0, 0x71, 0x78),
    rgb(0xa6, 0xda, 0x95),
    rgb(0xe6, 0xc3, 0x84),
    rgb(0x7d, 0xb7, 0xff),
    rgb(0xc6, 0xa0, 0xf6),
    rgb(0x6c, 0xd4, 0xe3),
    rgb(0xd8, 0xdf, 0xe7),
    rgb(0x55, 0x61, 0x73),
    rgb(0xff, 0x8b, 0x92),
    rgb(0xb8, 0xf2, 0xa7),
    rgb(0xf4, 0xd1, 0x9a),
    rgb(0x98, 0xc7, 0xff),
    rgb(0xd7, 0xb7, 0xff),
    rgb(0x8c, 0xe5, 0xf0),
    rgb(0xf4, 0xf8, 0xfc),
];

pub fn terminal_host(session: &SessionEntry) -> GtkBox {
    let area = DrawingArea::new();
    area.set_hexpand(true);
    area.set_vexpand(true);
    area.set_focusable(true);
    area.add_css_class("terminal-view");

    let adjustment = Adjustment::new(0.0, 0.0, 0.0, 1.0, 8.0, 0.0);
    let scrollbar = Scrollbar::new(Orientation::Vertical, Some(&adjustment));
    scrollbar.set_vexpand(true);

    let container = GtkBox::new(Orientation::Horizontal, 0);
    container.set_hexpand(true);
    container.set_vexpand(true);
    container.set_focusable(true);
    container.add_css_class("terminal-host");
    container.append(&area);
    container.append(&scrollbar);

    let state = SessionTerminalState::new(
        session.clone(),
        area.clone(),
        adjustment.clone(),
        scrollbar.clone(),
    );
    state.borrow_mut().refresh_view();

    install_draw_func(&area, state.clone());
    install_key_controller(&area, state.clone());
    install_scroll_controller(&area, state.clone());
    install_focus_controller(&area, state.clone());
    install_drag_controller(&area, state.clone());
    install_context_menu(&area, state.clone());
    install_scrollbar_sync(&adjustment, state.clone());
    install_socket_pump(state);

    {
        let area = area.clone();
        glib::idle_add_local_once(move || {
            area.grab_focus();
        });
    }
    container
}

fn install_draw_func(area: &DrawingArea, state: Rc<RefCell<SessionTerminalState>>) {
    area.set_draw_func(move |_area, cr, width, height| {
        if let Ok(mut state) = state.try_borrow_mut() {
            state.draw(cr, width, height);
        }
    });
}

fn install_socket_pump(state: Rc<RefCell<SessionTerminalState>>) {
    glib::timeout_add_local(Duration::from_millis(33), move || {
        if let Ok(mut state) = state.try_borrow_mut() {
            if state.area.parent().is_none() {
                return glib::ControlFlow::Break;
            }
            state.poll();
        }
        glib::ControlFlow::Continue
    });
}

fn install_key_controller(area: &DrawingArea, state: Rc<RefCell<SessionTerminalState>>) {
    let controller = EventControllerKey::new();
    let im_context = IMMulticontext::new();
    im_context.set_client_widget(Some(area));
    im_context.set_use_preedit(false);
    {
        let state = state.clone();
        im_context.connect_commit(move |_context, text| {
            if let Ok(mut state) = state.try_borrow_mut() {
                state.send_text(text);
            }
        });
    }
    controller.set_im_context(Some(&im_context));
    let area_for_clipboard = area.clone();
    controller.connect_key_pressed(move |controller, key, _keycode, modifiers| {
        if is_paste_shortcut(key, modifiers) {
            paste_from_clipboard(&area_for_clipboard, state.clone(), false);
            return glib::Propagation::Stop;
        }

        if is_copy_shortcut(key, modifiers) {
            if let Ok(mut s) = state.try_borrow_mut() {
                if let Some(text) = s.copy_selection_text() {
                    area_for_clipboard.clipboard().set_text(&text);
                }
            }
            return glib::Propagation::Stop;
        }

        let modifiers = key_modifiers(controller, modifiers);
        if let Ok(mut state) = state.try_borrow_mut() {
            state.handle_key(key, modifiers);
        }
        glib::Propagation::Stop
    });
    area.add_controller(controller);
}

fn install_context_menu(area: &DrawingArea, state: Rc<RefCell<SessionTerminalState>>) {
    let menu = gio::Menu::new();
    menu.append(Some("Copy"), Some("term.copy"));
    menu.append(Some("Paste"), Some("term.paste"));

    let popover = PopoverMenu::from_model(Some(&menu));
    popover.set_has_arrow(false);
    popover.set_parent(area);

    let action_group = gio::SimpleActionGroup::new();

    let copy_action = gio::SimpleAction::new("copy", None);
    {
        let area = area.clone();
        let state = state.clone();
        copy_action.connect_activate(move |_, _| {
            if let Ok(mut s) = state.try_borrow_mut() {
                if let Some(text) = s.copy_selection_text() {
                    area.clipboard().set_text(&text);
                }
            }
        });
    }
    action_group.add_action(&copy_action);

    let paste_action = gio::SimpleAction::new("paste", None);
    {
        let area = area.clone();
        let state = state.clone();
        paste_action.connect_activate(move |_, _| {
            paste_from_clipboard(&area, state.clone(), false);
        });
    }
    action_group.add_action(&paste_action);

    area.insert_action_group("term", Some(&action_group));

    let gesture = GestureClick::new();
    gesture.set_button(gdk::BUTTON_SECONDARY);
    {
        let popover = popover.clone();
        let area = area.clone();
        gesture.connect_pressed(move |_g, _n, x, y| {
            if area.root().is_none() {
                return;
            }
            if popover.parent().is_none() {
                popover.set_parent(&area);
            }
            area.grab_focus();
            let rect = gdk::Rectangle::new(x as i32, y as i32, 1, 1);
            popover.set_pointing_to(Some(&rect));
            popover.popup();
        });
    }
    area.add_controller(gesture);
}

fn install_drag_controller(area: &DrawingArea, state: Rc<RefCell<SessionTerminalState>>) {
    let drag = GestureDrag::new();
    drag.set_button(gdk::BUTTON_PRIMARY);
    {
        let state = state.clone();
        drag.connect_drag_begin(move |_drag, x, y| {
            if let Ok(mut s) = state.try_borrow_mut() {
                s.begin_selection(x, y);
            }
        });
    }
    {
        let state = state.clone();
        drag.connect_drag_update(move |drag, dx, dy| {
            let Some((sx, sy)) = drag.start_point() else {
                return;
            };
            if let Ok(mut s) = state.try_borrow_mut() {
                s.update_selection(sx + dx, sy + dy);
            }
        });
    }
    {
        let area = area.clone();
        drag.connect_drag_end(move |drag, dx, dy| {
            let Some((sx, sy)) = drag.start_point() else {
                return;
            };
            let text_opt = if let Ok(mut s) = state.try_borrow_mut() {
                s.update_selection(sx + dx, sy + dy);
                s.copy_selection_text()
            } else {
                None
            };
            if let Some(text) = text_opt {
                area.primary_clipboard().set_text(&text);
            }
        });
    }
    area.add_controller(drag);
}

fn install_scroll_controller(area: &DrawingArea, state: Rc<RefCell<SessionTerminalState>>) {
    let controller = EventControllerScroll::new(EventControllerScrollFlags::VERTICAL);
    controller.connect_scroll(move |_controller, _dx, dy| {
        if let Ok(mut state) = state.try_borrow_mut() {
            state.scroll_by_delta(dy);
        }
        glib::Propagation::Stop
    });
    area.add_controller(controller);
}

fn install_scrollbar_sync(adjustment: &Adjustment, state: Rc<RefCell<SessionTerminalState>>) {
    adjustment.connect_value_changed(move |adjustment| {
        if let Ok(mut state) = state.try_borrow_mut() {
            state.handle_adjustment_change(adjustment.value().round() as u64);
        }
    });
}

fn install_focus_controller(area: &DrawingArea, state: Rc<RefCell<SessionTerminalState>>) {
    let controller = GestureClick::new();
    {
        let area = area.clone();
        let state = state.clone();
        controller.connect_pressed(move |gesture, _n_press, _x, _y| {
            area.grab_focus();
            if gesture.current_button() == 2 {
                paste_from_clipboard(&area, state.clone(), true);
            }
        });
    }
    area.add_controller(controller);
}

struct SessionTerminalState {
    area: DrawingArea,
    adjustment: Adjustment,
    scrollbar: Scrollbar,
    terminal: Terminal<'static, 'static>,
    render_state: RenderState<'static>,
    row_iterator: RowIterator<'static>,
    cell_iterator: CellIterator<'static>,
    socket: Option<UnixStream>,
    session_pid: Option<u32>,
    sync_adjustment: bool,
    last_cols: u16,
    last_rows: u16,
    cell_width: f64,
    cell_height: f64,
    font_desc: pango::FontDescription,
    selection: Option<Selection>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct SelectionPoint {
    /// Column within the row.
    col: u16,
    /// Absolute row index in the scrollback (scrollbar offset + viewport row).
    row: u64,
}

#[derive(Clone, Copy, Debug)]
struct Selection {
    anchor: SelectionPoint,
    head: SelectionPoint,
}

impl Selection {
    fn is_empty(&self) -> bool {
        self.anchor == self.head
    }

    fn normalized(&self) -> (SelectionPoint, SelectionPoint) {
        let a = (self.anchor.row, self.anchor.col);
        let h = (self.head.row, self.head.col);
        if a <= h {
            (self.anchor, self.head)
        } else {
            (self.head, self.anchor)
        }
    }

    fn contains(&self, col: u16, row: u64) -> bool {
        if self.is_empty() {
            return false;
        }
        let (start, end) = self.normalized();
        if row < start.row || row > end.row {
            return false;
        }
        if start.row == end.row {
            col >= start.col && col < end.col
        } else if row == start.row {
            col >= start.col
        } else if row == end.row {
            col < end.col
        } else {
            true
        }
    }
}

impl SessionTerminalState {
    fn new(
        session: SessionEntry,
        area: DrawingArea,
        adjustment: Adjustment,
        scrollbar: Scrollbar,
    ) -> Rc<RefCell<Self>> {
        let mut terminal = Terminal::new(TerminalOptions {
            cols: DEFAULT_COLS,
            rows: DEFAULT_ROWS,
            max_scrollback: MAX_SCROLLBACK,
        })
        .expect("failed to create libghostty-vt terminal");
        apply_pretty_theme(&mut terminal);

        if let Ok(log) = fs::read(Path::new(&session.log_path)) {
            terminal.vt_write(&log);
        }

        let socket = if matches!(session.status.as_str(), "running" | "starting")
            && Path::new(&session.socket_path).exists()
        {
            UnixStream::connect(&session.socket_path)
                .ok()
                .and_then(|stream| {
                    stream.set_nonblocking(true).ok()?;
                    Some(stream)
                })
        } else {
            None
        };

        Rc::new(RefCell::new(Self {
            area,
            adjustment,
            scrollbar,
            terminal,
            render_state: RenderState::new().expect("failed to create libghostty-vt render state"),
            row_iterator: RowIterator::new().expect("failed to create libghostty-vt row iterator"),
            cell_iterator: CellIterator::new()
                .expect("failed to create libghostty-vt cell iterator"),
            socket,
            session_pid: session.pid,
            sync_adjustment: false,
            last_cols: DEFAULT_COLS,
            last_rows: DEFAULT_ROWS,
            cell_width: 8.6,
            cell_height: 18.0,
            font_desc: pango::FontDescription::new(),
            selection: None,
        }))
    }

    fn poll(&mut self) {
        let should_follow_output = self.should_follow_output();
        let Some(socket) = &mut self.socket else {
            return;
        };

        let mut changed = false;

        loop {
            let mut chunk = [0_u8; 4096];
            match socket.read(&mut chunk) {
                Ok(0) => {
                    self.socket = None;
                    break;
                }
                Ok(len) => {
                    self.terminal.vt_write(&chunk[..len]);
                    changed = true;
                }
                Err(err) if err.kind() == io::ErrorKind::WouldBlock => break,
                Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
                Err(_) => {
                    self.socket = None;
                    break;
                }
            }
        }

        if changed {
            if should_follow_output {
                self.terminal.scroll_viewport(ScrollViewport::Bottom);
            }
            self.refresh_view();
        }
    }

    fn refresh_view(&mut self) {
        self.sync_scrollbar();
        self.area.queue_draw();
    }

    fn draw(&mut self, cr: &gtk::cairo::Context, width: i32, height: i32) {
        self.update_metrics(cr);
        self.resize_terminal(width, height);

        let scrollbar_offset = self.terminal.scrollbar().ok().map(|s| s.offset).unwrap_or(0);
        let selection = self.selection;

        let Ok(snapshot) = self.render_state.update(&self.terminal) else {
            return;
        };
        let Ok(colors) = snapshot.colors() else {
            return;
        };

        paint_rect(cr, 0.0, 0.0, width as f64, height as f64, colors.background);
        let layout = create_layout(cr);
        layout.set_font_description(Some(&self.font_desc));

        let Ok(mut rows) = self.row_iterator.update(&snapshot) else {
            return;
        };

        let mut row_index = 0_u16;
        while let Some(row) = rows.next() {
            let Ok(mut cells) = self.cell_iterator.update(row) else {
                break;
            };

            let abs_row = scrollbar_offset.saturating_add(u64::from(row_index));

            let mut col_index = 0_u16;
            while let Some(cell) = cells.next() {
                let style = cell.style().unwrap_or_default();
                let (mut fg, mut bg) =
                    resolved_colors(cell, &style, colors.foreground, colors.background);

                let selected = selection
                    .map(|s| s.contains(col_index, abs_row))
                    .unwrap_or(false);
                if selected {
                    std::mem::swap(&mut fg, &mut bg);
                }

                let x = HORIZONTAL_PADDING + (f64::from(col_index) * self.cell_width);
                let y = VERTICAL_PADDING + (f64::from(row_index) * self.cell_height);
                paint_rect(cr, x, y, self.cell_width, self.cell_height, bg);

                if !style.invisible {
                    let text: String = cell.graphemes().unwrap_or_default().into_iter().collect();
                    if !text.is_empty() && text != " " {
                        let font_desc = styled_font_description(&self.font_desc, style);
                        layout.set_font_description(Some(&font_desc));
                        layout.set_text(&text);
                        set_source_color(cr, fg);
                        cr.move_to(x, y);
                        show_layout(cr, &layout);
                    }
                }

                col_index = col_index.saturating_add(1);
            }

            row_index = row_index.saturating_add(1);
        }

        if let Ok(true) = snapshot.cursor_visible() {
            if let Ok(Some(cursor)) = snapshot.cursor_viewport() {
                let cursor_color = snapshot
                    .cursor_color()
                    .ok()
                    .flatten()
                    .or(colors.cursor)
                    .unwrap_or(colors.foreground);
                draw_cursor(
                    cr,
                    cursor.x,
                    cursor.y,
                    cursor_color,
                    &snapshot,
                    self.cell_width,
                    self.cell_height,
                );
            }
        }
    }

    fn update_metrics(&mut self, cr: &gtk::cairo::Context) {
        let layout = create_layout(cr);
        let font_context = layout.context();
        self.font_desc = terminal_font_description(&font_context);
        layout.set_font_description(Some(&self.font_desc));
        layout.set_text("M");

        let (pixel_width, pixel_height) = layout.pixel_size();

        self.cell_width = f64::from(pixel_width).max(1.0);
        self.cell_height = f64::from(pixel_height).max(1.0);
    }

    fn resize_terminal(&mut self, width: i32, height: i32) {
        let usable_width = (f64::from(width) - (HORIZONTAL_PADDING * 2.0)).max(self.cell_width);
        let usable_height = (f64::from(height) - (VERTICAL_PADDING * 2.0)).max(self.cell_height);
        let cols = ((usable_width / self.cell_width).floor() as u16).max(1);
        let rows = ((usable_height / self.cell_height).floor() as u16).max(1);
        if cols == self.last_cols && rows == self.last_rows {
            return;
        }

        self.last_cols = cols;
        self.last_rows = rows;
        let _ = self.terminal.resize(
            cols,
            rows,
            self.cell_width.round() as u32,
            self.cell_height.round() as u32,
        );
        self.resize_pty(cols, rows);
        if self.should_follow_output() {
            self.terminal.scroll_viewport(ScrollViewport::Bottom);
        }
        self.sync_scrollbar();
    }

    fn resize_pty(&self, cols: u16, rows: u16) {
        let Some(pid) = self.session_pid else {
            return;
        };
        let tty_path = format!("/proc/{pid}/fd/0");
        let Ok(file) = File::options()
            .read(true)
            .write(true)
            .custom_flags(libc::O_CLOEXEC)
            .open(&tty_path)
        else {
            return;
        };

        let mut winsize = libc::winsize {
            ws_row: rows,
            ws_col: cols,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        unsafe {
            libc::ioctl(file.as_raw_fd(), libc::TIOCSWINSZ, &mut winsize);
        }
    }

    fn should_follow_output(&self) -> bool {
        let Ok(scrollbar) = self.terminal.scrollbar() else {
            return false;
        };

        !self.is_alternate_screen()
            && scrollbar.offset.saturating_add(scrollbar.len) >= scrollbar.total
    }

    fn handle_adjustment_change(&mut self, value: u64) {
        if self.sync_adjustment {
            return;
        }

        let Ok(scrollbar) = self.terminal.scrollbar() else {
            return;
        };

        let target = value.min(scrollbar.total.saturating_sub(scrollbar.len));
        if target == scrollbar.offset {
            return;
        }

        if target == 0 {
            self.terminal.scroll_viewport(ScrollViewport::Top);
        } else if target.saturating_add(scrollbar.len) >= scrollbar.total {
            self.terminal.scroll_viewport(ScrollViewport::Bottom);
        } else {
            let delta = target as i64 - scrollbar.offset as i64;
            self.terminal
                .scroll_viewport(ScrollViewport::Delta(delta as isize));
        }

        self.refresh_view();
    }

    fn scroll_by_delta(&mut self, dy: f64) {
        if dy == 0.0 {
            return;
        }

        if self.is_alternate_screen() {
            return;
        }

        let delta = (dy.signum() as isize) * SCROLL_LINES_PER_TICK;
        self.terminal.scroll_viewport(ScrollViewport::Delta(delta));
        self.refresh_view();
    }

    fn sync_scrollbar(&mut self) {
        let Ok(scrollbar) = self.terminal.scrollbar() else {
            return;
        };

        self.sync_adjustment = true;
        self.adjustment.set_lower(0.0);
        self.adjustment
            .set_upper(scrollbar.total.max(scrollbar.len) as f64);
        self.adjustment.set_page_size(scrollbar.len as f64);
        self.adjustment
            .set_page_increment(scrollbar.len.max(1) as f64);
        self.adjustment.set_step_increment(1.0);
        self.adjustment.set_value(scrollbar.offset as f64);
        self.sync_adjustment = false;

        self.scrollbar
            .set_visible(scrollbar.total > scrollbar.len && !self.is_alternate_screen());
    }

    fn is_alternate_screen(&self) -> bool {
        matches!(
            self.terminal.active_screen(),
            Ok(ffi::GhosttyTerminalScreen_GHOSTTY_TERMINAL_SCREEN_ALTERNATE)
        )
    }

    fn handle_key(&mut self, key: gdk::Key, modifiers: gdk::ModifierType) {
        let Some(socket) = &mut self.socket else {
            return;
        };

        let bytes = encode_key(key, modifiers);
        if bytes.is_empty() {
            return;
        }

        if socket.write_all(&bytes).is_ok() {
            let _ = socket.flush();
        }
    }

    fn send_text(&mut self, text: &str) {
        let Some(socket) = &mut self.socket else {
            return;
        };

        if socket.write_all(text.as_bytes()).is_ok() {
            let _ = socket.flush();
        }
    }

    fn paste_text(&mut self, text: &str) {
        self.send_text(text);
    }

    fn pixel_to_cell(&self, x: f64, y: f64) -> (u16, u16) {
        let cols = self.last_cols.max(1);
        let rows = self.last_rows.max(1);
        let col_f = ((x - HORIZONTAL_PADDING) / self.cell_width).floor();
        let row_f = ((y - VERTICAL_PADDING) / self.cell_height).floor();
        let col = col_f.clamp(0.0, f64::from(cols - 1)) as u16;
        let row = row_f.clamp(0.0, f64::from(rows - 1)) as u16;
        (col, row)
    }

    fn viewport_row_to_absolute(&self, vrow: u16) -> u64 {
        let offset = self.terminal.scrollbar().map(|s| s.offset).unwrap_or(0);
        offset.saturating_add(u64::from(vrow))
    }

    fn begin_selection(&mut self, x: f64, y: f64) {
        let (col, vrow) = self.pixel_to_cell(x, y);
        let row = self.viewport_row_to_absolute(vrow);
        let point = SelectionPoint { col, row };
        self.selection = Some(Selection {
            anchor: point,
            head: point,
        });
        self.area.queue_draw();
    }

    fn update_selection(&mut self, x: f64, y: f64) {
        if self.selection.is_none() {
            return;
        }
        let (col, vrow) = self.pixel_to_cell(x, y);
        let row = self.viewport_row_to_absolute(vrow);
        if let Some(selection) = self.selection.as_mut() {
            selection.head = SelectionPoint { col, row };
        }
        self.area.queue_draw();
    }

    fn copy_selection_text(&mut self) -> Option<String> {
        let selection = self.selection?;
        if selection.is_empty() {
            return None;
        }
        let (start, end) = selection.normalized();

        let snapshot = self.render_state.update(&self.terminal).ok()?;
        let scrollbar_offset = self.terminal.scrollbar().ok().map(|s| s.offset).unwrap_or(0);

        let mut rows = self.row_iterator.update(&snapshot).ok()?;
        let mut output = String::new();
        let mut visual_index: u16 = 0;
        let mut emitted_any = false;

        while let Some(row) = rows.next() {
            let abs_row = scrollbar_offset.saturating_add(u64::from(visual_index));
            visual_index = visual_index.saturating_add(1);

            if abs_row < start.row || abs_row > end.row {
                continue;
            }

            let start_col = if abs_row == start.row { start.col } else { 0 };
            let end_col_exclusive = if abs_row == end.row { end.col } else { u16::MAX };

            let Ok(mut cells) = self.cell_iterator.update(row) else {
                continue;
            };

            let mut line = String::new();
            let mut col_idx: u16 = 0;
            while let Some(cell) = cells.next() {
                if col_idx >= start_col && col_idx < end_col_exclusive {
                    let graphemes = cell.graphemes().unwrap_or_default();
                    if graphemes.is_empty() {
                        line.push(' ');
                    } else {
                        line.extend(graphemes);
                    }
                }
                col_idx = col_idx.saturating_add(1);
            }

            let trimmed = line.trim_end_matches(|c: char| c == ' ' || c == '\0');
            if emitted_any {
                output.push('\n');
            }
            output.push_str(trimmed);
            emitted_any = true;
        }

        if output.is_empty() { None } else { Some(output) }
    }
}

fn resolved_colors(
    cell: &libghostty_vt::render::CellIteration<'_, '_>,
    style: &Style,
    default_fg: RgbColor,
    default_bg: RgbColor,
) -> (RgbColor, RgbColor) {
    let mut fg = cell.fg_color().ok().flatten().unwrap_or(default_fg);
    let mut bg = cell.bg_color().ok().flatten().unwrap_or(default_bg);

    if style.inverse {
        std::mem::swap(&mut fg, &mut bg);
    }

    if style.faint {
        fg = dim_color(fg);
    }

    (fg, bg)
}

fn dim_color(color: RgbColor) -> RgbColor {
    RgbColor {
        r: ((f64::from(color.r) * 0.7).round() as u8),
        g: ((f64::from(color.g) * 0.7).round() as u8),
        b: ((f64::from(color.b) * 0.7).round() as u8),
    }
}

fn draw_cursor(
    cr: &gtk::cairo::Context,
    x: u16,
    y: u16,
    color: RgbColor,
    snapshot: &libghostty_vt::render::Snapshot<'_, '_>,
    cell_width: f64,
    cell_height: f64,
) {
    let x = HORIZONTAL_PADDING + (f64::from(x) * cell_width);
    let y = VERTICAL_PADDING + (f64::from(y) * cell_height);
    let style = snapshot
        .cursor_visual_style()
        .unwrap_or(CursorVisualStyle::Block);

    match style {
        CursorVisualStyle::Bar => {
            paint_rect(cr, x, y, 2.0, cell_height, color);
        }
        CursorVisualStyle::Underline => {
            paint_rect(cr, x, y + cell_height - 2.0, cell_width, 2.0, color);
        }
        CursorVisualStyle::BlockHollow => {
            set_source_color(cr, color);
            cr.set_line_width(1.0);
            cr.rectangle(x + 0.5, y + 0.5, cell_width - 1.0, cell_height - 1.0);
            let _ = cr.stroke();
        }
        CursorVisualStyle::Block => {
            let mut fill = color;
            fill.r = fill.r.saturating_add(24);
            fill.g = fill.g.saturating_add(24);
            fill.b = fill.b.saturating_add(24);
            paint_rect(cr, x, y, cell_width, cell_height, fill);
        }
        _ => {
            paint_rect(cr, x, y, cell_width, cell_height, color);
        }
    }
}

fn paint_rect(cr: &gtk::cairo::Context, x: f64, y: f64, width: f64, height: f64, color: RgbColor) {
    set_source_color(cr, color);
    cr.rectangle(x, y, width, height);
    let _ = cr.fill();
}

fn set_source_color(cr: &gtk::cairo::Context, color: RgbColor) {
    cr.set_source_rgb(
        f64::from(color.r) / 255.0,
        f64::from(color.g) / 255.0,
        f64::from(color.b) / 255.0,
    );
}

fn terminal_font_description(context: &pango::Context) -> pango::FontDescription {
    let mut font_desc = pango::FontDescription::new();
    font_desc.set_family(select_terminal_font_family(context));
    font_desc.set_size((FONT_SIZE * f64::from(pango::SCALE)).round() as i32);
    font_desc
}

fn styled_font_description(base: &pango::FontDescription, style: Style) -> pango::FontDescription {
    let mut font_desc = base.clone();
    font_desc.set_style(if style.italic {
        pango::Style::Italic
    } else {
        pango::Style::Normal
    });
    font_desc.set_weight(if style.bold {
        pango::Weight::Bold
    } else {
        pango::Weight::Normal
    });
    font_desc
}

fn apply_pretty_theme(terminal: &mut Terminal<'static, 'static>) {
    let mut escape = String::new();
    write!(
        &mut escape,
        "\u{1b}]10;{}\u{7}\u{1b}]11;{}\u{7}\u{1b}]12;{}\u{7}",
        color_to_hex(THEME_FOREGROUND),
        color_to_hex(THEME_BACKGROUND),
        color_to_hex(THEME_CURSOR)
    )
    .expect("writing terminal theme colors should not fail");

    for (index, color) in THEME_PALETTE.iter().enumerate() {
        write!(
            &mut escape,
            "\u{1b}]4;{index};{}\u{7}",
            color_to_hex(*color)
        )
        .expect("writing terminal palette should not fail");
    }

    terminal.vt_write(escape.as_bytes());
}

fn color_to_hex(color: RgbColor) -> String {
    format!("#{:02x}{:02x}{:02x}", color.r, color.g, color.b)
}

fn select_terminal_font_family(context: &pango::Context) -> &str {
    let families = context.list_families();
    for candidate in FONT_FAMILIES {
        if families.iter().any(|family| family.name() == candidate) {
            return candidate;
        }
    }

    "Monospace"
}

const fn rgb(r: u8, g: u8, b: u8) -> RgbColor {
    RgbColor { r, g, b }
}

fn is_paste_shortcut(key: gdk::Key, modifiers: gdk::ModifierType) -> bool {
    let ctrl_shift = modifiers.contains(gdk::ModifierType::CONTROL_MASK)
        && modifiers.contains(gdk::ModifierType::SHIFT_MASK);
    let shift_only = modifiers.contains(gdk::ModifierType::SHIFT_MASK)
        && !modifiers.contains(gdk::ModifierType::CONTROL_MASK)
        && !modifiers.contains(gdk::ModifierType::ALT_MASK);

    (ctrl_shift && matches!(key, gdk::Key::V | gdk::Key::v))
        || (shift_only && matches!(key, gdk::Key::Insert))
}

fn is_copy_shortcut(key: gdk::Key, modifiers: gdk::ModifierType) -> bool {
    let ctrl_shift = modifiers.contains(gdk::ModifierType::CONTROL_MASK)
        && modifiers.contains(gdk::ModifierType::SHIFT_MASK);
    ctrl_shift && matches!(key, gdk::Key::C | gdk::Key::c)
}

fn paste_from_clipboard(
    area: &DrawingArea,
    state: Rc<RefCell<SessionTerminalState>>,
    primary: bool,
) {
    let clipboard = if primary {
        area.primary_clipboard()
    } else {
        area.clipboard()
    };

    clipboard.read_text_async(None::<&gtk::gio::Cancellable>, move |result| {
        let Ok(Some(text)) = result else {
            return;
        };

        if let Ok(mut state) = state.try_borrow_mut() {
            state.paste_text(text.as_str());
        }
    });
}

fn key_modifiers(
    controller: &EventControllerKey,
    modifiers: gdk::ModifierType,
) -> gdk::ModifierType {
    controller
        .current_event()
        .and_then(|event| event.downcast::<gdk::KeyEvent>().ok())
        .map(|event| modifiers.difference(event.consumed_modifiers()))
        .unwrap_or(modifiers)
}

fn encode_key(key: gdk::Key, modifiers: gdk::ModifierType) -> Vec<u8> {
    match key {
        gdk::Key::Return => vec![b'\r'],
        gdk::Key::Tab => vec![b'\t'],
        gdk::Key::BackSpace => vec![0x7f],
        gdk::Key::Escape => vec![0x1b],
        gdk::Key::Up => b"\x1b[A".to_vec(),
        gdk::Key::Down => b"\x1b[B".to_vec(),
        gdk::Key::Right => b"\x1b[C".to_vec(),
        gdk::Key::Left => b"\x1b[D".to_vec(),
        gdk::Key::Home => b"\x1b[H".to_vec(),
        gdk::Key::End => b"\x1b[F".to_vec(),
        gdk::Key::Page_Up => b"\x1b[5~".to_vec(),
        gdk::Key::Page_Down => b"\x1b[6~".to_vec(),
        gdk::Key::Delete => b"\x1b[3~".to_vec(),
        key => {
            if modifiers.contains(gdk::ModifierType::CONTROL_MASK) {
                if let Some(ch) = key.to_unicode() {
                    if let Some(byte) = encode_ctrl_char(ch) {
                        return vec![byte];
                    }
                }
                return Vec::new();
            }

            if modifiers.contains(gdk::ModifierType::ALT_MASK) {
                if let Some(ch) = key.to_unicode() {
                    let mut out = vec![0x1b];
                    let mut buf = [0_u8; 4];
                    out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
                    return out;
                }
            }

            if key.to_unicode().is_some() {
                return Vec::new();
            }

            Vec::new()
        }
    }
}

fn encode_ctrl_char(ch: char) -> Option<u8> {
    match ch {
        '@' | '`' | ' ' => Some(0x00),
        'a'..='z' => Some((ch as u8) - b'a' + 0x01),
        'A'..='Z' => Some((ch as u8) - b'A' + 0x01),
        '[' => Some(0x1b),
        '\\' => Some(0x1c),
        ']' => Some(0x1d),
        '^' => Some(0x1e),
        '_' => Some(0x1f),
        _ => None,
    }
}
