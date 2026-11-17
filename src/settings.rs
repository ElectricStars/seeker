// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Eric Allen

//! Appearance and font settings: the Preferences window, how the choices are
//! applied to egui, and system font discovery (through fontconfig's `fc-list`).

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use eframe::egui::{
    self, Color32, FontData, FontDefinitions, FontFamily, FontId, RichText, TextStyle, Theme,
};
use serde::{Deserialize, Serialize};

/// Body text size egui was designed around; widget sizes scale from it.
const EGUI_BODY_SIZE: f32 = 13.0;
pub const DEFAULT_FONT_SIZE: f32 = 16.0;
/// The text sizes offered (points). No sliders: a list and -/+ buttons.
pub const FONT_SIZES: &[f32] = &[
    10.0, 11.0, 12.0, 13.0, 14.0, 15.0, 16.0, 17.0, 18.0, 20.0, 22.0, 24.0, 26.0, 28.0, 30.0, 32.0,
];
/// WCAG AA contrast for normal text.
pub const GOOD_CONTRAST: f32 = 4.5;

const UBUNTU: &str = "Ubuntu-Light";
const HACK: &str = "Hack";
const USER_FONT_KEY: &str = "seeker-user-font";

/// The listed size closest to `size`.
pub fn snap_font_size(size: f32) -> f32 {
    let size = if size.is_finite() {
        size
    } else {
        DEFAULT_FONT_SIZE
    };
    FONT_SIZES
        .iter()
        .copied()
        .min_by(|a, b| (a - size).abs().total_cmp(&(b - size).abs()))
        .unwrap_or(DEFAULT_FONT_SIZE)
}

/// Next listed size up (`up`) or down from `size`, staying in range.
pub fn step_font_size(size: f32, up: bool) -> f32 {
    let i = FONT_SIZES
        .iter()
        .position(|s| *s == snap_font_size(size))
        .unwrap_or(0);
    let j = if up {
        (i + 1).min(FONT_SIZES.len() - 1)
    } else {
        i.saturating_sub(1)
    };
    FONT_SIZES[j]
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ThemeChoice {
    Light,
    Dark,
    /// Follow the desktop's light/dark preference.
    System,
    /// Light or dark is picked from the chosen background color.
    Custom,
}

/// A font: one of the fonts built into Seeker, or a file found by fontconfig.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum FontChoice {
    /// egui font key ("Ubuntu-Light" or "Hack").
    Builtin(String),
    System {
        family: String,
        path: PathBuf,
        index: u32,
    },
}

impl FontChoice {
    pub fn ubuntu() -> Self {
        FontChoice::Builtin(UBUNTU.into())
    }
    pub fn hack() -> Self {
        FontChoice::Builtin(HACK.into())
    }
    pub fn label(&self) -> String {
        match self {
            FontChoice::Builtin(k) if k == HACK => "Hack (built-in, monospace)".into(),
            FontChoice::Builtin(_) => "Ubuntu Light (built-in)".into(),
            FontChoice::System { family, .. } => family.clone(),
        }
    }
}

/// Saved preferences. Unknown fields in older saved files (`mono_font`,
/// `ui_scale` from the first 0.1.2 test build) are ignored.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub theme: ThemeChoice,
    /// Background for `ThemeChoice::Custom` (sRGB).
    pub background: [u8; 3],
    /// Use `text_color` instead of the automatic black/white.
    pub custom_text: bool,
    pub text_color: [u8; 3],
    /// One font for the whole window, preview pane included.
    #[serde(alias = "ui_font")]
    pub font: FontChoice,
    /// Text size in points, one of `FONT_SIZES`.
    pub font_size: f32,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            theme: ThemeChoice::Light,
            background: [250, 246, 232],
            custom_text: false,
            text_color: [20, 20, 20],
            font: FontChoice::ubuntu(),
            font_size: DEFAULT_FONT_SIZE,
        }
    }
}

// ------------------------------------------------------------------ colors

