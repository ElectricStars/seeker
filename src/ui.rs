// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Eric Allen

//! The egui desktop interface.

use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::time::Duration;

use crossbeam_channel::{Receiver, Sender};
use eframe::egui::{
    self, Align, Color32, FontId, Key, Layout, RichText, Sense, TextFormat, text::LayoutJob,
};
use egui_extras::{Column, TableBuilder};
use serde::{Deserialize, Serialize};

use crate::cli::Args;
use crate::export;
use crate::platform;
use crate::search::{self, Event, FileHit, NameMode, SearchHandle, SearchOptions, Stats};
use crate::{APP_ID, APP_NAME, DESKTOP_ID};

const STORAGE_KEY: &str = "form";
const MAX_ERRORS_KEPT: usize = 2_000;

/// Everything typed into the search form. Persisted between runs.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
struct Form {
    name_pattern: String,
    name_mode: NameMode,
    content: String,
    content_regex: bool,
    case_sensitive: bool,
    whole_word: bool,
    look_in: String,
    recurse: bool,
    exclude_dirs: String,
    respect_gitignore: bool,
    include_hidden: bool,
    follow_symlinks: bool,
    skip_binary: bool,
    min_size: String,
    max_size: String,
    modified_after: String,
    modified_before: String,
    context_lines: usize,
    show_options: bool,
}

impl Default for Form {
    fn default() -> Self {
        let d = SearchOptions::default();
        Self {
            name_pattern: String::new(),
            name_mode: NameMode::Wildcard,
            content: String::new(),
            content_regex: false,
            case_sensitive: false,
            whole_word: false,
            look_in: std::env::var("HOME").unwrap_or_else(|_| "/".into()),
            recurse: d.recurse,
            exclude_dirs: d.exclude_dirs.join(";"),
            respect_gitignore: d.respect_gitignore,
            include_hidden: d.include_hidden,
            follow_symlinks: d.follow_symlinks,
            skip_binary: d.skip_binary,
            min_size: String::new(),
            max_size: String::new(),
            modified_after: String::new(),
            modified_before: String::new(),
            context_lines: d.context_lines,
            show_options: true,
        }
    }
}

impl Form {
    fn to_options(&self) -> Result<SearchOptions, String> {
        Ok(SearchOptions {
            roots: search::parse_roots(&self.look_in),
            name_pattern: self.name_pattern.clone(),
            name_mode: self.name_mode,
            content: self.content.clone(),
            content_is_regex: self.content_regex,
            case_sensitive: self.case_sensitive,
            whole_word: self.whole_word,
            recurse: self.recurse,
            exclude_dirs: self
                .exclude_dirs
                .split(';')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect(),
            respect_gitignore: self.respect_gitignore,
            include_hidden: self.include_hidden,
            follow_symlinks: self.follow_symlinks,
            skip_binary: self.skip_binary,
            min_size: search::parse_size(&self.min_size).map_err(|e| format!("Min size: {e}"))?,
            max_size: search::parse_size(&self.max_size).map_err(|e| format!("Max size: {e}"))?,
            modified_after: search::parse_date(&self.modified_after, false)
                .map_err(|e| format!("Modified after: {e}"))?,
            modified_before: search::parse_date(&self.modified_before, true)
                .map_err(|e| format!("Modified before: {e}"))?,
            context_lines: self.context_lines,
            threads: 0,
        })
    }

