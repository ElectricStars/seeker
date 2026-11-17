// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Eric Allen

//! Appearance and font settings: the Settings window, how the choices are
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
pub const MIN_FONT_SIZE: f32 = 10.0;
pub const MAX_FONT_SIZE: f32 = 32.0;
pub const MIN_SCALE: f32 = 0.75;
pub const MAX_SCALE: f32 = 2.5;
/// WCAG AA contrast for normal text.
pub const GOOD_CONTRAST: f32 = 4.5;

const UBUNTU: &str = "Ubuntu-Light";
const HACK: &str = "Hack";
const UI_FONT_KEY: &str = "seeker-ui-font";
const MONO_FONT_KEY: &str = "seeker-mono-font";

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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub theme: ThemeChoice,
    /// Background for `ThemeChoice::Custom` (sRGB).
    pub background: [u8; 3],
    /// Use `text_color` instead of the automatic black/white.
    pub custom_text: bool,
    pub text_color: [u8; 3],
    pub ui_font: FontChoice,
    pub mono_font: FontChoice,
    /// Body text size in points.
    pub font_size: f32,
    /// Overall zoom of the interface (fields, buttons, spacing and text).
    pub ui_scale: f32,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            theme: ThemeChoice::Light,
            background: [250, 246, 232],
            custom_text: false,
            text_color: [20, 20, 20],
            ui_font: FontChoice::ubuntu(),
            mono_font: FontChoice::hack(),
            font_size: DEFAULT_FONT_SIZE,
            ui_scale: 1.0,
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
        let s = self.font_size.clamp(MIN_FONT_SIZE, MAX_FONT_SIZE);
        let k = s / EGUI_BODY_SIZE;
        style.text_styles = BTreeMap::from([
            (TextStyle::Small, FontId::proportional(s * 0.75)),
            (TextStyle::Body, FontId::proportional(s)),
            (TextStyle::Button, FontId::proportional(s)),
            (TextStyle::Heading, FontId::proportional(s * 1.4)),
            (TextStyle::Monospace, FontId::monospace(s * 0.95)),
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
        sp.slider_width = d.slider_width * k;
        sp.indent = d.indent * k;
        sp.menu_margin = d.menu_margin;
    }

    pub fn font_definitions(&self, loaded: &LoadedFonts) -> FontDefinitions {
        let mut fonts = FontDefinitions::default();
        let prop = fonts.families.entry(FontFamily::Proportional).or_default();
        // Hack as a fallback so symbols like ▲▼ render.
        prop.insert(1.min(prop.len()), HACK.to_owned());
        let place = |fonts: &mut FontDefinitions,
                     fam: FontFamily,
                     choice: &FontChoice,
                     data: Option<&Arc<FontData>>,
                     key: &str| {
            let first = match (choice, data) {
                (FontChoice::System { .. }, Some(d)) => {
                    fonts.font_data.insert(key.to_owned(), d.clone());
                    key.to_owned()
                }
                (FontChoice::Builtin(k), _) if k == HACK || k == UBUNTU => k.clone(),
                _ => return,
            };
            let list = fonts.families.entry(fam).or_default();
            list.retain(|n| *n != first);
            list.insert(0, first);
        };
        place(
            &mut fonts,
            FontFamily::Proportional,
            &self.ui_font,
            loaded.ui.as_ref(),
            UI_FONT_KEY,
        );
        place(
            &mut fonts,
            FontFamily::Monospace,
            &self.mono_font,
            loaded.mono.as_ref(),
            MONO_FONT_KEY,
        );
        fonts
    }
}

// ------------------------------------------------------------ font files

/// Font files read from disk for the current choices.
#[derive(Default)]
pub struct LoadedFonts {
    ui_src: Option<FontChoice>,
    mono_src: Option<FontChoice>,
    ui: Option<Arc<FontData>>,
    mono: Option<Arc<FontData>>,
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

impl LoadedFonts {
    /// Loads files for changed choices. On failure the choice falls back to
    /// the built-in font and the error is returned.
    pub fn refresh(&mut self, s: &mut Settings) -> Vec<String> {
        let mut errs = Vec::new();
        if self.ui_src.as_ref() != Some(&s.ui_font) {
            match load_font(&s.ui_font) {
                Ok(d) => self.ui = d,
                Err(e) => {
                    errs.push(e);
                    s.ui_font = FontChoice::ubuntu();
                    self.ui = None;
                }
            }
            self.ui_src = Some(s.ui_font.clone());
        }
        if self.mono_src.as_ref() != Some(&s.mono_font) {
            match load_font(&s.mono_font) {
                Ok(d) => self.mono = d,
                Err(e) => {
                    errs.push(e);
                    s.mono_font = FontChoice::hack();
                    self.mono = None;
                }
            }
            self.mono_src = Some(s.mono_font.clone());
        }
        errs
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
pub enum Tab {
    #[default]
    Appearance,
    Font,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum FontTarget {
    #[default]
    Interface,
    Preview,
}

/// State of the Settings window that isn't saved.
#[derive(Default)]
pub struct SettingsWindow {
    pub open: bool,
    pub tab: Tab,
    target: FontTarget,
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

impl SettingsWindow {
    pub fn open_at(&mut self, tab: Tab) {
        self.open = true;
        self.tab = tab;
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

    pub fn show(&mut self, ctx: &egui::Context, s: &mut Settings) {
        if !self.open {
            return;
        }
        let mut open = true;
        let em = s.font_size;
        egui::Window::new("Settings")
            .open(&mut open)
            .collapsible(false)
            .resizable(true)
            .default_width(em * 34.0)
            .pivot(egui::Align2::CENTER_CENTER)
            .default_pos(ctx.content_rect().center())
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut self.tab, Tab::Appearance, "Appearance");
                    ui.selectable_value(&mut self.tab, Tab::Font, "Font");
                });
                ui.separator();
                match self.tab {
                    Tab::Appearance => self.appearance(ui, s),
                    Tab::Font => {
                        self.poll_fonts(ctx);
                        self.font(ui, s);
                    }
                }
            });
        self.open = open;
    }