fn channel_lin(c: u8) -> f32 {
    let c = c as f32 / 255.0;
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// WCAG relative luminance, 0 (black) ..= 1 (white).
pub fn luminance(c: [u8; 3]) -> f32 {
    0.2126 * channel_lin(c[0]) + 0.7152 * channel_lin(c[1]) + 0.0722 * channel_lin(c[2])
}

/// WCAG contrast ratio, 1 ..= 21.
pub fn contrast(a: [u8; 3], b: [u8; 3]) -> f32 {
    let (la, lb) = (luminance(a), luminance(b));
    let (hi, lo) = if la > lb { (la, lb) } else { (lb, la) };
    (hi + 0.05) / (lo + 0.05)
}

/// The more readable of near-black and near-white on `bg`.
pub fn auto_text_color(bg: [u8; 3]) -> [u8; 3] {
    const DARK: [u8; 3] = [20, 20, 20];
    const LIGHT: [u8; 3] = [240, 240, 240];
    if contrast(bg, DARK) >= contrast(bg, LIGHT) {
        DARK
    } else {
        LIGHT
    }
}

fn is_dark(bg: [u8; 3]) -> bool {
    auto_text_color(bg)[0] > 128
}

fn rgb(c: [u8; 3]) -> Color32 {
    Color32::from_rgb(c[0], c[1], c[2])
}

/// Moves `c` towards `to` by `t` (0..=1).
fn mix(c: [u8; 3], to: [u8; 3], t: f32) -> [u8; 3] {
    let m = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * t).round() as u8;
    [m(c[0], to[0]), m(c[1], to[1]), m(c[2], to[2])]
}

impl Settings {
    /// Background actually shown, for the contrast check and preview.
    pub fn effective_background(&self, system_dark: bool) -> [u8; 3] {
        match self.theme {
            ThemeChoice::Custom => self.background,
            ThemeChoice::Light => [248, 248, 248],
            ThemeChoice::Dark => [27, 27, 27],
            ThemeChoice::System if system_dark => [27, 27, 27],
            ThemeChoice::System => [248, 248, 248],
        }
    }

    pub fn effective_text(&self, system_dark: bool) -> [u8; 3] {
        let bg = self.effective_background(system_dark);
        if self.custom_text {
            self.text_color
        } else {
            auto_text_color(bg)
        }
    }

    /// Builds the visuals for the chosen theme / colors.
    pub fn visuals(&self, system_dark: bool) -> egui::Visuals {
        let dark = match self.theme {
            ThemeChoice::Light => false,
            ThemeChoice::Dark => true,
            ThemeChoice::System => system_dark,
            ThemeChoice::Custom => is_dark(self.background),
        };
        let mut v = if dark {
            egui::Visuals::dark()
        } else {
            egui::Visuals::light()
        };
        let text = self.effective_text(system_dark);
        if self.theme == ThemeChoice::Custom {
            let bg = self.background;
            // Shades move away from the background, towards the text color.
            let toward = auto_text_color(bg);
            let away = [255, 255, 255];
            let shade = |t: f32| rgb(mix(bg, toward, t));
            v.panel_fill = rgb(bg);
            // Fields stand out from the background: lighter on light colors
            // (towards white) and slightly lighter on dark ones too, since
            // "darker than black" isn't possible.
            let field = if dark {
                shade(0.09)
            } else {
                rgb(mix(bg, away, 0.7))
            };
            v.window_fill = if dark {
                shade(0.05)
            } else {
                rgb(mix(bg, away, 0.35))
            };
            v.faint_bg_color = shade(0.04);
            v.extreme_bg_color = field;
            v.text_edit_bg_color = Some(field);
            v.code_bg_color = shade(0.08);
            v.window_stroke.color = shade(0.3);
            let w = &mut v.widgets;
            w.noninteractive.bg_fill = rgb(bg);
            w.noninteractive.weak_bg_fill = rgb(bg);
            w.noninteractive.bg_stroke.color = shade(0.25);
            for (wv, t) in [
                (&mut w.inactive, 0.10),
                (&mut w.hovered, 0.18),
                (&mut w.active, 0.26),
                (&mut w.open, 0.14),
            ] {
                wv.bg_fill = shade(t);
                wv.weak_bg_fill = shade(t);
            }
            w.inactive.bg_stroke.color = shade(0.3);
        }
        if self.theme == ThemeChoice::Custom || self.custom_text {
            let t = rgb(text);
            v.override_text_color = Some(t);
            for wv in [
                &mut v.widgets.noninteractive,
                &mut v.widgets.inactive,
                &mut v.widgets.hovered,
                &mut v.widgets.active,
                &mut v.widgets.open,
            ] {
                wv.fg_stroke.color = t;
            }
        }
        v
    }