    fn apply_args(&mut self, a: &Args) {
        if !a.folders.is_empty() {
            self.look_in = a.folders.join(";");
        }
        if let Some(n) = &a.name {
            self.name_pattern = n.clone();
            self.name_mode = if a.name_regex {
                NameMode::Regex
            } else {
                NameMode::Wildcard
            };
        }
        if let Some(t) = &a.text {
            self.content = t.clone();
            self.content_regex = a.regex;
            self.case_sensitive = a.case_sensitive;
            self.whole_word = a.word;
        }
        if let Some(c) = a.context {
            self.context_lines = c;
        }
        if let Some(x) = &a.exclude {
            self.exclude_dirs = x.clone();
        }
        self.include_hidden |= a.hidden;
        self.respect_gitignore |= a.gitignore;
        self.follow_symlinks |= a.follow;
        if a.binary {
            self.skip_binary = false;
        }
        if a.no_recurse {
            self.recurse = false;
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SortCol {
    Found,
    Name,
    Folder,
    Size,
    Modified,
    Hits,
}

/// Results of native dialogs, which run on a helper thread.
enum DialogResult {
    Folders(Vec<PathBuf>),
    Export(PathBuf, ExportKind),
    /// The dialog closed instantly: probably no xdg-desktop-portal or zenity.
    Unavailable(Option<ExportKind>),
}

/// A dialog that returns "cancelled" faster than this almost certainly never opened.
const DIALOG_MIN_TIME: Duration = Duration::from_millis(400);

#[derive(Debug, Clone, Copy)]
enum ExportKind {
    Csv,
    Text,
}

pub struct SeekerApp {
    form: Form,
    results: Vec<FileHit>,
    /// Arrival order, so "unsorted" can be restored.
    sort: (SortCol, bool),
    needs_sort: bool,
    selected: Option<usize>,
    current_hit: usize,
    scroll_preview: bool,
    scroll_table: bool,
    search: Option<SearchHandle>,
    last_stats: Option<Stats>,
    errors: Vec<String>,
    show_errors: bool,
    show_about: bool,
    message: Option<(String, bool)>, // (text, is_error)
    auto_start: bool,
    dialog_tx: Sender<DialogResult>,
    dialog_rx: Receiver<DialogResult>,
    focus_name: bool,
}

impl SeekerApp {
    fn new(cc: &eframe::CreationContext<'_>, args: Args) -> Self {
        // Use the bundled Hack font as a fallback so symbols like ▲▼ render.
        let mut fonts = egui::FontDefinitions::default();
        if let Some(fam) = fonts.families.get_mut(&egui::FontFamily::Proportional) {
            fam.insert(1.min(fam.len()), "Hack".to_owned());
        }
        cc.egui_ctx.set_fonts(fonts);
        let mut form: Form = cc
            .storage
            .and_then(|s| eframe::get_value(s, STORAGE_KEY))
            .unwrap_or_default();
        form.apply_args(&args);
        let (dialog_tx, dialog_rx) = crossbeam_channel::unbounded();
        Self {
            form,
            results: Vec::new(),
            sort: (SortCol::Found, true),
            needs_sort: false,
            selected: None,
            current_hit: 0,
            scroll_preview: false,
            scroll_table: false,
            search: None,
            last_stats: None,
            errors: Vec::new(),
            show_errors: false,
            show_about: false,
            message: None,
            auto_start: args.auto,
            dialog_tx,
            dialog_rx,
            focus_name: true,
        }
    }

    fn is_running(&self) -> bool {
        self.search.is_some()
    }

    fn start_search(&mut self, ctx: &egui::Context) {
        self.stop_search();
        let opts = match self.form.to_options() {
            Ok(o) => o,
            Err(e) => {
                self.message = Some((e, true));
                return;
            }
        };
        let ctx2 = ctx.clone();
        match search::start(opts, move || {
            ctx2.request_repaint_after(Duration::from_millis(30))
        }) {
            Ok(h) => {
                self.results.clear();
                self.errors.clear();
                self.selected = None;
                self.current_hit = 0;
                self.last_stats = None;
                self.message = None;
                self.search = Some(h);
            }
            Err(e) => self.message = Some((e, true)),
        }
    }

    fn stop_search(&mut self) {
        if let Some(h) = &self.search {
            h.cancel();
        }
    }

    /// Drains events from the background search (bounded per frame to keep the UI fluid).
    fn poll_search(&mut self) {
        let Some(h) = &self.search else { return };
        let mut finished = None;
        for ev in h.rx.try_iter().take(20_000) {
            match ev {
                Event::Hit(hit) => {
                    self.results.push(*hit);
                    self.needs_sort = true;
                }
                Event::Error(e) => {
                    if self.errors.len() < MAX_ERRORS_KEPT {
                        self.errors.push(e);
                    }
                }
                Event::Done(mut stats) => {
                    stats.elapsed = h.elapsed();
                    finished = Some(stats);
                }
            }
        }
        if let Some(stats) = finished {
            self.last_stats = Some(stats);
            self.search = None;
            self.needs_sort = true;
            if self.selected.is_none() && !self.results.is_empty() {
                self.select(0);
            }
        }
    }

    fn apply_sort(&mut self) {
        if !self.needs_sort {
            return;
        }
        self.needs_sort = false;
        let (col, asc) = self.sort;
        if col == SortCol::Found {
            return; // arrival order
        }
        let sel_path = self
            .selected
            .and_then(|i| self.results.get(i))
            .map(|h| h.path.clone());
        let key_name = |h: &FileHit| {
            h.path
                .file_name()
                .map(|s| s.to_string_lossy().to_lowercase())
                .unwrap_or_default()
        };
        self.results.sort_by(|a, b| {
            let o = match col {
                SortCol::Found => std::cmp::Ordering::Equal,
                SortCol::Name => key_name(a).cmp(&key_name(b)),
                SortCol::Folder => a.path.parent().cmp(&b.path.parent()),
                SortCol::Size => a.size.cmp(&b.size),
                SortCol::Modified => a.modified.cmp(&b.modified),
                SortCol::Hits => a.hits.cmp(&b.hits),
            }
            .then_with(|| a.path.cmp(&b.path));
            if asc { o } else { o.reverse() }
        });
        if let Some(p) = sel_path {
            self.selected = self.results.iter().position(|h| h.path == p);
        }
    }

    fn select(&mut self, i: usize) {
        if self.selected != Some(i) {
            self.selected = Some(i);
            self.current_hit = 0;
            self.scroll_preview = true;
        }
    }

    fn hit_indices(&self) -> Vec<usize> {
        self.selected
            .and_then(|i| self.results.get(i))
            .map(|h| {
                h.lines
                    .iter()
                    .enumerate()
                    .filter(|(_, l)| l.is_hit)
                    .map(|(i, _)| i)
                    .collect()
            })
            .unwrap_or_default()
    }

    fn step_hit(&mut self, forward: bool) {
        let n = self.hit_indices().len();
        if n == 0 {
            return;
        }
        self.current_hit = if forward {
            (self.current_hit + 1) % n
        } else {
            (self.current_hit + n - 1) % n
        };
        self.scroll_preview = true;
    }

    fn pick_folders(&self, ctx: &egui::Context) {
        let tx = self.dialog_tx.clone();
        let ctx = ctx.clone();
        let start = search::parse_roots(&self.form.look_in).into_iter().next();
        std::thread::spawn(move || {
            let mut d = rfd::FileDialog::new().set_title("Choose folders to search");
            if let Some(s) = start {
                d = d.set_directory(s);
            }
            let t = std::time::Instant::now();
            match d.pick_folders() {
                Some(f) => {
                    platform::debug(&format!("folder picker returned {f:?}"));
                    let f: Vec<PathBuf> =
                        f.into_iter().map(platform::resolve_portal_path).collect();
                    platform::debug(&format!("resolved to {f:?}"));
                    let _ = tx.send(DialogResult::Folders(f));
                }
                None if t.elapsed() < DIALOG_MIN_TIME => {
                    let _ = tx.send(DialogResult::Unavailable(None));
                }
                None => {}
            }
            ctx.request_repaint();
        });
    }

    fn pick_export(&self, kind: ExportKind, ctx: &egui::Context) {
        let tx = self.dialog_tx.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let (name, ext, label) = match kind {
                ExportKind::Csv => ("seeker-results.csv", "csv", "CSV"),
                ExportKind::Text => ("seeker-results.txt", "txt", "Text"),
            };
            let t = std::time::Instant::now();
            match rfd::FileDialog::new()
                .set_title("Export results")
                .set_file_name(name)
                .add_filter(label, &[ext])
                .save_file()
            {
                Some(p) => {
                    platform::debug(&format!("save dialog returned {p:?}"));
                    let p = platform::sandbox_doc_path(p);
                    let _ = tx.send(DialogResult::Export(p, kind));
                }
                None if t.elapsed() < DIALOG_MIN_TIME => {
                    platform::debug("save dialog unavailable");
                    let _ = tx.send(DialogResult::Unavailable(Some(kind)));
                }
                None => {}
            }
            ctx.request_repaint();
        });
    }

    fn handle_dialogs(&mut self) {
        while let Ok(r) = self.dialog_rx.try_recv() {
            match r {
                DialogResult::Folders(f) => {
                    let list: Vec<String> =
                        f.iter().map(|p| p.to_string_lossy().into_owned()).collect();
                    self.form.look_in = list.join(";");
                }
                DialogResult::Unavailable(None) => {
                    self.message = Some((
                        "Folder picker unavailable (install xdg-desktop-portal or zenity). \
                         Type folders into \"Look in\", separated by ;"
                            .into(),
                        true,
                    ));
                }
                DialogResult::Unavailable(Some(kind)) => {
                    // No save dialog: write next to the user's home folder instead.
                    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
                    let ext = match kind {
                        ExportKind::Csv => "csv",
                        ExportKind::Text => "txt",
                    };
                    let home = std::env::var_os("HOME")
                        .map(PathBuf::from)
                        .unwrap_or_default();
                    let p = home.join(format!("{APP_ID}-results-{stamp}.{ext}"));
                    let _ = self.dialog_tx.send(DialogResult::Export(p, kind));
                }
                DialogResult::Export(p, kind) => {
                    let res = std::fs::File::create(&p).and_then(|f| {
                        let w = std::io::BufWriter::new(f);
                        match kind {
                            ExportKind::Csv => export::write_csv(w, &self.results),
                            ExportKind::Text => export::write_text(w, &self.results),
                        }
                    });
                    self.message = Some(match res {
                        Ok(()) => (
                            format!("Exported {} results to {}", self.results.len(), p.display()),
                            false,
                        ),
                        Err(e) => (format!("Export failed: {e}"), true),
                    });
                }
            }
        }
    }

    fn report(&mut self, r: Result<(), String>) {
        if let Err(e) = r {
            self.message = Some((e, true));
        }
    }

    fn handle_keys(&mut self, ctx: &egui::Context) {
        let typing = ctx.egui_wants_keyboard_input();
        let (enter, esc, f3, shift, up, down, ctrl_f) = ctx.input(|i| {
            (
                i.key_pressed(Key::Enter),
                i.key_pressed(Key::Escape),
                i.key_pressed(Key::F3),
                i.modifiers.shift,
                i.key_pressed(Key::ArrowUp),
                i.key_pressed(Key::ArrowDown),
                i.modifiers.command && i.key_pressed(Key::F),
            )
        });
        if esc && self.is_running() {
            self.stop_search();
        }
        if enter && !self.is_running() && !self.show_about {
            self.start_search(ctx);
        }
        if f3 {
            self.step_hit(!shift);
        }
        if ctrl_f {
            self.focus_name = true;
        }
        if !typing && !self.results.is_empty() {
            let n = self.results.len();
            if down {
                let i = self.selected.map_or(0, |i| (i + 1).min(n - 1));
                self.select(i);
                self.scroll_table = true;
            } else if up {
                let i = self.selected.map_or(0, |i| i.saturating_sub(1));
                self.select(i);
                self.scroll_table = true;
            }
        }
    }

    // ---------------------------------------------------------------- UI parts

    fn menu_bar(&mut self, ui: &mut egui::Ui) {
        egui::MenuBar::new().ui(ui, |ui| {
            ui.menu_button("File", |ui| {
                let has = !self.results.is_empty();
                if ui
                    .add_enabled(has, egui::Button::new("Export results as CSV…"))
                    .clicked()
                {
                    self.pick_export(ExportKind::Csv, ui.ctx());
                    ui.close();
                }
                if ui
                    .add_enabled(has, egui::Button::new("Export results as text…"))
                    .clicked()
                {
                    self.pick_export(ExportKind::Text, ui.ctx());
                    ui.close();
                }
                ui.separator();
                if ui.button("Reset form to defaults").clicked() {
                    self.form = Form::default();
                    ui.close();
                }
                ui.separator();
                if ui.button("Quit").clicked() {
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                }
            });
            ui.menu_button("Help", |ui| {
                if ui.button(format!("About {APP_NAME}")).clicked() {
                    self.show_about = true;
                    ui.close();
                }
            });
        });
    }

    fn search_form(&mut self, ui: &mut egui::Ui) {
        let running = self.is_running();
        egui::Grid::new("form")
            .num_columns(2)
            .spacing([8.0, 6.0])
            .show(ui, |ui| {
                ui.label("File name:");
                ui.horizontal(|ui| {
                    let r = ui.add(
                        egui::TextEdit::singleline(&mut self.form.name_pattern)
                            .hint_text("e.g. *.rs;*.toml;!*.lock   (empty = all files)")
                            .desired_width(ui.available_width() - 150.0),
                    );
                    if self.focus_name {
                        r.request_focus();
                        self.focus_name = false;
                    }
                    egui::ComboBox::from_id_salt("name_mode")
                        .width(120.0)
                        .selected_text(match self.form.name_mode {
                            NameMode::Wildcard => "Wildcards",
                            NameMode::Regex => "Regex",
                        })
                        .show_ui(ui, |ui| {
                            ui.selectable_value(
                                &mut self.form.name_mode,
                                NameMode::Wildcard,
                                "Wildcards",
                            );
                            ui.selectable_value(&mut self.form.name_mode, NameMode::Regex, "Regex");
                        });
                });
                ui.end_row();

                ui.label("Containing text:");
                ui.horizontal(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut self.form.content)
                            .hint_text("text to find inside files (empty = name search only)")
                            .desired_width(ui.available_width() - 290.0),
                    );
                    ui.checkbox(&mut self.form.content_regex, "Regex");
                    ui.checkbox(&mut self.form.case_sensitive, "Match case");
                    ui.checkbox(&mut self.form.whole_word, "Whole word");
                });
                ui.end_row();

                ui.label("Look in:");
                ui.horizontal(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut self.form.look_in)
                            .hint_text("folders, separated by ;")
                            .desired_width(ui.available_width() - 290.0),
                    );
                    if ui
                        .button("Browse…")
                        .on_hover_text("Pick one or more folders")
                        .clicked()
                    {
                        self.pick_folders(ui.ctx());
                    }
                    ui.checkbox(&mut self.form.recurse, "Subfolders");
                    ui.add_space(8.0);
                    if running {
                        if ui
                            .add(
                                egui::Button::new(RichText::new("Stop").strong())
                                    .min_size([80.0, 0.0].into()),
                            )
                            .on_hover_text("Esc")
                            .clicked()
                        {
                            self.stop_search();
                        }
                    } else if ui
                        .add(
                            egui::Button::new(RichText::new("Search").strong())
                                .min_size([80.0, 0.0].into()),
                        )
                        .on_hover_text("Enter")
                        .clicked()
                    {
                        let ctx = ui.ctx().clone();
                        self.start_search(&ctx);
                    }
                });
                ui.end_row();
            });

        let header = egui::CollapsingHeader::new("Options")
            .open(Some(self.form.show_options))
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.checkbox(&mut self.form.include_hidden, "Hidden files");
                    ui.checkbox(&mut self.form.respect_gitignore, "Respect .gitignore");
                    ui.checkbox(&mut self.form.follow_symlinks, "Follow symlinks");
                    ui.checkbox(&mut self.form.skip_binary, "Skip binary files");
                    ui.separator();
                    ui.label("Context lines:");
                    ui.add(egui::DragValue::new(&mut self.form.context_lines).range(0..=20));
                });
                ui.horizontal_wrapped(|ui| {
                    ui.label("Exclude folders:");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.form.exclude_dirs)
                            .hint_text(".git;node_modules")
                            .desired_width(260.0),
                    );
                    ui.separator();
                    ui.label("Size:");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.form.min_size)
                            .hint_text("min, e.g. 10K")
                            .desired_width(90.0),
                    );
                    ui.label("–");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.form.max_size)
                            .hint_text("max, e.g. 5M")
                            .desired_width(90.0),
                    );
                    ui.separator();
                    ui.label("Modified:");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.form.modified_after)
                            .hint_text("from YYYY-MM-DD")
                            .desired_width(110.0),
                    );
                    ui.label("–");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.form.modified_before)
                            .hint_text("to YYYY-MM-DD")
                            .desired_width(110.0),
                    );
                });
            });
        if header.header_response.clicked() {
            self.form.show_options = !self.form.show_options;
        }
        if let Some((msg, is_err)) = &self.message {
            let color = if *is_err {
                ui.visuals().error_fg_color
            } else {
                ui.visuals().weak_text_color()
            };
            ui.colored_label(color, msg);
        }
    }

    fn status_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            let fmt = |n: u64| format_count(n);
            if let Some(h) = &self.search {
                ui.spinner();
                let p = &h.progress;
                ui.label(format!(
                    "Searching…  {} files scanned · {} matching files · {} hits · {:.1} s",
                    fmt(p.files_scanned.load(Ordering::Relaxed)),
                    fmt(p.files_matched.load(Ordering::Relaxed)),
                    fmt(p.total_hits.load(Ordering::Relaxed)),
                    h.elapsed().as_secs_f32()
                ));
            } else if let Some(s) = &self.last_stats {
                ui.label(format!(
                    "{}  {} files scanned · {} matching files · {} hits · {}",
                    if s.cancelled { "Stopped." } else { "Done." },
                    fmt(s.files_scanned),
                    fmt(s.files_matched),
                    fmt(s.total_hits),
                    format_duration(s.elapsed)
                ));
            } else {
                ui.label("Ready. Press Enter to search.");
            }
            if !self.errors.is_empty() {
                ui.separator();
                let label = format!("{} unreadable — show", self.errors.len());
                if ui
                    .link(label)
                    .on_hover_text("Show permission/read errors")
                    .clicked()
                {
                    self.show_errors = true;
                }
            }
        });
    }

    fn results_table(&mut self, ui: &mut egui::Ui) {
        let row_h = ui.text_style_height(&egui::TextStyle::Body) + 4.0;
        let mut clicked: Option<usize> = None;
        let mut open: Option<usize> = None;
        let mut action: Option<(usize, RowAction)> = None;
        let mut sort_click: Option<SortCol> = None;
        let (cur_col, asc) = self.sort;
        let scroll_to = self.scroll_table.then_some(self.selected).flatten();
        self.scroll_table = false;
        let has_content = self.results.iter().any(|h| h.hit_lines > 0);

        let mut tb = TableBuilder::new(ui)
            .striped(true)
            .resizable(true)
            .sense(Sense::click())
            .cell_layout(Layout::left_to_right(Align::Center))
            .column(Column::initial(220.0).at_least(80.0).clip(true))
            .column(Column::initial(380.0).at_least(80.0).clip(true))
            .column(Column::initial(80.0).at_least(50.0))
            .column(Column::initial(130.0).at_least(80.0))
            .column(Column::remainder().at_least(50.0))
            .min_scrolled_height(0.0)
            .max_scroll_height(f32::INFINITY);
        if let Some(i) = scroll_to {
            tb = tb.scroll_to_row(i, None);
        }
        tb.header(row_h + 2.0, |mut header| {
            for (col, title) in [
                (SortCol::Name, "Name"),
                (SortCol::Folder, "Folder"),
                (SortCol::Size, "Size"),
                (SortCol::Modified, "Modified"),
                (SortCol::Hits, "Hits"),
            ] {
                header.col(|ui| {
                    let arrow = if cur_col == col {
                        if asc { " ▲" } else { " ▼" }
                    } else {
                        ""
                    };
                    let r = ui.add(
                        egui::Button::new(RichText::new(format!("{title}{arrow}")).strong())
                            .frame(false),
                    );
                    if r.clicked() {
                        sort_click = Some(col);
                    }
                });
            }
        })
        .body(|body| {
            body.rows(row_h, self.results.len(), |mut row| {
                let i = row.index();
                let h = &self.results[i];
                row.set_selected(self.selected == Some(i));
                let name = h
                    .path
                    .file_name()
                    .map(|s| s.to_string_lossy())
                    .unwrap_or_default();
                let folder = h
                    .path
                    .parent()
                    .map(|p| p.to_string_lossy())
                    .unwrap_or_default();
                row.col(|ui| {
                    ui.add(egui::Label::new(name.as_ref()).truncate().selectable(false));
                });
                row.col(|ui| {
                    ui.add(
                        egui::Label::new(folder.as_ref())
                            .truncate()
                            .selectable(false),
                    );
                });
                row.col(|ui| {
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.add(egui::Label::new(search::format_size(h.size)).selectable(false));
                    });
                });
                row.col(|ui| {
                    ui.add(egui::Label::new(export::format_time(h.modified)).selectable(false));
                });
                row.col(|ui| {
                    let t = if has_content {
                        format_count(h.hits)
                    } else {
                        "–".into()
                    };
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.add(egui::Label::new(t).selectable(false));
                    });
                });
                let resp = row.response();
                if resp.clicked() || resp.secondary_clicked() {
                    clicked = Some(i);
                }
                if resp.double_clicked() {
                    open = Some(i);
                }
                resp.context_menu(|ui| {
                    if ui.button("Open").clicked() {
                        action = Some((i, RowAction::Open));
                        ui.close();
                    }
                    if ui.button("Open containing folder").clicked() {
                        action = Some((i, RowAction::Folder));
                        ui.close();
                    }
                    if ui.button("Open terminal here").clicked() {
                        action = Some((i, RowAction::Terminal));
                        ui.close();
                    }
                    ui.separator();
                    if ui.button("Copy full path").clicked() {
                        action = Some((i, RowAction::CopyPath));
                        ui.close();
                    }
                    if ui.button("Copy file name").clicked() {
                        action = Some((i, RowAction::CopyName));
                        ui.close();
                    }
                });
            });
        });

        if let Some(col) = sort_click {
            self.sort = if self.sort.0 == col {
                (col, !self.sort.1)
            } else {
                (col, true)
            };
            self.needs_sort = true;
        }
        if let Some(i) = clicked {
            self.select(i);
        }
        if let Some(i) = open {
            let p = self.results[i].path.clone();
            self.report(platform::open_file(&p));
        }
        if let Some((i, a)) = action {
            let p = self.results[i].path.clone();
            match a {
                RowAction::Open => self.report(platform::open_file(&p)),
                RowAction::Folder => self.report(platform::show_in_folder(&p)),
                RowAction::Terminal => {
                    let dir = p.parent().unwrap_or(&p).to_path_buf();
                    match platform::open_terminal(&dir) {
                        Err(e) if e == platform::TERMINAL_UNAVAILABLE => {
                            // Sandboxed (Flatpak): we can't start host programs,
                            // so hand the user the path instead.
                            let cmd = format!("cd {}", shell_quote(&dir.to_string_lossy()));
                            ui.ctx().copy_text(cmd.clone());
                            self.message = Some((
                                format!(
                                    "Can't open a terminal from the Flatpak sandbox. Copied \"{cmd}\" to the clipboard; paste it into your terminal."
                                ),
                                false,
                            ));
                        }
                        r => self.report(r),
                    }
                }
                RowAction::CopyPath => ui.ctx().copy_text(p.to_string_lossy().into_owned()),
                RowAction::CopyName => ui.ctx().copy_text(
                    p.file_name()
                        .map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                ),
            }
        }
    }

    fn preview(&mut self, ui: &mut egui::Ui) {
        let Some(h) = self.selected.and_then(|i| self.results.get(i)) else {
            ui.centered_and_justified(|ui| {
                ui.weak("Select a result to preview matching lines.");
            });
            return;
        };
        let hit_idx = self.hit_indices();
        let n_hits = hit_idx.len();
        let mut step: Option<bool> = None;
        let path = h.path.clone();
        ui.horizontal(|ui| {
            ui.add(egui::Label::new(RichText::new(path.to_string_lossy()).strong()).truncate());
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if n_hits > 0 {
                    if ui.button("Next ▼").on_hover_text("F3").clicked() {
                        step = Some(true);
                    }
                    if ui.button("Prev ▲").on_hover_text("Shift+F3").clicked() {
                        step = Some(false);
                    }
                    ui.label(format!(
                        "line {} of {}{}",
                        (self.current_hit + 1).min(n_hits),
                        n_hits,
                        if h.truncated { "+" } else { "" }
                    ));
                }
            });
        });
        ui.separator();
        if let Some(f) = step {
            self.step_hit(f);
        }
        let Some(h) = self.selected.and_then(|i| self.results.get(i)) else {
            return;
        };
        if h.lines.is_empty() {
            ui.weak(format!(
                "{} · {} · modified {}",
                search::format_size(h.size),
                h.path
                    .extension()
                    .map(|e| e.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "file".into()),
                export::format_time(h.modified)
            ));
            if ui.button("Open file").clicked() {
                let p = h.path.clone();
                self.report(platform::open_file(&p));
            }
            return;
        }

        let font = FontId::monospace(13.0);
        let row_h = ui.fonts_mut(|f| f.row_height(&font)) + 2.0;
        let current_line = hit_idx.get(self.current_hit).copied();
        let max_no = h.lines.iter().map(|l| l.number).max().unwrap_or(1);
        let digits = max_no.to_string().len().max(3);
        let vis = ui.visuals().clone();
        let hl_bg = if vis.dark_mode {
            Color32::from_rgb(120, 90, 0)
        } else {
            Color32::from_rgb(255, 225, 100)
        };
        let hl_fg = if vis.dark_mode {
            Color32::WHITE
        } else {
            Color32::BLACK
        };
        let cur_bg = vis.selection.bg_fill.gamma_multiply(0.35);

        let mut sa = egui::ScrollArea::both()
            .auto_shrink([false, false])
            .id_salt("preview");
        if self.scroll_preview {
            self.scroll_preview = false;
            if let Some(cl) = current_line {
                let avail = ui.available_height();
                sa = sa.vertical_scroll_offset((cl as f32 * row_h - avail / 3.0).max(0.0));
            } else {
                sa = sa.vertical_scroll_offset(0.0);
            }
        }
        let lines = &h.lines;
        sa.show_rows(ui, row_h, lines.len(), |ui, range| {
            for i in range {
                let l = &lines[i];
                let mut job = LayoutJob::default();
                let num_fmt = TextFormat {
                    font_id: font.clone(),
                    color: vis.weak_text_color(),
                    ..Default::default()
                };
                if l.number == 0 {
                    job.append(&format!("{:>digits$}  ⋯", ""), 0.0, num_fmt);
                } else {
                    let marker = if l.is_hit { '›' } else { ' ' };
                    job.append(&format!("{:>digits$} {marker} ", l.number), 0.0, num_fmt);
                    let normal = TextFormat {
                        font_id: font.clone(),
                        color: if l.is_hit {
                            vis.text_color()
                        } else {
                            vis.weak_text_color()
                        },
                        ..Default::default()
                    };
                    let strong = TextFormat {
                        font_id: font.clone(),
                        color: hl_fg,
                        background: hl_bg,
                        ..Default::default()
                    };
                    let mut pos = 0;
                    for &(s, e) in &l.ranges {
                        if s < pos || e > l.text.len() {
                            continue;
                        }
                        job.append(&l.text[pos..s], 0.0, normal.clone());
                        job.append(&l.text[s..e], 0.0, strong.clone());
                        pos = e;
                    }
                    job.append(&l.text[pos..], 0.0, normal);
                }
                // Reserve a background slot so the current-hit bar sits under the text.
                let bg = ui.painter().add(egui::Shape::Noop);
                let resp = ui.add(egui::Label::new(job).extend());
                if Some(i) == current_line {
                    let mut r = resp.rect;
                    r.set_width(r.width().max(ui.clip_rect().width()));
                    ui.painter()
                        .set(bg, egui::Shape::rect_filled(r, 2.0, cur_bg));
                }
            }
        });
    }

    fn windows(&mut self, ctx: &egui::Context) {
        if self.show_errors {
            let mut open = true;
            egui::Window::new("Unreadable files and folders")
                .open(&mut open)
                .default_size([640.0, 320.0])
                .show(ctx, |ui| {
                    if ui.button("Copy all").clicked() {
                        ctx.copy_text(self.errors.join("\n"));
                    }
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        for e in &self.errors {
                            ui.monospace(e);
                        }
                        if self.errors.len() >= MAX_ERRORS_KEPT {
                            ui.weak("(list truncated)");
                        }
                    });
                });
            self.show_errors = open;
        }
        if self.show_about {
            let mut open = true;
            egui::Window::new(format!("About {APP_NAME}"))
                .open(&mut open)
                .collapsible(false)
                .resizable(false)
                .show(ctx, |ui| {
                    ui.heading(format!("{APP_NAME} {}", crate::VERSION));
                    ui.label("Fast file search for Linux, written in Rust.");
                    ui.label("Uses the ripgrep search crates and egui.");
                    ui.label("Copyright (C) 2026 Eric Allen. License: GPL-3.0-or-later.");
                    ui.label("Independent project, not affiliated with Mythicsoft.");
                    ui.add_space(6.0);
                    ui.weak("Enter: search · Esc: stop · F3 / Shift+F3: next/prev hit · ↑/↓: move selection · Ctrl+F: focus name");
                });
            self.show_about = open;
        }
    }
}

