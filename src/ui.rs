// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Eric Allen

//! The egui desktop interface.

use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::time::Duration;

use crossbeam_channel::{Receiver, Sender};
use eframe::egui::{
    self, Align, Color32, Key, Layout, RichText, Sense, TextFormat, text::LayoutJob,
};
use egui_extras::{Column, TableBuilder};
use serde::{Deserialize, Serialize};

use crate::cli::Args;
use crate::export;
use crate::platform;
use crate::search::{self, Event, FileHit, NameMode, SearchHandle, SearchOptions, Stats};
use crate::settings::{self, LoadedFont, PrefsAction, PrefsWindow, Settings};
use crate::{APP_ID, APP_NAME, DESKTOP_ID};

const STORAGE_KEY: &str = "form";
const SETTINGS_KEY: &str = "settings";
/// Bumped when form defaults change in a way saved forms should pick up.
/// 2 (0.1.2): hidden files searched and no folders excluded by default.
/// 3 (0.1.2): "Exclude folders" replaced by Include Files / Include Folders.
const FORM_VERSION: u32 = 3;

fn legacy_form_version() -> u32 {
    1
}
const MAX_ERRORS_KEPT: usize = 2_000;

/// Everything typed into the search form. Persisted between runs.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
struct Form {
    /// Missing in forms saved before 0.1.2.
    #[serde(default = "legacy_form_version")]
    form_version: u32,
    name_pattern: String,
    name_mode: NameMode,
    content: String,
    content_regex: bool,
    case_sensitive: bool,
    whole_word: bool,
    look_in: String,
    recurse: bool,
    /// List matching files.
    include_files: bool,
    /// List matching folders (name searches only).
    include_folders: bool,
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
            form_version: FORM_VERSION,
            name_pattern: String::new(),
            name_mode: NameMode::Wildcard,
            content: String::new(),
            content_regex: false,
            case_sensitive: false,
            whole_word: false,
            look_in: std::env::var("HOME").unwrap_or_else(|_| "/".into()),
            recurse: d.recurse,
            include_files: d.include_files,
            include_folders: d.include_folders,
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
    /// Moves a form saved by an older version to the current defaults where
    /// the user hadn't changed them.
    fn migrate(&mut self) {
        if self.form_version < 2 {
            // 0.1.0/0.1.1 skipped hidden files by default, which hid files
            // people expected to find (issue #3).
            self.include_hidden = true;
        }
        if self.form_version < 3 {
            // The old "Exclude folders" list is no longer in the window (#8); it
            // is dropped when the form loads. Files and folders are both listed.
            self.include_files = true;
            self.include_folders = true;
        }
        if !self.include_files && !self.include_folders {
            self.include_files = true;
        }
        self.form_version = FORM_VERSION;
    }

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
            include_files: self.include_files,
            include_folders: self.include_folders,
            exclude_dirs: Vec::new(),
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
        if let Some((files, folders)) = a.types {
            self.include_files = files;
            self.include_folders = folders;
        }
        self.include_hidden |= a.hidden;
        if a.skip_hidden {
            self.include_hidden = false;
        }
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
    /// Rows that fit in the results table (for Page Up / Page Down).
    page_rows: usize,
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
    /// Height the search form needed last frame.
    form_content_h: f32,
    applied_theme_key: Option<bool>,
    /// Live settings: what's shown, including unsaved Preferences changes.
    settings: Settings,
    /// The last saved settings (what Cancel goes back to).
    saved_settings: Settings,
    /// Settings currently applied to the egui context.
    applied: Option<Settings>,
    loaded_font: LoadedFont,
    prefs: PrefsWindow,
}