    /// Text sizes and widget spacing, scaled from egui's defaults.
    pub fn apply_style(&self, style: &mut egui::Style) {
        let s = snap_font_size(self.font_size);
        let k = s / EGUI_BODY_SIZE;
        style.text_styles = BTreeMap::from([
            (TextStyle::Small, FontId::proportional((s * 0.8).round())),
            (TextStyle::Body, FontId::proportional(s)),
            (TextStyle::Button, FontId::proportional(s)),
            (TextStyle::Heading, FontId::proportional((s * 1.4).round())),
            (TextStyle::Monospace, FontId::monospace(s)),
        ]);
        let d = egui::style::Spacing::default();
        let sp = &mut style.spacing;
        sp.item_spacing = d.item_spacing * k;
        sp.button_padding = d.button_padding * k + egui::vec2(1.0, 2.0);
        sp.interact_size = d.interact_size * k + egui::vec2(0.0, 4.0);
        sp.icon_width = d.icon_width * k;
        sp.icon_width_inner = d.icon_width_inner * k;
        sp.icon_spacing = d.icon_spacing * k;
        sp.combo_width = d.combo_width * k;
        sp.text_edit_width = d.text_edit_width * k;
        sp.indent = d.indent * k;
        sp.menu_margin = d.menu_margin;
    }

    pub fn font_definitions(&self, loaded: &LoadedFont) -> FontDefinitions {
        let mut fonts = FontDefinitions::default();
        let prop = fonts.families.entry(FontFamily::Proportional).or_default();
        // Hack as a fallback so symbols like ▲▼ render.
        prop.insert(1.min(prop.len()), HACK.to_owned());
        let first = match (&self.font, loaded.data.as_ref()) {
            (FontChoice::System { .. }, Some(d)) => {
                fonts.font_data.insert(USER_FONT_KEY.to_owned(), d.clone());
                USER_FONT_KEY.to_owned()
            }
            (FontChoice::Builtin(k), _) if k == HACK || k == UBUNTU => k.clone(),
            _ => return fonts,
        };
        // The same font everywhere, preview pane (monospace family) included.
        for fam in [FontFamily::Proportional, FontFamily::Monospace] {
            let list = fonts.families.entry(fam).or_default();
            list.retain(|n| *n != first);
            list.insert(0, first.clone());
        }
        fonts
    }
}

// ------------------------------------------------------------ font files

/// The font file read from disk for the current choice.
#[derive(Default)]
pub struct LoadedFont {
    src: Option<FontChoice>,
    data: Option<Arc<FontData>>,
}

fn load_font(choice: &FontChoice) -> Result<Option<Arc<FontData>>, String> {
    match choice {
        FontChoice::Builtin(_) => Ok(None),
        FontChoice::System {
            family,
            path,
            index,
        } => {
            let bytes = std::fs::read(path)
                .map_err(|e| format!("Can't load font \"{family}\" ({}): {e}", path.display()))?;
            let mut d = FontData::from_owned(bytes);
            d.index = *index;
            Ok(Some(Arc::new(d)))
        }
    }
}

impl LoadedFont {
    /// Loads the file if the choice changed. On failure the choice falls back
    /// to the built-in font and the error is returned.
    pub fn refresh(&mut self, s: &mut Settings) -> Option<String> {
        if self.src.as_ref() == Some(&s.font) {
            return None;
        }
        let mut err = None;
        match load_font(&s.font) {
            Ok(d) => self.data = d,
            Err(e) => {
                err = Some(e);
                s.font = FontChoice::ubuntu();
                self.data = None;
            }
        }
        self.src = Some(s.font.clone());
        err
    }
}

/// A font family found on the system.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SystemFont {
    pub family: String,
    pub path: PathBuf,
    pub index: u32,
    pub monospace: bool,
}

/// Rank of a style name for choosing a family's "regular" face (lower is better).
fn style_rank(style: &str) -> u8 {
    let s = style.to_ascii_lowercase();
    if s == "regular" || s == "book" || s == "normal" || s == "roman" {
        0
    } else if s.contains("italic") || s.contains("oblique") {
        3
    } else if s.contains("bold") || s.contains("black") || s.contains("heavy") {
        2
    } else {
        1
    }
}