#[derive(Clone, Copy)]
enum RowAction {
    Open,
    Folder,
    Terminal,
    CopyPath,
    CopyName,
}

/// Quotes a path for a POSIX shell when needed.
fn shell_quote(s: &str) -> String {
    if s.chars()
        .all(|c| c.is_ascii_alphanumeric() || "/._-+~".contains(c))
    {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', r"'\''"))
    }
}

fn format_duration(d: Duration) -> String {
    if d.as_secs_f64() < 1.0 {
        format!("{} ms", d.as_millis())
    } else {
        format!("{:.2} s", d.as_secs_f64())
    }
}

fn format_count(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

impl eframe::App for SeekerApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        if self.auto_start {
            self.auto_start = false;
            self.start_search(&ctx);
        }
        self.poll_search();
        self.apply_sort();
        self.handle_dialogs();
        self.handle_keys(&ctx);
        if self.is_running() {
            ctx.request_repaint_after(Duration::from_millis(100));
        }

        egui::Panel::top("menu").show(ui, |ui| self.menu_bar(ui));
        egui::Panel::top("form")
            .frame(egui::Frame::side_top_panel(ui.style()).inner_margin(8.0))
            .show(ui, |ui| self.search_form(ui));
        egui::Panel::bottom("status").show(ui, |ui| self.status_bar(ui));
        egui::CentralPanel::default().show(ui, |ui| {
            egui::Panel::bottom("preview")
                .resizable(true)
                .default_size(ui.available_height() * 0.45)
                .size_range(80.0..=ui.available_height() - 80.0)
                .show(ui, |ui| self.preview(ui));
            egui::CentralPanel::no_frame().show(ui, |ui| {
                if self.results.is_empty() && !self.is_running() {
                    ui.centered_and_justified(|ui| {
                        ui.weak(if self.last_stats.is_some() {
                            "No matching files."
                        } else {
                            ""
                        });
                    });
                } else {
                    self.results_table(ui);
                }
            });
        });
        self.windows(&ctx);
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        eframe::set_value(storage, STORAGE_KEY, &self.form);
    }
}