impl SeekerApp {
    fn new(cc: &eframe::CreationContext<'_>, args: Args) -> Self {
        let mut form: Form = cc
            .storage
            .and_then(|s| eframe::get_value(s, STORAGE_KEY))
            .unwrap_or_default();
        form.migrate();
        form.apply_args(&args);
        let mut settings: Settings = cc
            .storage
            .and_then(|s| eframe::get_value(s, SETTINGS_KEY))
            .unwrap_or_default();
        settings.font_size = settings::snap_font_size(settings.font_size);
        // Text size is set in Preferences only: no Ctrl +/- zoom that can run
        // away, and undo any zoom left over from the first 0.1.2 test build.
        cc.egui_ctx.options_mut(|o| o.zoom_with_keyboard = false);
        cc.egui_ctx.set_zoom_factor(1.0);
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
            page_rows: 10,
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
            form_content_h: 0.0,
            applied_theme_key: None,
            saved_settings: settings.clone(),
            settings,
            applied: None,
            loaded_font: LoadedFont::default(),
            prefs: PrefsWindow::default(),
        }
        .with_settings_applied(&cc.egui_ctx)
    }

    fn with_settings_applied(mut self, ctx: &egui::Context) -> Self {
        self.apply_settings(ctx);
        self
    }

    /// Applies changed settings (theme, colors, font, size) to egui.
    fn apply_settings(&mut self, ctx: &egui::Context) {
        let s = &mut self.settings;
        s.font_size = settings::snap_font_size(s.font_size);
        let system_dark = ctx.system_theme() == Some(egui::Theme::Dark);
        let theme_key = (s.theme == settings::ThemeChoice::System).then_some(system_dark);
        if self.applied.as_ref() == Some(s) && self.applied_theme_key == theme_key {
            return;
        }
        if let Some(e) = self.loaded_font.refresh(s) {
            self.message = Some((e, true));
        }
        let s = &self.settings;
        if self.applied.as_ref().is_none_or(|a| a.font != s.font) {
            ctx.set_fonts(s.font_definitions(&self.loaded_font));
        }
        let visuals = s.visuals(system_dark);
        let theme = if visuals.dark_mode {
            egui::Theme::Dark
        } else {
            egui::Theme::Light
        };
        ctx.set_theme(theme);
        ctx.set_visuals_of(theme, visuals);
        ctx.all_styles_mut(|st| s.apply_style(st));
        self.applied = Some(s.clone());
        self.applied_theme_key = theme_key;
    }

    /// Handles the Save / Cancel / Close buttons of Preferences.
    fn prefs_action(&mut self, action: PrefsAction, frame: &mut eframe::Frame) {
        platform::debug(&format!("preferences action {action:?}"));
        let save = matches!(action, PrefsAction::Save | PrefsAction::SaveAndClose);
        let revert = matches!(action, PrefsAction::Cancel | PrefsAction::DiscardAndClose);
        if save {
            self.saved_settings = self.settings.clone();
            if let Some(st) = frame.storage_mut() {
                eframe::set_value(st, SETTINGS_KEY, &self.saved_settings);
                st.flush();
            }
        }
        if revert {
            self.settings = self.saved_settings.clone();
        }
        if matches!(
            action,
            PrefsAction::SaveAndClose | PrefsAction::DiscardAndClose | PrefsAction::Close
        ) {
            self.prefs.open = false;
        }
    }

    /// "No matching files", plus which form fields narrowed the search.
    fn no_match_message(&self) -> String {
        let f = &self.form;
        let mut why = Vec::new();
        if !f.content.is_empty() {
            why.push(format!(
                "only files containing \"{}\" are listed (clear \"Containing text\" to search by name only)",
                f.content
            ));
        }
        if !f.include_hidden {
            why.push("hidden files are skipped".into());
        }
        if f.respect_gitignore {
            why.push(".gitignore rules are applied".into());
        }
        if !f.include_files {
            why.push("only folders are listed (Include Files is off)".into());
        } else if !f.include_folders {
            why.push("only files are listed (Include Folders is off)".into());
        }
        if !f.recurse {
            why.push("subfolders are not searched".into());
        }
        if why.is_empty() {
            "Nothing found.".into()
        } else {
            format!("Nothing found.\nNote: {}.", why.join("; "))
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
        let (enter, esc, f3, shift, ctrl_f) = ctx.input(|i| {
            (
                i.key_pressed(Key::Enter),
                i.key_pressed(Key::Escape),
                i.key_pressed(Key::F3),
                i.modifiers.shift,
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
        // Moving through the found list works whenever the cursor isn't in a
        // text field.
        if !typing && !self.results.is_empty() {
            let moves = ctx.input_mut(|i| {
                let mut m = Vec::new();
                for (key, mv) in [
                    (Key::ArrowUp, ListMove::Up),
                    (Key::ArrowDown, ListMove::Down),
                    (Key::PageUp, ListMove::PageUp),
                    (Key::PageDown, ListMove::PageDown),
                    (Key::Home, ListMove::Home),
                    (Key::End, ListMove::End),
                ] {
                    // Consume the key so no scroll area also scrolls by it.
                    while i.consume_key(egui::Modifiers::NONE, key) {
                        m.push(mv);
                    }
                }
                m
            });
            for mv in moves {
                let i = list_move(self.selected, self.results.len(), self.page_rows, mv);
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
            ui.menu_button("Settings", |ui| {
                if ui.button("Preferences…").clicked() {
                    platform::debug("open preferences");
                    self.prefs.open();
                    ui.close();
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
        let em = egui::TextStyle::Body.resolve(ui.style()).size;
        let label_w = ["File name:", "Containing text:", "Look in:"]
            .iter()
            .map(|t| text_width(ui, t))
            .fold(0.0, f32::max)
            + 2.0;
        let (mut do_search, mut do_stop, mut browse) = (false, false, false);
        let form = &mut self.form;
        let focus_name = &mut self.focus_name;

        form_row(
            ui,
            "name",
            "File name:",
            label_w,
            |ui, w| {
                let r = ui.add(
                    egui::TextEdit::singleline(&mut form.name_pattern)
                        .hint_text("e.g. *.rs;*.toml;!*.lock   (empty = all files)")
                        .desired_width(w),
                );
                if *focus_name {
                    r.request_focus();
                    *focus_name = false;
                }
            },
            |ui| {
                egui::ComboBox::from_id_salt("name_mode")
                    .selected_text(match form.name_mode {
                        NameMode::Wildcard => "Wildcards",
                        NameMode::Regex => "Regex",
                    })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut form.name_mode, NameMode::Wildcard, "Wildcards");
                        ui.selectable_value(&mut form.name_mode, NameMode::Regex, "Regex");
                    });
            },
        );
        form_row(
            ui,
            "content",
            "Containing text:",
            label_w,
            |ui, w| {
                ui.add(
                    egui::TextEdit::singleline(&mut form.content)
                        .hint_text("text to find inside files (empty = name search only)")
                        .desired_width(w),
                );
            },
            |ui| {
                ui.checkbox(&mut form.content_regex, "Regex");
                ui.checkbox(&mut form.case_sensitive, "Match case");
                ui.checkbox(&mut form.whole_word, "Whole word");
            },
        );
        form_row(
            ui,
            "look_in",
            "Look in:",
            label_w,
            |ui, w| {
                ui.add(
                    egui::TextEdit::singleline(&mut form.look_in)
                        .hint_text("folders, separated by ;")
                        .desired_width(w),
                );
            },
            |ui| {
                if ui
                    .button("Browse…")
                    .on_hover_text("Pick one or more folders")
                    .clicked()
                {
                    browse = true;
                }
                ui.checkbox(&mut form.recurse, "Subfolders");
                let (text, hint) = if running {
                    ("Stop", "Esc")
                } else {
                    ("Search", "Enter")
                };
                if ui
                    .add(
                        egui::Button::new(RichText::new(text).strong())
                            .min_size([em * 5.0, 0.0].into()),
                    )
                    .on_hover_text(hint)
                    .clicked()
                {
                    if running {
                        do_stop = true;
                    } else {
                        do_search = true;
                    }
                }
            },
        );
        if browse {
            self.pick_folders(ui.ctx());
        }
        if do_stop {
            self.stop_search();
        }
        if do_search {
            let ctx = ui.ctx().clone();
            self.start_search(&ctx);
        }

        // Every group wraps as a unit and no field is wider than the window.
        let full_w = ui.available_width();
        let field_w = |w: f32| (w * em / 13.0).min(full_w * 0.6);
        let header = egui::CollapsingHeader::new("Options")
            .open(Some(self.form.show_options))
            .show(ui, |ui| {
                let f = &mut self.form;
                let sp = ui.spacing().item_spacing.x;
                let pad = ui.spacing().button_padding.x * 2.0;
                // Text fields are a little wider than their desired width (frame + margin).
                let fpad = pad + 8.0;
                ui.horizontal_wrapped(|ui| {
                    let icon = ui.spacing().icon_width + ui.spacing().icon_spacing;
                    for (v, t) in [
                        (&mut f.include_hidden, "Hidden files"),
                        (&mut f.respect_gitignore, "Respect .gitignore"),
                        (&mut f.follow_symlinks, "Follow symlinks"),
                        (&mut f.skip_binary, "Skip binary files"),
                    ] {
                        wrap_before(ui, icon + text_width(ui, t));
                        ui.checkbox(v, t);
                    }
                    let combo_w = em * 3.5;
                    wrap_before(ui, text_width(ui, "Context lines:") + sp + combo_w + pad);
                    ui.label("Context lines:");
                    egui::ComboBox::from_id_salt("context_lines")
                        .selected_text(f.context_lines.to_string())
                        .width(combo_w)
                        .show_ui(ui, |ui| {
                            for n in 0..=10 {
                                ui.selectable_value(&mut f.context_lines, n, n.to_string());
                            }
                        });
                });
                ui.horizontal_wrapped(|ui| {
                    let icon = ui.spacing().icon_width + ui.spacing().icon_spacing;
                    // At least one of the two stays on: the last checked box is locked.
                    let files_alone = f.include_files && !f.include_folders;
                    let folders_alone = f.include_folders && !f.include_files;
                    const BOTH_OFF: &str =
                        "At least one of Include Files and Include Folders must be on.";
                    wrap_before(ui, icon + text_width(ui, "Include Files"));
                    ui.add_enabled(
                        !files_alone,
                        egui::Checkbox::new(&mut f.include_files, "Include Files"),
                    )
                    .on_hover_text("List files whose name matches.")
                    .on_disabled_hover_text(BOTH_OFF);
                    wrap_before(ui, icon + text_width(ui, "Include Folders"));
                    ui.add_enabled(
                        !folders_alone,
                        egui::Checkbox::new(&mut f.include_folders, "Include Folders"),
                    )
                    .on_hover_text("List folders whose name matches the File name pattern.")
                    .on_disabled_hover_text(BOTH_OFF);
                    if !f.content.is_empty() && f.include_folders {
                        let hint = if f.include_files {
                            "(folders are not listed when searching for text)"
                        } else {
                            "(folders have no text: turn on Include Files or clear Containing text)"
                        };
                        wrap_before(ui, text_width(ui, hint));
                        ui.weak(hint);
                    }
                    let sz_w = field_w(110.0);
                    wrap_before(
                        ui,
                        text_width(ui, "Size:")
                            + text_width(ui, "–")
                            + 2.0 * (sz_w + fpad)
                            + 4.0 * sp,
                    );
                    ui.label("Size:");
                    ui.add(
                        egui::TextEdit::singleline(&mut f.min_size)
                            .hint_text("min, e.g. 10K")
                            .desired_width(sz_w),
                    );
                    ui.label("–");
                    ui.add(
                        egui::TextEdit::singleline(&mut f.max_size)
                            .hint_text("max, e.g. 5M")
                            .desired_width(sz_w),
                    );
                    let dt_w = field_w(130.0);
                    wrap_before(
                        ui,
                        text_width(ui, "Modified:")
                            + text_width(ui, "–")
                            + 2.0 * (dt_w + fpad)
                            + 4.0 * sp,
                    );
                    ui.label("Modified:");
                    ui.add(
                        egui::TextEdit::singleline(&mut f.modified_after)
                            .hint_text("from YYYY-MM-DD")
                            .desired_width(dt_w),
                    );
                    ui.label("–");
                    ui.add(
                        egui::TextEdit::singleline(&mut f.modified_before)
                            .hint_text("to YYYY-MM-DD")
                            .desired_width(dt_w),
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
                    "Searching…  {} scanned · {} found · {} hits · {:.1} s",
                    fmt(p.files_scanned.load(Ordering::Relaxed)),
                    fmt(p.files_matched.load(Ordering::Relaxed)),
                    fmt(p.total_hits.load(Ordering::Relaxed)),
                    h.elapsed().as_secs_f32()
                ));
            } else if let Some(s) = &self.last_stats {
                ui.label(format!(
                    "{}  {} scanned · {} found · {} hits · {}",
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
        // Header and scroll bar aside, this many rows are visible: one "page".
        self.page_rows = (((ui.available_height() - row_h - 8.0)
            / (row_h + ui.spacing().item_spacing.y))
            .floor() as usize)
            .max(1);
        let has_content = self.results.iter().any(|h| h.hit_lines > 0);
        let size_w = text_width(ui, "999.9 KB") + 16.0;
        let date_w = text_width(ui, "2026-11-16 20:17") + 16.0;
        let hits_w = text_width(ui, "Hits 9999") + 8.0;
        let rest = (ui.available_width() - size_w - date_w - hits_w - 40.0).max(160.0);
        let (name_w, folder_w) = (rest * 0.45, rest * 0.55);

        // Column widths are remembered per window width (in 40-point steps)
        // and text size, so after a resize or a new text size the columns are
        // laid out again to fit instead of running past the window edge.
        let layout_key = (
            (ui.available_width() / 40.0) as u32,
            egui::TextStyle::Body.resolve(ui.style()).size as u32,
        );
        let icon_colors = IconColors::new(ui.visuals());
        let mut tb = TableBuilder::new(ui)
            .id_salt(("results", layout_key))
            .striped(true)
            .resizable(true)
            .sense(Sense::click())
            .cell_layout(Layout::left_to_right(Align::Center))
            // Fixed columns sized for their contents, the two text columns
            // share the rest, so the table fits the window at any text size.
            .column(Column::initial(name_w).at_least(60.0).clip(true))
            .column(Column::initial(folder_w).at_least(60.0).clip(true))
            .column(Column::initial(size_w).at_least(40.0).clip(true))
            .column(Column::initial(date_w).at_least(60.0).clip(true))
            .column(Column::remainder().at_least(30.0).clip(true))
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
                    kind_icon(ui, h.is_dir, &icon_colors);
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
                        let size = if h.is_dir {
                            "–".into()
                        } else {
                            search::format_size(h.size)
                        };
                        ui.add(egui::Label::new(size).selectable(false));
                    });
                });
                row.col(|ui| {
                    ui.add(egui::Label::new(export::format_time(h.modified)).selectable(false));
                });
                row.col(|ui| {
                    let t = if has_content && !h.is_dir {
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
                    if ui.button("Copy name").clicked() {
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
                    // A folder opens a terminal in itself, a file in its folder.
                    let dir = if p.is_dir() {
                        p.clone()
                    } else {
                        p.parent().unwrap_or(&p).to_path_buf()
                    };
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
        if h.is_dir {
            ui.weak(format!(
                "Folder · modified {}",
                export::format_time(h.modified)
            ));
            if ui.button("Open folder").clicked() {
                let p = h.path.clone();
                self.report(platform::open_file(&p));
            }
            return;
        }
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

        let font = egui::TextStyle::Monospace.resolve(ui.style());
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

/// Width of `text` in the body font.
/// A keyboard move in the found list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ListMove {
    Up,
    Down,
    PageUp,
    PageDown,
    Home,
    End,
}

/// The row selected after `mv`, in a list of `n > 0` rows with `page` rows visible.
fn list_move(selected: Option<usize>, n: usize, page: usize, mv: ListMove) -> usize {
    let last = n.saturating_sub(1);
    let page = page.max(1);
    let Some(cur) = selected.map(|i| i.min(last)) else {
        return match mv {
            ListMove::End => last,
            _ => 0,
        };
    };
    match mv {
        ListMove::Up => cur.saturating_sub(1),
        ListMove::Down => (cur + 1).min(last),
        ListMove::PageUp => cur.saturating_sub(page),
        ListMove::PageDown => (cur + page).min(last),
        ListMove::Home => 0,
        ListMove::End => last,
    }
}

/// Colors for the painted file and folder icons.
struct IconColors {
    folder_fill: Color32,
    folder_tab: Color32,
    folder_line: Color32,
    file_fill: Color32,
    file_line: Color32,
}

impl IconColors {
    fn new(v: &egui::Visuals) -> Self {
        let text = v.text_color();
        Self {
            folder_fill: Color32::from_rgb(240, 180, 50),
            folder_tab: Color32::from_rgb(214, 150, 30),
            folder_line: Color32::from_rgb(150, 100, 10),
            file_fill: if v.dark_mode {
                Color32::from_gray(225)
            } else {
                Color32::WHITE
            },
            file_line: text.gamma_multiply(0.75),
        }
    }
}

/// Paints a small folder or file icon sized to the text, so it shows the same
/// whatever font is chosen (no icon font or emoji needed).
fn kind_icon(ui: &mut egui::Ui, is_dir: bool, c: &IconColors) {
    let h = ui.text_style_height(&egui::TextStyle::Body);
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(h * 1.1, h), Sense::hover());
    resp.on_hover_text(if is_dir { "Folder" } else { "File" });
    let p = ui.painter();
    let s = h * 0.9;
    let r = egui::Rect::from_center_size(rect.center(), egui::vec2(s, s));
    let stroke = |c: Color32| egui::Stroke::new((h / 16.0).max(1.0), c);
    if is_dir {
        let tab = egui::Rect::from_min_max(
            egui::pos2(r.left() + s * 0.04, r.top() + s * 0.12),
            egui::pos2(r.left() + s * 0.46, r.top() + s * 0.34),
        );
        p.rect(
            tab,
            s * 0.06,
            c.folder_tab,
            stroke(c.folder_line),
            egui::StrokeKind::Inside,
        );
        let body = egui::Rect::from_min_max(
            egui::pos2(r.left() + s * 0.04, r.top() + s * 0.26),
            egui::pos2(r.right() - s * 0.04, r.bottom() - s * 0.1),
        );
        p.rect(
            body,
            s * 0.08,
            c.folder_fill,
            stroke(c.folder_line),
            egui::StrokeKind::Inside,
        );
    } else {
        let (l, t) = (r.left() + s * 0.16, r.top() + s * 0.04);
        let (rt, b) = (r.right() - s * 0.16, r.bottom() - s * 0.04);
        let fold = s * 0.24;
        let outline = vec![
            egui::pos2(l, t),
            egui::pos2(rt - fold, t),
            egui::pos2(rt, t + fold),
            egui::pos2(rt, b),
            egui::pos2(l, b),
        ];
        p.add(egui::Shape::convex_polygon(
            outline.clone(),
            c.file_fill,
            egui::Stroke::NONE,
        ));
        p.add(egui::Shape::closed_line(outline, stroke(c.file_line)));
        p.add(egui::Shape::line(
            vec![
                egui::pos2(rt - fold, t),
                egui::pos2(rt - fold, t + fold),
                egui::pos2(rt, t + fold),
            ],
            stroke(c.file_line),
        ));
        for k in 0..3 {
            let y = t + s * (0.42 + 0.16 * k as f32);
            p.line_segment(
                [egui::pos2(l + s * 0.12, y), egui::pos2(rt - s * 0.12, y)],
                stroke(c.file_line.gamma_multiply(0.6)),
            );
        }
    }
}

fn text_width(ui: &egui::Ui, text: &str) -> f32 {
    egui::WidgetText::from(text)
        .into_galley(
            ui,
            Some(egui::TextWrapMode::Extend),
            f32::INFINITY,
            egui::TextStyle::Body,
        )
        .size()
        .x
}

/// In a `horizontal_wrapped` layout: starts a new line unless a group of
/// widgets about `w` wide still fits on the current one.
fn wrap_before(ui: &mut egui::Ui, w: f32) {
    let sp = ui.spacing().item_spacing.x;
    if ui.available_size_before_wrap().x < w + sp && ui.cursor().min.x > ui.max_rect().min.x + 1.0 {
        ui.end_row();
    }
}

/// One row of the search form: a label column, a text field that takes the
/// remaining width, and controls on the right. If the controls don't fit next
/// to a usable field (narrow window or large text), they wrap onto the next
/// line under the field instead of running off the window.
fn form_row(
    ui: &mut egui::Ui,
    id: &str,
    label: &str,
    label_w: f32,
    mut field: impl FnMut(&mut egui::Ui, f32),
    mut controls: impl FnMut(&mut egui::Ui),
) {
    let em = egui::TextStyle::Body.resolve(ui.style()).size;
    let sp = ui.spacing().item_spacing.x;
    // Width the controls took on one line, remembered per text size.
    let key = egui::Id::unique(("seeker_form_row", id, em.to_bits()));
    let known: Option<f32> = ui.ctx().data(|d| d.get_temp(key));
    let controls_w = known.unwrap_or(em * 12.0);
    let row_h = ui.spacing().interact_size.y;
    let mut wrapped = false;
    ui.horizontal(|ui| {
        ui.allocate_ui_with_layout(
            egui::vec2(label_w, row_h),
            Layout::left_to_right(Align::Center),
            |ui| {
                ui.set_min_width(label_w);
                ui.label(label);
            },
        );
        let avail = ui.available_width();
        if avail - controls_w - sp >= em * 10.0 {
            field(ui, (avail - controls_w - sp).max(0.0));
            let w = ui.horizontal(|ui| controls(ui)).response.rect.width();
            if known.is_none_or(|k| (k - w).abs() > 0.5) {
                ui.ctx().data_mut(|d| d.insert_temp(key, w));
                ui.ctx().request_repaint();
            }
        } else {
            field(ui, avail.max(0.0));
            wrapped = true;
        }
    });
    if wrapped {
        ui.horizontal(|ui| {
            ui.add_space(label_w + sp);
            ui.horizontal_wrapped(|ui| controls(ui));
        });
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
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.apply_settings(&ctx);
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
        // The form never takes more than about half the window (it scrolls
        // instead), so results and preview stay usable at large text sizes.
        let form_max_h = (ui.available_height() * 0.5).max(120.0);
        egui::Panel::top("form")
            .frame(egui::Frame::side_top_panel(ui.style()).inner_margin(8.0))
            .show(ui, |ui| {
                // A panel keeps last frame's height, so ask for the height the
                // form needed last time (capped) to let it grow when it needs to.
                ui.set_min_height(self.form_content_h.min(form_max_h));
                let out = egui::ScrollArea::vertical()
                    .id_salt("form_scroll")
                    .max_height(form_max_h)
                    .auto_shrink([false, true])
                    .show(ui, |ui| self.search_form(ui));
                let h = out.content_size.y;
                if (h - self.form_content_h).abs() > 0.5 {
                    self.form_content_h = h;
                    ui.ctx().request_repaint();
                }
            });
        egui::Panel::bottom("status").show(ui, |ui| self.status_bar(ui));
        egui::CentralPanel::default().show(ui, |ui| {
            let avail_h = ui.available_height();
            let row_h = ui.spacing().interact_size.y;
            let max_preview = (avail_h - 2.5 * row_h).max(row_h);
            egui::Panel::bottom("preview")
                .resizable(true)
                .default_size(avail_h * 0.45)
                .size_range(row_h.min(max_preview)..=max_preview)
                .show(ui, |ui| self.preview(ui));
            egui::CentralPanel::no_frame().show(ui, |ui| {
                if self.results.is_empty() && !self.is_running() {
                    let msg = if self.last_stats.is_some() {
                        self.no_match_message()
                    } else {
                        String::new()
                    };
                    ui.centered_and_justified(|ui| {
                        ui.weak(msg);
                    });
                } else {
                    self.results_table(ui);
                }
            });
        });
        self.windows(&ctx);
        if let Some(a) = self
            .prefs
            .show(&ctx, &mut self.settings, &self.saved_settings)
        {
            self.prefs_action(a, frame);
        }
        if self.applied.as_ref() != Some(&self.settings) {
            // Changed in Preferences: apply on the next frame.
            ctx.request_repaint();
        }
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        eframe::set_value(storage, STORAGE_KEY, &self.form);
        // Unsaved Preferences changes are not kept (only Save keeps them).
        eframe::set_value(storage, SETTINGS_KEY, &self.saved_settings);
    }
}

pub fn run(args: Args) -> Result<(), String> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(APP_NAME)
            .with_app_id(DESKTOP_ID)
            .with_inner_size([1240.0, 840.0])
            .with_min_inner_size([700.0, 520.0]),
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
    fn list_keys_move_and_clamp() {
        use ListMove::*;
        assert_eq!(list_move(None, 50, 10, Down), 0);
        assert_eq!(list_move(None, 50, 10, End), 49);
        assert_eq!(list_move(Some(0), 50, 10, Up), 0);
        assert_eq!(list_move(Some(5), 50, 10, Down), 6);
        assert_eq!(list_move(Some(5), 50, 10, PageDown), 15);
        assert_eq!(list_move(Some(45), 50, 10, PageDown), 49);
        assert_eq!(list_move(Some(15), 50, 10, PageUp), 5);
        assert_eq!(list_move(Some(3), 50, 10, PageUp), 0);
        assert_eq!(list_move(Some(30), 50, 10, Home), 0);
        assert_eq!(list_move(Some(30), 50, 10, End), 49);
        // A stale selection past the end is clamped; a zero page still moves.
        assert_eq!(list_move(Some(99), 5, 10, Up), 3);
        assert_eq!(list_move(Some(2), 5, 0, PageDown), 3);
    }

    #[test]
    fn old_saved_form_is_migrated() {
        // A form saved by 0.1.1 (no form_version, old defaults).
        let old = r#"(name_pattern: "*.md", include_hidden: false, exclude_dirs: ".git;node_modules;target;__pycache__")"#;
        let mut f: Form = ron::from_str(old).unwrap();
        assert_eq!(f.form_version, 1);
        f.migrate();
        assert!(f.include_hidden);
        assert!(f.include_files && f.include_folders);
        assert_eq!(f.name_pattern, "*.md");
        assert!(f.to_options().unwrap().exclude_dirs.is_empty());
        // A form saved by build 2 (version 2) with a custom exclude list:
        // the list is dropped and both files and folders are listed.
        let mut f: Form =
            ron::from_str(r#"(form_version: 2, exclude_dirs: "build", include_hidden: false)"#)
                .unwrap();
        f.migrate();
        assert!(!f.include_hidden);
        assert!(f.include_files && f.include_folders);
        assert_eq!(f.form_version, FORM_VERSION);
        // Choices made in the current version are left alone...
        let mut f = Form {
            include_hidden: false,
            include_files: false,
            ..Default::default()
        };
        f.migrate();
        assert!(!f.include_hidden);
        assert!(!f.include_files && f.include_folders);
        // ...but never both kinds turned off.
        let mut f = Form {
            include_files: false,
            include_folders: false,
            ..Default::default()
        };
        f.migrate();
        assert!(f.include_files);
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