/// Parses `fc-list --format '%{family[0]}\t%{style[0]}\t%{file}\t%{index}\t%{spacing}\n'`
/// into one entry per family (its most regular face), sorted by name.
pub fn parse_fc_list(out: &str) -> Vec<SystemFont> {
    let mut best: BTreeMap<String, (u8, SystemFont)> = BTreeMap::new();
    for line in out.lines() {
        let f: Vec<&str> = line.split('\t').collect();
        if f.len() < 4 {
            continue;
        }
        let (family, style, file) = (f[0].trim(), f[1].trim(), f[2].trim());
        let lower = file.to_ascii_lowercase();
        if family.is_empty()
            || !(lower.ends_with(".ttf") || lower.ends_with(".otf") || lower.ends_with(".ttc"))
        {
            continue;
        }
        let index = f[3].trim().parse::<u32>().unwrap_or(0);
        // Variable-font instances share the file but have index > 0xFFFF.
        let index = if index > 0xFFFF {
            index & 0xFFFF
        } else {
            index
        };
        let monospace = f.get(4).is_some_and(|s| s.trim() == "100");
        let rank = style_rank(style);
        let font = SystemFont {
            family: family.to_string(),
            path: PathBuf::from(file),
            index,
            monospace,
        };
        match best.get(family) {
            Some((r, _)) if *r <= rank => {}
            _ => {
                best.insert(family.to_string(), (rank, font));
            }
        }
    }
    let mut v: Vec<SystemFont> = best.into_values().map(|(_, f)| f).collect();
    v.sort_by_key(|f| f.family.to_lowercase());
    v
}

/// Lists installed fonts with fontconfig. Empty if `fc-list` isn't available.
pub fn discover_system_fonts() -> Vec<SystemFont> {
    let out = std::process::Command::new("fc-list")
        .args([
            "--format",
            "%{family[0]}\t%{style[0]}\t%{file}\t%{index}\t%{spacing}\n",
        ])
        .output();
    match out {
        Ok(o) if o.status.success() => parse_fc_list(&String::from_utf8_lossy(&o.stdout)),
        _ => Vec::new(),
    }
}

// ------------------------------------------------------------- window

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Page {
    #[default]
    Appearance,
    Font,
}

/// What the user asked for with the buttons at the bottom of Preferences.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrefsAction {
    /// Persist the current settings; keep the window open.
    Save,
    /// Go back to the last saved settings; keep the window open.
    Cancel,
    /// Save, then close (from the "unsaved changes" question).
    SaveAndClose,
    /// Go back to the last saved settings and close.
    DiscardAndClose,
    /// Close; nothing to save.
    Close,
}

/// State of the Preferences window that isn't saved.
#[derive(Default)]
pub struct PrefsWindow {
    pub open: bool,
    pub page: Page,
    /// Showing "Save changes before closing?".
    confirm_close: bool,
    /// When the window was opened; it's redrawn for a moment after that,
    /// because a new native window can map after its first frame was drawn.
    opened_at: Option<std::time::Instant>,
    filter: String,
    fonts: Option<Vec<SystemFont>>,
    fonts_rx: Option<crossbeam_channel::Receiver<Vec<SystemFont>>>,
}

const PRESETS: &[(&str, [u8; 3])] = &[
    ("White", [255, 255, 255]),
    ("Paper", [250, 246, 232]),
    ("Light gray", [228, 228, 228]),
    ("Sepia", [240, 225, 196]),
    ("Sky", [220, 234, 248]),
    ("Mint", [220, 242, 228]),
    ("Slate", [52, 61, 70]),
    ("Charcoal", [40, 40, 40]),
    ("Black", [0, 0, 0]),
];

pub const PREFS_TITLE: &str = "Seeker Preferences";

impl PrefsWindow {
    pub fn open(&mut self) {
        self.open = true;
        self.confirm_close = false;
        self.opened_at = Some(std::time::Instant::now());
    }

    fn poll_fonts(&mut self, ctx: &egui::Context) {
        if self.fonts.is_some() {
            return;
        }
        match &self.fonts_rx {
            None => {
                let (tx, rx) = crossbeam_channel::bounded(1);
                let ctx = ctx.clone();
                std::thread::spawn(move || {
                    let _ = tx.send(discover_system_fonts());
                    ctx.request_repaint();
                });
                self.fonts_rx = Some(rx);
            }
            Some(rx) => {
                if let Ok(f) = rx.try_recv() {
                    self.fonts = Some(f);
                    self.fonts_rx = None;
                }
            }
        }
    }