pub fn run(args: Args) -> Result<(), String> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(APP_NAME)
            .with_app_id(DESKTOP_ID)
            .with_inner_size([1100.0, 760.0])
            .with_min_inner_size([640.0, 420.0]),
        ..Default::default()
    };
    eframe::run_native(
        APP_ID,
        options,
        Box::new(move |cc| Ok(Box::new(SeekerApp::new(cc, args)))),
    )
    .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn quoting() {
        assert_eq!(shell_quote("/home/a/b"), "/home/a/b");
        assert_eq!(shell_quote("/x y/it's"), r"'/x y/it'\''s'");
    }
    #[test]
    fn counts_have_separators() {
        assert_eq!(format_count(0), "0");
        assert_eq!(format_count(999), "999");
        assert_eq!(format_count(1000), "1,000");
        assert_eq!(format_count(1234567), "1,234,567");
    }
    #[test]
    fn form_roundtrip_to_options() {
        let mut f = Form {
            look_in: "/tmp;/var".into(),
            min_size: "1K".into(),
            ..Default::default()
        };
        let o = f.to_options().unwrap();
        assert_eq!(o.roots.len(), 2);
        assert_eq!(o.min_size, Some(1024));
        f.modified_after = "nope".into();
        assert!(f.to_options().is_err());
    }
}