    fn appearance(&mut self, ui: &mut egui::Ui, s: &mut Settings) {
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
                let (r, resp) = ui.allocate_exact_size(
                    egui::vec2(s.font_size * 1.8, s.font_size * 1.4),
                    egui::Sense::click(),
                );
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
            .inner_margin(s.font_size * 0.75)
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
            if ui.button(" A− ").on_hover_text("Smaller text").clicked() {
                s.font_size = (s.font_size - 1.0).max(MIN_FONT_SIZE);
            }
            ui.add(
                egui::Slider::new(&mut s.font_size, MIN_FONT_SIZE..=MAX_FONT_SIZE)
                    .step_by(1.0)
                    .suffix(" pt"),
            );
            if ui.button(" A+ ").on_hover_text("Larger text").clicked() {
                s.font_size = (s.font_size + 1.0).min(MAX_FONT_SIZE);
            }
        });
        ui.label(RichText::new("Interface scale").strong());
        ui.horizontal(|ui| {
            let mut pct = (s.ui_scale * 100.0).round();
            if ui
                .add(
                    egui::Slider::new(&mut pct, MIN_SCALE * 100.0..=MAX_SCALE * 100.0)
                        .step_by(5.0)
                        .suffix(" %"),
                )
                .changed()
            {
                s.ui_scale = pct / 100.0;
            }
            ui.weak("Enlarges fields, buttons and text. Ctrl + / Ctrl − also work.");
        });
        ui.add_space(6.0);

        ui.label(RichText::new("Font").strong());
        ui.horizontal(|ui| {
            ui.radio_value(&mut self.target, FontTarget::Interface, "Interface");
            ui.radio_value(&mut self.target, FontTarget::Preview, "Preview pane");
        });
        let current = match self.target {
            FontTarget::Interface => &mut s.ui_font,
            FontTarget::Preview => &mut s.mono_font,
        };
        ui.horizontal(|ui| {
            ui.label("Current:");
            ui.label(RichText::new(current.label()).strong());
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
                .max_height(row_h * 8.0)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    for b in [FontChoice::ubuntu(), FontChoice::hack()] {
                        let label = b.label();
                        if filter.is_empty() || label.to_lowercase().contains(&filter) {
                            let sel = *current == b;
                            if ui.selectable_label(sel, label).clicked() {
                                *current = b;
                            }
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
                            for f in list
                                .iter()
                                .filter(|f| filter.is_empty() || f.family.to_lowercase().contains(&filter))
                            {
                                let sel = matches!(current, FontChoice::System { family, .. } if *family == f.family);
                                let label = if f.monospace {
                                    format!("{}  (monospace)", f.family)
                                } else {
                                    f.family.clone()
                                };
                                if ui.selectable_label(sel, label).clicked() {
                                    *current = FontChoice::System {
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
        ui.add_space(6.0);
        ui.label("Sample:");
        ui.label("The quick brown fox jumps over the lazy dog. 0123456789");
        ui.label(RichText::new("fn main() { println!(\"Hello\"); }  // preview pane").monospace());
        ui.add_space(6.0);
        if ui.button("Reset font settings").clicked() {
            let d = Settings::default();
            s.ui_font = d.ui_font;
            s.mono_font = d.mono_font;
            s.font_size = d.font_size;
            s.ui_scale = d.ui_scale;
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
        assert!(st.spacing.interact_size.y > egui::style::Spacing::default().interact_size.y);
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
        let s = Settings {
            background: [240, 225, 196],
            ..s
        };
        assert!(!s.visuals(true).dark_mode);
    }

    #[test]
    fn settings_roundtrip_and_old_files() {
        let s = Settings {
            ui_font: FontChoice::System {
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
        // Settings saved by an older version (fields missing) load with defaults.
        let old: Settings = ron::from_str("(theme: Dark)").unwrap();
        assert_eq!(old.theme, ThemeChoice::Dark);
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