    /// Shows Preferences as its own desktop window (an egui viewport). Where
    /// separate windows aren't supported egui embeds it in the main window.
    /// `s` is edited live; `saved` is what's on disk.
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        s: &mut Settings,
        saved: &Settings,
    ) -> Option<PrefsAction> {
        if !self.open {
            return None;
        }
        // Sized from the saved text size so the window doesn't jump around
        // while sizes are being tried.
        let em = snap_font_size(saved.font_size);
        let builder = egui::ViewportBuilder::default()
            .with_title(PREFS_TITLE)
            .with_app_id(crate::DESKTOP_ID)
            .with_inner_size([(em * 44.0).min(1100.0), (em * 34.0).min(820.0)])
            .with_min_inner_size([480.0, 400.0]);
        let id = egui::ViewportId::from_hash_of("seeker-preferences");
        if self
            .opened_at
            .is_some_and(|t| t.elapsed() < std::time::Duration::from_millis(1500))
        {
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
        }
        ctx.show_viewport_immediate(id, builder, |ui, class| {
            crate::platform::debug(&format!(
                "preferences viewport frame (embedded: {})",
                class == egui::ViewportClass::EmbeddedWindow
            ));
            let dirty = s != saved;
            if ui.ctx().input(|i| i.viewport().close_requested()) {
                // The title-bar close button works like "Close".
                if dirty {
                    ui.ctx()
                        .send_viewport_cmd(egui::ViewportCommand::CancelClose);
                    self.confirm_close = true;
                } else {
                    return Some(PrefsAction::Close);
                }
            }
            if class == egui::ViewportClass::EmbeddedWindow {
                ui.set_min_size(egui::vec2(em * 40.0, em * 30.0));
            }
            // The Preferences window keeps the saved text size while sizes
            // are tried, so its buttons stay put; the main window previews.
            saved.apply_style(ui.style_mut());
            self.contents(ui, s, dirty)
        })
    }

    fn contents(
        &mut self,
        ui: &mut egui::Ui,
        s: &mut Settings,
        dirty: bool,
    ) -> Option<PrefsAction> {
        let em = egui::TextStyle::Body.resolve(ui.style()).size;
        let pad = em.round();
        let style = ui.style().clone();
        let mut action = None;

        egui::Panel::bottom("prefs_buttons")
            .resizable(false)
            .frame(
                egui::Frame::side_top_panel(&style)
                    .inner_margin(egui::Margin::symmetric(pad as i8, (pad * 0.6) as i8)),
            )
            .show(ui, |ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if self.confirm_close {
                        if ui.button("Keep editing").clicked() {
                            self.confirm_close = false;
                        }
                        if ui.button("Discard").clicked() {
                            action = Some(PrefsAction::DiscardAndClose);
                        }
                        if ui.button(RichText::new("Save").strong()).clicked() {
                            action = Some(PrefsAction::SaveAndClose);
                        }
                        ui.add(
                            egui::Label::new(
                                RichText::new("Save your changes before closing?").strong(),
                            )
                            .wrap(),
                        );
                        return;
                    }
                    if ui
                        .add_enabled(dirty, egui::Button::new(RichText::new("Save").strong()))
                        .on_hover_text("Keep these settings")
                        .clicked()
                    {
                        action = Some(PrefsAction::Save);
                    }
                    if ui
                        .add_enabled(dirty, egui::Button::new("Cancel"))
                        .on_hover_text("Go back to the last saved settings")
                        .clicked()
                    {
                        action = Some(PrefsAction::Cancel);
                    }
                    if ui.button("Close").clicked() {
                        if dirty {
                            self.confirm_close = true;
                        } else {
                            action = Some(PrefsAction::Close);
                        }
                    }
                    if dirty {
                        ui.add(egui::Label::new(RichText::new("Unsaved changes").weak()).wrap());
                    }
                });
            });

        egui::Panel::left("prefs_nav")
            .resizable(false)
            .exact_size(em * 10.0)
            .frame(egui::Frame::side_top_panel(&style).inner_margin(pad * 0.75))
            .show(ui, |ui| {
                ui.add_space(pad * 0.25);
                for (page, label) in [
                    (Page::Appearance, "Appearance"),
                    (Page::Font, "Font and Size"),
                ] {
                    let sel = self.page == page;
                    let b = egui::Button::selectable(sel, RichText::new(label).size(em))
                        .min_size(egui::vec2(ui.available_width(), em * 2.0));
                    if ui.add(b).clicked() {
                        self.page = page;
                    }
                }
            });

        egui::CentralPanel::default()
            .frame(egui::Frame::central_panel(&style).inner_margin(pad))
            .show(ui, |ui| {
                egui::Frame::group(&style)
                    .inner_margin(pad)
                    .corner_radius(6.0)
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        egui::ScrollArea::vertical()
                            .auto_shrink([false, false])
                            .show(ui, |ui| match self.page {
                                Page::Appearance => {
                                    ui.heading("Appearance");
                                    ui.add_space(pad * 0.5);
                                    self.appearance(ui, s);
                                }
                                Page::Font => {
                                    ui.heading("Font and Size");
                                    ui.add_space(pad * 0.5);
                                    self.poll_fonts(ui.ctx());
                                    self.font(ui, s);
                                }
                            });
                    });
            });
        action
    }

    fn appearance(&mut self, ui: &mut egui::Ui, s: &mut Settings) {
        let em = egui::TextStyle::Body.resolve(ui.style()).size;
        let system_dark = ui.ctx().system_theme() == Some(Theme::Dark);
        ui.label(RichText::new("Theme").strong());
        ui.horizontal_wrapped(|ui| {
            ui.radio_value(&mut s.theme, ThemeChoice::Light, "Light");
            ui.radio_value(&mut s.theme, ThemeChoice::Dark, "Dark");
            ui.radio_value(&mut s.theme, ThemeChoice::System, "Follow system");
            ui.radio_value(&mut s.theme, ThemeChoice::Custom, "Custom background");
        });
        ui.add_space(6.0);
        ui.label(RichText::new("Background color").strong());
        ui.horizontal_wrapped(|ui| {
            if egui::color_picker::color_edit_button_srgb(ui, &mut s.background).changed() {
                s.theme = ThemeChoice::Custom;
            }
            ui.label("Pick any color, or a preset:");
        });
        ui.horizontal_wrapped(|ui| {
            for (name, c) in PRESETS {
                let (r, resp) =
                    ui.allocate_exact_size(egui::vec2(em * 1.8, em * 1.4), egui::Sense::click());
                let selected = s.theme == ThemeChoice::Custom && s.background == *c;
                ui.painter().rect_filled(r, 3.0, rgb(*c));
                let stroke = if selected {
                    egui::Stroke::new(3.0, ui.visuals().selection.bg_fill)
                } else {
                    egui::Stroke::new(1.0, ui.visuals().widgets.noninteractive.bg_stroke.color)
                };
                ui.painter()
                    .rect_stroke(r, 3.0, stroke, egui::StrokeKind::Outside);
                if resp.on_hover_text(*name).clicked() {
                    s.background = *c;
                    s.theme = ThemeChoice::Custom;
                }
            }
        });
        ui.add_space(6.0);
        ui.label(RichText::new("Text color").strong());
        ui.horizontal_wrapped(|ui| {
            ui.radio_value(&mut s.custom_text, false, "Automatic (best contrast)");
            ui.radio_value(&mut s.custom_text, true, "Custom:");
            if egui::color_picker::color_edit_button_srgb(ui, &mut s.text_color).changed() {
                s.custom_text = true;
            }
        });
        let bg = s.effective_background(system_dark);
        let fg = s.effective_text(system_dark);
        let ratio = contrast(bg, fg);
        ui.add_space(6.0);
        egui::Frame::new()
            .fill(rgb(bg))
            .stroke(egui::Stroke::new(1.0, rgb(mix(bg, fg, 0.3))))
            .corner_radius(4.0)
            .inner_margin(em * 0.75)
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(
                    RichText::new("Sample: the quick brown fox jumps over the lazy dog.")
                        .color(rgb(fg)),
                );
                ui.label(
                    RichText::new("README.md   ~/Documents/notes   4.2 KB")
                        .monospace()
                        .color(rgb(fg)),
                );
            });
        if ratio >= GOOD_CONTRAST {
            ui.label(format!("Contrast {ratio:.1}:1 — easy to read."));
        } else {
            ui.colored_label(
                ui.visuals().warn_fg_color,
                format!(
                    "Contrast {ratio:.1}:1 — hard to read (aim for {GOOD_CONTRAST}:1 or more). \
                     Try \"Automatic\" text color."
                ),
            );
        }
        ui.add_space(6.0);
        if ui.button("Reset appearance").clicked() {
            let d = Settings::default();
            s.theme = d.theme;
            s.background = d.background;
            s.custom_text = d.custom_text;
            s.text_color = d.text_color;
        }
    }

    fn font(&mut self, ui: &mut egui::Ui, s: &mut Settings) {
        ui.label(RichText::new("Text size").strong());
        ui.horizontal(|ui| {
            let cur = snap_font_size(s.font_size);
            if ui
                .add_enabled(cur > FONT_SIZES[0], egui::Button::new("  −  "))
                .on_hover_text("Smaller text")
                .clicked()
            {
                s.font_size = step_font_size(cur, false);
            }
            egui::ComboBox::from_id_salt("font_size")
                .selected_text(format!("{cur:.0} pt"))
                .height(f32::INFINITY)
                .width(ui.spacing().interact_size.x * 2.2)
                .show_ui(ui, |ui| {
                    for &sz in FONT_SIZES {
                        let label = if sz == DEFAULT_FONT_SIZE {
                            format!("{sz:.0} pt (default)")
                        } else {
                            format!("{sz:.0} pt")
                        };
                        ui.selectable_value(&mut s.font_size, sz, label);
                    }
                });
            if ui
                .add_enabled(
                    cur < FONT_SIZES[FONT_SIZES.len() - 1],
                    egui::Button::new("  +  "),
                )
                .on_hover_text("Larger text")
                .clicked()
            {
                s.font_size = step_font_size(cur, true);
            }
        });
        ui.weak("Fields, buttons and spacing grow with the text.");
        ui.add_space(8.0);

        ui.label(RichText::new("Font").strong());
        ui.horizontal(|ui| {
            ui.label("Current:");
            ui.label(RichText::new(s.font.label()).strong());
        });
        // Sample in the chosen font and size (this window itself keeps the
        // saved size until Save).
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.add(
                egui::Label::new(
                    RichText::new("The quick brown fox jumps over the lazy dog. 0123456789")
                        .size(snap_font_size(s.font_size)),
                )
                .wrap(),
            );
        });
        ui.horizontal(|ui| {
            ui.label("Find:");
            ui.add(
                egui::TextEdit::singleline(&mut self.filter)
                    .hint_text("type part of a font name")
                    .desired_width(ui.available_width()),
            );
        });
        let filter = self.filter.to_lowercase();
        let row_h = ui.text_style_height(&TextStyle::Body) + ui.spacing().item_spacing.y + 4.0;
        egui::Frame::group(ui.style()).show(ui, |ui| {
            egui::ScrollArea::vertical()
                .id_salt("font_list")
                .max_height(row_h * 7.0)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    for b in [FontChoice::ubuntu(), FontChoice::hack()] {
                        let label = b.label();
                        if (filter.is_empty() || label.to_lowercase().contains(&filter))
                            && ui.selectable_label(s.font == b, label).clicked()
                        {
                            s.font = b;
                        }
                    }
                    match &self.fonts {
                        None => {
                            ui.horizontal(|ui| {
                                ui.spinner();
                                ui.weak("Looking for installed fonts…");
                            });
                        }
                        Some(list) if list.is_empty() => {
                            ui.weak("No system fonts found (fontconfig's fc-list is missing).");
                        }
                        Some(list) => {
                            for f in list.iter().filter(|f| {
                                filter.is_empty() || f.family.to_lowercase().contains(&filter)
                            }) {
                                let sel = matches!(&s.font, FontChoice::System { family, .. } if *family == f.family);
                                let label = if f.monospace {
                                    format!("{}  (monospace)", f.family)
                                } else {
                                    f.family.clone()
                                };
                                if ui.selectable_label(sel, label).clicked() {
                                    s.font = FontChoice::System {
                                        family: f.family.clone(),
                                        path: f.path.clone(),
                                        index: f.index,
                                    };
                                }
                            }
                        }
                    }
                });
        });
        ui.add_space(8.0);
        if ui.button("Reset font and size").clicked() {
            let d = Settings::default();
            s.font = d.font;
            s.font_size = d.font_size;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contrast_and_auto_text() {
        assert!((contrast([0, 0, 0], [255, 255, 255]) - 21.0).abs() < 0.01);
        assert!((contrast([9, 9, 9], [9, 9, 9]) - 1.0).abs() < 0.001);
        assert_eq!(auto_text_color([0, 0, 0]), [240, 240, 240]);
        assert_eq!(auto_text_color([255, 255, 255]), [20, 20, 20]);
        assert_eq!(auto_text_color([250, 246, 232]), [20, 20, 20]);
        // Automatic text is always readable on any preset.
        for (_, c) in PRESETS {
            assert!(contrast(*c, auto_text_color(*c)) >= GOOD_CONTRAST, "{c:?}");
        }
    }

    #[test]
    fn default_is_light_and_larger() {
        let s = Settings::default();
        assert_eq!(s.theme, ThemeChoice::Light);
        assert!(!s.visuals(true).dark_mode);
        assert!(s.font_size > EGUI_BODY_SIZE);
        let mut st = egui::Style::default();
        s.apply_style(&mut st);
        assert_eq!(st.text_styles[&TextStyle::Body].size, DEFAULT_FONT_SIZE);
        // One size for everything, preview pane included.
        assert_eq!(
            st.text_styles[&TextStyle::Monospace].size,
            DEFAULT_FONT_SIZE
        );
        assert!(st.spacing.interact_size.y > egui::style::Spacing::default().interact_size.y);
    }

    #[test]
    fn font_sizes_snap_and_step_without_running_away() {
        assert!(FONT_SIZES.contains(&DEFAULT_FONT_SIZE));
        assert_eq!(snap_font_size(16.4), 16.0);
        assert_eq!(snap_font_size(19.0), 18.0);
        assert_eq!(snap_font_size(500.0), 32.0);
        assert_eq!(snap_font_size(-3.0), 10.0);
        assert_eq!(snap_font_size(f32::NAN), DEFAULT_FONT_SIZE);
        assert_eq!(step_font_size(18.0, true), 20.0);
        assert_eq!(step_font_size(18.0, false), 17.0);
        assert_eq!(step_font_size(32.0, true), 32.0);
        assert_eq!(step_font_size(10.0, false), 10.0);
        // A huge saved size can't blow up the layout.
        let s = Settings {
            font_size: 900.0,
            ..Default::default()
        };
        let mut st = egui::Style::default();
        s.apply_style(&mut st);
        assert_eq!(st.text_styles[&TextStyle::Body].size, 32.0);
    }

    #[test]
    fn one_font_for_everything() {
        let s = Settings {
            font: FontChoice::hack(),
            ..Default::default()
        };
        let f = s.font_definitions(&LoadedFont::default());
        assert_eq!(f.families[&FontFamily::Proportional][0], HACK);
        assert_eq!(f.families[&FontFamily::Monospace][0], HACK);
        let f = Settings::default().font_definitions(&LoadedFont::default());
        assert_eq!(f.families[&FontFamily::Monospace][0], UBUNTU);
    }

    #[test]
    fn custom_background_visuals() {
        let s = Settings {
            theme: ThemeChoice::Custom,
            background: [0, 0, 0],
            ..Default::default()
        };
        let v = s.visuals(false);
        assert!(v.dark_mode);
        assert_eq!(v.panel_fill, Color32::BLACK);
        assert_eq!(v.override_text_color, Some(rgb([240, 240, 240])));
        // Fields stay visible on black.
        assert_ne!(v.text_edit_bg_color, Some(Color32::BLACK));
        let s = Settings {
            background: [240, 225, 196],
            ..s
        };
        assert!(!s.visuals(true).dark_mode);
    }

    #[test]
    fn settings_roundtrip_and_old_files() {
        let s = Settings {
            font: FontChoice::System {
                family: "DejaVu Sans".into(),
                path: "/usr/share/fonts/DejaVuSans.ttf".into(),
                index: 0,
            },
            ..Default::default()
        };
        // eframe stores values as RON.
        let txt = ron::to_string(&s).unwrap();
        let back: Settings = ron::from_str(&txt).unwrap();
        assert_eq!(back, s);
        // Saved by the first 0.1.2 test build: ui_font/mono_font/ui_scale.
        let old: Settings = ron::from_str(
            r#"(theme:Custom,background:(240,225,196),ui_font:Builtin("Hack"),mono_font:Builtin("Hack"),font_size:18.0,ui_scale:1.5)"#,
        )
        .unwrap();
        assert_eq!(old.theme, ThemeChoice::Custom);
        assert_eq!(old.font, FontChoice::hack());
        assert_eq!(old.font_size, 18.0);
        let old: Settings = ron::from_str("(theme: Dark)").unwrap();
        assert_eq!(old.font_size, DEFAULT_FONT_SIZE);
    }

    #[test]
    fn fc_list_parsing() {
        let out = "DejaVu Sans\tBold\t/f/DejaVuSans-Bold.ttf\t0\t0\n\
                   DejaVu Sans\tBook\t/f/DejaVuSans.ttf\t0\t0\n\
                   DejaVu Sans Mono\tBook\t/f/DejaVuSansMono.ttf\t0\t100\n\
                   Noto Sans CJK JP\tRegular\t/f/NotoSansCJK.ttc\t2\t0\n\
                   Bitmap\tRegular\t/f/misc.pcf.gz\t0\t0\n\
                   Inter\tItalic\t/f/Inter-Italic.ttf\t0\t0\n\
                   Inter\tMedium\t/f/Inter.ttf\t65536\t0\n\
                   broken line\n";
        let v = parse_fc_list(out);
        let names: Vec<_> = v.iter().map(|f| f.family.as_str()).collect();
        assert_eq!(
            names,
            [
                "DejaVu Sans",
                "DejaVu Sans Mono",
                "Inter",
                "Noto Sans CJK JP"
            ]
        );
        assert_eq!(v[0].path, PathBuf::from("/f/DejaVuSans.ttf"));
        assert!(v[1].monospace);
        assert_eq!(v[2].path, PathBuf::from("/f/Inter.ttf"));
        assert_eq!(v[2].index, 0);
        assert_eq!(v[3].index, 2);
    }
}
