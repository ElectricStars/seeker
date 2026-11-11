// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Eric Allen

//! Search engine: walks one or more folders in parallel, filters files by
//! name / size / date, and optionally searches file contents.
//!
//! This module has no GUI dependencies so it can be unit-tested and reused
//! by the CLI mode.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime};

use crossbeam_channel::{Receiver, Sender};
use globset::{GlobBuilder, GlobSet, GlobSetBuilder};
use grep_regex::{RegexMatcher, RegexMatcherBuilder};
use grep_searcher::{
    BinaryDetection, Searcher, SearcherBuilder, Sink, SinkContext, SinkContextKind, SinkMatch,
};
use ignore::{WalkBuilder, WalkState};
use regex::{Regex, RegexBuilder};

/// Maximum number of lines (hits + context) kept in memory per file.
pub const MAX_STORED_LINES_PER_FILE: usize = 5_000;
/// Lines longer than this (in bytes) are truncated in the stored preview.
pub const MAX_LINE_LEN: usize = 2_000;

/// How the file-name pattern is interpreted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum NameMode {
    /// `*.rs;*.txt` style wildcards. A leading `!` excludes.
    #[default]
    Wildcard,
    /// A regular expression matched against the file name.
    Regex,
}

/// Everything needed to run one search.
#[derive(Debug, Clone)]
pub struct SearchOptions {
    pub roots: Vec<PathBuf>,
    /// Name pattern(s). Empty means "all files".
    pub name_pattern: String,
    pub name_mode: NameMode,
    /// Text to look for inside files. Empty means "name search only".
    pub content: String,
    pub content_is_regex: bool,
    pub case_sensitive: bool,
    pub whole_word: bool,
    pub recurse: bool,
    /// Folder names (wildcards allowed) that are never entered.
    pub exclude_dirs: Vec<String>,
    pub respect_gitignore: bool,
    pub include_hidden: bool,
    pub follow_symlinks: bool,
    pub skip_binary: bool,
    pub min_size: Option<u64>,
    pub max_size: Option<u64>,
    /// Inclusive lower bound on modification time.
    pub modified_after: Option<SystemTime>,
    /// Exclusive upper bound on modification time.
    pub modified_before: Option<SystemTime>,
    /// Number of context lines stored before/after each hit.
    pub context_lines: usize,
    /// 0 = pick automatically.
    pub threads: usize,
}

impl Default for SearchOptions {
    fn default() -> Self {
        Self {
            roots: Vec::new(),
            name_pattern: String::new(),
            name_mode: NameMode::Wildcard,
            content: String::new(),
            content_is_regex: false,
            case_sensitive: false,
            whole_word: false,
            recurse: true,
            exclude_dirs: default_excludes(),
            respect_gitignore: false,
            include_hidden: false,
            follow_symlinks: false,
            skip_binary: true,
            min_size: None,
            max_size: None,
            modified_after: None,
            modified_before: None,
            context_lines: 2,
            threads: 0,
        }
    }
}

pub fn default_excludes() -> Vec<String> {
    [".git", "node_modules", "target", "__pycache__"]
        .iter()
        .map(|s| s.to_string())
        .collect()
}

/// One line shown in the preview: either a hit or a context line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    pub number: u64,
    pub text: String,
    /// Byte ranges of matches inside `text` (empty for context lines).
    pub ranges: Vec<(usize, usize)>,
    pub is_hit: bool,
}

/// A file that matched all criteria.
#[derive(Debug, Clone)]
pub struct FileHit {
    pub path: PathBuf,
    pub size: u64,
    pub modified: Option<SystemTime>,
    /// Number of individual matches in the file (0 for name-only searches).
    pub hits: u64,
    /// Number of lines containing at least one match.
    pub hit_lines: u64,
    /// Hit + context lines, in file order. A gap is marked by a line with
    /// `number == 0` (a "context break").
    pub lines: Vec<Line>,
    /// True if `lines` was truncated.
    pub truncated: bool,
}

/// Messages sent from the background search to the caller.
#[derive(Debug)]
pub enum Event {
    Hit(Box<FileHit>),
    Error(String),
    Done(Stats),
}

#[derive(Debug, Clone, Default)]
pub struct Stats {
    pub files_scanned: u64,
    pub files_matched: u64,
    pub total_hits: u64,
    pub errors: u64,
    pub elapsed: Duration,
    pub cancelled: bool,
}

/// Shared live counters, readable while a search is running.
#[derive(Debug, Default)]
pub struct Progress {
    pub files_scanned: AtomicU64,
    pub files_matched: AtomicU64,
    pub total_hits: AtomicU64,
    pub errors: AtomicU64,
}

/// Handle to a running background search.
pub struct SearchHandle {
    pub rx: Receiver<Event>,
    pub progress: Arc<Progress>,
    cancel: Arc<AtomicBool>,
    started: Instant,
    thread: Option<JoinHandle<()>>,
}

impl SearchHandle {
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
    pub fn elapsed(&self) -> Duration {
        self.started.elapsed()
    }
}

impl Drop for SearchHandle {
    fn drop(&mut self) {
        self.cancel();
        // Don't join: the walker notices the flag quickly and exits on its own.
        let _ = self.thread.take();
    }
}

/// Compiled name filter.
#[derive(Debug, Clone)]
pub enum NameFilter {
    All,
    Globs {
        include: Option<GlobSet>,
        exclude: Option<GlobSet>,
    },
    Regex(Regex),
}

impl NameFilter {
    pub fn new(pattern: &str, mode: NameMode) -> Result<Self, String> {
        let pattern = pattern.trim();
        if pattern.is_empty() {
            return Ok(NameFilter::All);
        }
        match mode {
            NameMode::Regex => RegexBuilder::new(pattern)
                .case_insensitive(true)
                .build()
                .map(NameFilter::Regex)
                .map_err(|e| format!("Invalid file-name regex: {e}")),
            NameMode::Wildcard => {
                let mut inc = GlobSetBuilder::new();
                let mut exc = GlobSetBuilder::new();
                let (mut n_inc, mut n_exc) = (0, 0);
                for part in pattern.split([';', ',']).map(str::trim) {
                    if part.is_empty() {
                        continue;
                    }
                    let (neg, pat) = match part.strip_prefix('!').or(part.strip_prefix('-')) {
                        Some(rest) => (true, rest.trim()),
                        None => (false, part),
                    };
                    // A bare word like "main" means "name contains main".
                    let pat = if pat.contains(['*', '?', '[', '{']) {
                        pat.to_string()
                    } else {
                        format!("*{pat}*")
                    };
                    let glob = GlobBuilder::new(&pat)
                        .case_insensitive(true)
                        .literal_separator(false)
                        .build()
                        .map_err(|e| format!("Invalid wildcard '{pat}': {e}"))?;
                    if neg {
                        exc.add(glob);
                        n_exc += 1;
                    } else {
                        inc.add(glob);
                        n_inc += 1;
                    }
                }
                let build = |b: GlobSetBuilder, n: usize| -> Result<Option<GlobSet>, String> {
                    if n == 0 {
                        Ok(None)
                    } else {
                        b.build().map(Some).map_err(|e| e.to_string())
                    }
                };
                Ok(NameFilter::Globs {
                    include: build(inc, n_inc)?,
                    exclude: build(exc, n_exc)?,
                })
            }
        }
    }

    /// `name` is the file name; `path` the full path (used when a wildcard contains `/`).
    pub fn matches(&self, name: &str, path: &Path) -> bool {
        match self {
            NameFilter::All => true,
            NameFilter::Regex(re) => re.is_match(name),
            NameFilter::Globs { include, exclude } => {
                let hit = |set: &GlobSet| set.is_match(name) || set.is_match(path);
                if let Some(ex) = exclude
                    && hit(ex)
                {
                    return false;
                }
                match include {
                    Some(inc) => hit(inc),
                    None => true,
                }
            }
        }
    }
}

/// Builds the regex source for the content search.
fn content_pattern(opts: &SearchOptions) -> String {
    let core = if opts.content_is_regex {
        opts.content.clone()
    } else {
        regex::escape(&opts.content)
    };
    if opts.whole_word {
        format!(r"\b(?:{core})\b")
    } else {
        core
    }
}

/// Content matchers: one for searching (bytes) and one for highlighting (str).
#[derive(Clone)]
pub struct ContentMatcher {
    pub grep: RegexMatcher,
    pub highlight: Regex,
}

impl ContentMatcher {
    pub fn new(opts: &SearchOptions) -> Result<Option<Self>, String> {
        if opts.content.is_empty() {
            return Ok(None);
        }
        let pat = content_pattern(opts);
        let grep = RegexMatcherBuilder::new()
            .case_insensitive(!opts.case_sensitive)
            .line_terminator(Some(b'\n'))
            .build(&pat)
            .map_err(|e| format!("Invalid search text: {e}"))?;
        let highlight = RegexBuilder::new(&pat)
            .case_insensitive(!opts.case_sensitive)
            .build()
            .map_err(|e| format!("Invalid search text: {e}"))?;
        Ok(Some(Self { grep, highlight }))
    }

    /// Byte ranges of every match inside `text`.
    pub fn ranges(&self, text: &str) -> Vec<(usize, usize)> {
        self.highlight
            .find_iter(text)
            .filter(|m| !m.is_empty())
            .map(|m| (m.start(), m.end()))
            .collect()
    }
}

/// Strips the line terminator, decodes lossily and truncates very long lines.
fn line_text(bytes: &[u8]) -> String {
    let mut b = bytes;
    while let Some((&last, rest)) = b.split_last() {
        if last == b'\n' || last == b'\r' {
            b = rest;
        } else {
            break;
        }
    }
    let mut s = String::from_utf8_lossy(b).into_owned();
    if s.len() > MAX_LINE_LEN {
        let mut cut = MAX_LINE_LEN;
        while !s.is_char_boundary(cut) {
            cut -= 1;
        }
        s.truncate(cut);
        s.push('…');
    }
    s
}

struct CollectSink<'a> {
    matcher: &'a ContentMatcher,
    cancel: &'a AtomicBool,
    hits: u64,
    hit_lines: u64,
    lines: Vec<Line>,
    truncated: bool,
    binary: bool,
    skip_binary: bool,
}

impl CollectSink<'_> {
    fn push(&mut self, line: Line) {
        if self.lines.len() >= MAX_STORED_LINES_PER_FILE {
            self.truncated = true;
        } else {
            self.lines.push(line);
        }
    }
}

impl Sink for CollectSink<'_> {
    type Error = io::Error;

    fn matched(&mut self, _s: &Searcher, m: &SinkMatch<'_>) -> Result<bool, io::Error> {
        let text = line_text(m.bytes());
        let ranges = self.matcher.ranges(&text);
        // Whole-line count is at least one even if the (truncated) text lost the match.
        self.hits += (ranges.len() as u64).max(1);
        self.hit_lines += 1;
        self.push(Line {
            number: m.line_number().unwrap_or(0),
            text,
            ranges,
            is_hit: true,
        });
        Ok(!self.cancel.load(Ordering::Relaxed))
    }

    fn context(&mut self, _s: &Searcher, c: &SinkContext<'_>) -> Result<bool, io::Error> {
        if matches!(c.kind(), SinkContextKind::Other) {
            return Ok(true);
        }
        let text = line_text(c.bytes());
        self.push(Line {
            number: c.line_number().unwrap_or(0),
            text,
            ranges: Vec::new(),
            is_hit: false,
        });
        Ok(!self.cancel.load(Ordering::Relaxed))
    }

    fn context_break(&mut self, _s: &Searcher) -> Result<bool, io::Error> {
        if !self.lines.is_empty() {
            self.push(Line {
                number: 0,
                text: String::new(),
                ranges: Vec::new(),
                is_hit: false,
            });
        }
        Ok(true)
    }

    fn binary_data(&mut self, _s: &Searcher, _offset: u64) -> Result<bool, io::Error> {
        self.binary = true;
        Ok(!self.skip_binary)
    }
}

/// Matches found in one file's contents.
#[derive(Debug, Clone)]
pub struct ContentResult {
    pub hits: u64,
    pub hit_lines: u64,
    pub lines: Vec<Line>,
    pub truncated: bool,
}

/// Searches one file's contents. Returns `None` if nothing matched (or it was a
/// skipped binary file).
pub fn search_file(
    searcher: &mut Searcher,
    matcher: &ContentMatcher,
    path: &Path,
    skip_binary: bool,
    cancel: &AtomicBool,
) -> io::Result<Option<ContentResult>> {
    let mut sink = CollectSink {
        matcher,
        cancel,
        hits: 0,
        hit_lines: 0,
        lines: Vec::new(),
        truncated: false,
        binary: false,
        skip_binary,
    };
    searcher.search_path(&matcher.grep, path, &mut sink)?;
    if (sink.binary && skip_binary) || sink.hit_lines == 0 {
        return Ok(None);
    }
    Ok(Some(ContentResult {
        hits: sink.hits,
        hit_lines: sink.hit_lines,
        lines: sink.lines,
        truncated: sink.truncated,
    }))
}

fn build_searcher(opts: &SearchOptions) -> Searcher {
    SearcherBuilder::new()
        .line_number(true)
        .before_context(opts.context_lines)
        .after_context(opts.context_lines)
        .binary_detection(if opts.skip_binary {
            BinaryDetection::quit(b'\x00')
        } else {
            BinaryDetection::convert(b'\x00')
        })
        .bom_sniffing(true)
        .build()
}

fn build_exclude_set(opts: &SearchOptions) -> Result<GlobSet, String> {
    let mut b = GlobSetBuilder::new();
    for pat in opts
        .exclude_dirs
        .iter()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
    {
        b.add(
            GlobBuilder::new(pat)
                .literal_separator(true)
                .build()
                .map_err(|e| format!("Invalid exclude pattern '{pat}': {e}"))?,
        );
    }
    b.build().map_err(|e| e.to_string())
}

fn is_virtual_fs(p: &Path) -> bool {
    [
        Path::new("/proc"),
        Path::new("/sys"),
        Path::new("/dev"),
        Path::new("/run"),
    ]
    .contains(&p)
}

/// Validates options and compiles everything needed, so errors are reported
/// before any thread starts.
fn prepare(
    opts: &SearchOptions,
) -> Result<(NameFilter, Option<ContentMatcher>, GlobSet, WalkBuilder), String> {
    if opts.roots.is_empty() {
        return Err("Choose at least one folder to look in.".into());
    }
    for r in &opts.roots {
        if !r.exists() {
            return Err(format!("Folder does not exist: {}", r.display()));
        }
    }
    let name = NameFilter::new(&opts.name_pattern, opts.name_mode)?;
    let content = ContentMatcher::new(opts)?;
    let excludes = build_exclude_set(opts)?;

    let mut wb = WalkBuilder::new(&opts.roots[0]);
    for r in &opts.roots[1..] {
        wb.add(r);
    }
    wb.hidden(!opts.include_hidden)
        .follow_links(opts.follow_symlinks)
        .git_ignore(opts.respect_gitignore)
        .git_global(opts.respect_gitignore)
        .git_exclude(opts.respect_gitignore)
        .ignore(opts.respect_gitignore)
        .parents(opts.respect_gitignore)
        .require_git(false)
        .max_depth(if opts.recurse { None } else { Some(1) })
        .threads(opts.threads);
    Ok((name, content, excludes, wb))
}

/// Runs a search on the current thread, calling `on_event` for every hit and
/// error (from several worker threads). Returns final statistics.
pub fn run(
    opts: &SearchOptions,
    cancel: &Arc<AtomicBool>,
    progress: &Arc<Progress>,
    on_event: impl Fn(Event) + Sync,
) -> Result<Stats, String> {
    let started = Instant::now();
    let (name_filter, content, excludes, mut wb) = prepare(opts)?;
    let excludes = Arc::new(excludes);
    {
        let excludes = excludes.clone();
        wb.filter_entry(move |e| {
            // Never filter a root, only folders found below it.
            if e.depth() == 0 {
                return true;
            }
            let is_dir = e.file_type().is_some_and(|t| t.is_dir());
            if !is_dir {
                return true;
            }
            // Kernel pseudo-filesystems are huge, endless or blocking; skip them
            // unless the user explicitly starts the search inside one.
            if e.depth() == 1 && is_virtual_fs(e.path()) {
                return false;
            }
            !excludes.is_match(e.file_name())
        });
    }
    let on_event = &on_event;
    let name_filter = &name_filter;
    let content = content.as_ref();

    wb.build_parallel().run(|| {
        let mut searcher = build_searcher(opts);
        let cancel = cancel.clone();
        let progress = progress.clone();
        Box::new(move |entry| {
            if cancel.load(Ordering::Relaxed) {
                return WalkState::Quit;
            }
            let entry = match entry {
                Ok(e) => e,
                Err(err) => {
                    progress.errors.fetch_add(1, Ordering::Relaxed);
                    on_event(Event::Error(err.to_string()));
                    return WalkState::Continue;
                }
            };
            let Some(ft) = entry.file_type() else {
                return WalkState::Continue;
            };
            // Symlinks to files are followed only if requested; dirs are handled by the walker.
            let is_file =
                ft.is_file() || (ft.is_symlink() && opts.follow_symlinks && entry.path().is_file());
            if !is_file {
                return WalkState::Continue;
            }
            let path = entry.path();
            let fname = entry.file_name().to_string_lossy();
            if !name_filter.matches(&fname, path) {
                return WalkState::Continue;
            }
            let meta = match entry.metadata() {
                Ok(m) => m,
                Err(err) => {
                    progress.errors.fetch_add(1, Ordering::Relaxed);
                    on_event(Event::Error(format!("{}: {err}", path.display())));
                    return WalkState::Continue;
                }
            };
            let size = meta.len();
            if opts.min_size.is_some_and(|m| size < m) || opts.max_size.is_some_and(|m| size > m) {
                return WalkState::Continue;
            }
            let modified = meta.modified().ok();
            if let Some(after) = opts.modified_after
                && modified.is_none_or(|m| m < after)
            {
                return WalkState::Continue;
            }
            if let Some(before) = opts.modified_before
                && modified.is_none_or(|m| m >= before)
            {
                return WalkState::Continue;
            }
            progress.files_scanned.fetch_add(1, Ordering::Relaxed);

            let mut hit = FileHit {
                path: path.to_path_buf(),
                size,
                modified,
                hits: 0,
                hit_lines: 0,
                lines: Vec::new(),
                truncated: false,
            };
            if let Some(m) = content {
                match search_file(&mut searcher, m, path, opts.skip_binary, &cancel) {
                    Ok(Some(r)) => {
                        hit.hits = r.hits;
                        hit.hit_lines = r.hit_lines;
                        hit.lines = r.lines;
                        hit.truncated = r.truncated;
                    }
                    Ok(None) => return WalkState::Continue,
                    Err(err) => {
                        progress.errors.fetch_add(1, Ordering::Relaxed);
                        on_event(Event::Error(format!("{}: {err}", path.display())));
                        return WalkState::Continue;
                    }
                }
            }
            progress.files_matched.fetch_add(1, Ordering::Relaxed);
            progress.total_hits.fetch_add(hit.hits, Ordering::Relaxed);
            on_event(Event::Hit(Box::new(hit)));
            WalkState::Continue
        })
    });

    Ok(Stats {
        files_scanned: progress.files_scanned.load(Ordering::Relaxed),
        files_matched: progress.files_matched.load(Ordering::Relaxed),
        total_hits: progress.total_hits.load(Ordering::Relaxed),
        errors: progress.errors.load(Ordering::Relaxed),
        elapsed: started.elapsed(),
        cancelled: cancel.load(Ordering::Relaxed),
    })
}

/// Starts a search on a background thread. Options are validated first, so
/// an invalid regex is reported immediately as `Err`.
///
/// `wake` is called after each batch of events (e.g. to request a GUI repaint).
pub fn start(
    opts: SearchOptions,
    wake: impl Fn() + Send + Sync + 'static,
) -> Result<SearchHandle, String> {
    prepare(&opts)?; // fail fast on bad input
    let (tx, rx): (Sender<Event>, Receiver<Event>) = crossbeam_channel::unbounded();
    let cancel = Arc::new(AtomicBool::new(false));
    let progress = Arc::new(Progress::default());
    let (c2, p2) = (cancel.clone(), progress.clone());
    let wake = Arc::new(wake);
    let thread = std::thread::Builder::new()
        .name("seeker-search".into())
        .spawn(move || {
            let tx2 = tx.clone();
            let w2 = wake.clone();
            let result = run(&opts, &c2, &p2, move |ev| {
                let _ = tx2.send(ev);
                w2();
            });
            let stats = match result {
                Ok(s) => s,
                Err(e) => {
                    let _ = tx.send(Event::Error(e));
                    Stats::default()
                }
            };
            let _ = tx.send(Event::Done(stats));
            wake();
        })
        .map_err(|e| e.to_string())?;
    Ok(SearchHandle {
        rx,
        progress,
        cancel,
        started: Instant::now(),
        thread: Some(thread),
    })
}

/// Parses sizes like `100`, `10K`, `1.5MB`, `2 GiB` (binary units).
pub fn parse_size(s: &str) -> Result<Option<u64>, String> {
    let s = s.trim();
    if s.is_empty() {
        return Ok(None);
    }
    let lower = s.to_ascii_lowercase();
    let split = lower
        .find(|c: char| !(c.is_ascii_digit() || c == '.'))
        .unwrap_or(lower.len());
    let (num, unit) = lower.split_at(split);
    let num: f64 = num
        .parse()
        .map_err(|_| format!("Invalid size '{s}' (try 10K, 5M, 1G)"))?;
    let mult: u64 = match unit.trim() {
        "" | "b" => 1,
        "k" | "kb" | "kib" => 1 << 10,
        "m" | "mb" | "mib" => 1 << 20,
        "g" | "gb" | "gib" => 1 << 30,
        "t" | "tb" | "tib" => 1 << 40,
        _ => return Err(format!("Invalid size unit in '{s}' (use K, M, G)")),
    };
    Ok(Some((num * mult as f64).round() as u64))
}

/// Human-readable size, e.g. `1.2 MB`.
pub fn format_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = bytes as f64;
    let mut i = 0;
    while v >= 1024.0 && i < UNITS.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{bytes} B")
    } else {
        format!("{v:.1} {}", UNITS[i])
    }
}

/// Parses `YYYY-MM-DD` as local midnight. `end_of_day` returns the next
/// midnight (exclusive bound) instead.
pub fn parse_date(s: &str, end_of_day: bool) -> Result<Option<SystemTime>, String> {
    use chrono::{Local, NaiveDate, TimeZone};
    let s = s.trim();
    if s.is_empty() {
        return Ok(None);
    }
    let d = NaiveDate::parse_from_str(s, "%Y-%m-%d")
        .map_err(|_| format!("Invalid date '{s}' (use YYYY-MM-DD)"))?;
    let d = if end_of_day {
        d.succ_opt().unwrap_or(d)
    } else {
        d
    };
    let dt = d.and_hms_opt(0, 0, 0).expect("midnight is valid");
    let local = Local
        .from_local_datetime(&dt)
        .earliest()
        .ok_or_else(|| format!("Invalid local date '{s}'"))?;
    Ok(Some(local.into()))
}

/// Splits a `;`-separated list of folders, expanding a leading `~`.
pub fn parse_roots(s: &str) -> Vec<PathBuf> {
    s.split(';')
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(expand_tilde)
        .collect()
}

pub fn expand_tilde(p: &str) -> PathBuf {
    if (p == "~" || p.starts_with("~/"))
        && let Some(home) = std::env::var_os("HOME")
    {
        let mut h = PathBuf::from(home);
        if p.len() > 2 {
            h.push(&p[2..]);
        }
        return h;
    }
    PathBuf::from(p)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn fixture() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        fs::create_dir_all(p.join("src/nested")).unwrap();
        fs::create_dir_all(p.join("node_modules/pkg")).unwrap();
        fs::create_dir_all(p.join(".hidden")).unwrap();
        fs::create_dir_all(p.join("ignored_dir")).unwrap();
        fs::write(
            p.join("src/main.rs"),
            "fn main() {\n    println!(\"Hello\");\n}\n",
        )
        .unwrap();
        fs::write(
            p.join("src/nested/lib.rs"),
            "// hello world\npub fn helper() {}\n// HELLO again, hello!\n",
        )
        .unwrap();
        fs::write(p.join("notes.txt"), "line one\nhelloworld\nline three\n").unwrap();
        fs::write(p.join("node_modules/pkg/index.js"), "hello from js\n").unwrap();
        fs::write(p.join(".hidden/secret.txt"), "hello hidden\n").unwrap();
        fs::write(p.join("ignored_dir/x.txt"), "hello ignored\n").unwrap();
        fs::write(p.join(".gitignore"), "ignored_dir/\n").unwrap();
        fs::write(p.join("blob.bin"), b"hello\x00\x01\x02binary").unwrap();
        // Latin-1 (invalid UTF-8) content.
        fs::write(p.join("latin1.txt"), b"caf\xe9 hello\n").unwrap();
        // UTF-16LE with BOM.
        let mut u16 = vec![0xFF, 0xFE];
        for c in "utf16 hello\n".encode_utf16() {
            u16.extend_from_slice(&c.to_le_bytes());
        }
        fs::write(p.join("wide.txt"), u16).unwrap();
        dir
    }

    fn search(opts: &SearchOptions) -> (Vec<FileHit>, Stats) {
        let cancel = Arc::new(AtomicBool::new(false));
        let progress = Arc::new(Progress::default());
        let hits = std::sync::Mutex::new(Vec::new());
        let stats = run(opts, &cancel, &progress, |e| {
            if let Event::Hit(h) = e {
                hits.lock().unwrap().push(*h);
            }
        })
        .unwrap();
        let mut v = hits.into_inner().unwrap();
        v.sort_by(|a, b| a.path.cmp(&b.path));
        (v, stats)
    }

    fn names(hits: &[FileHit], root: &Path) -> Vec<String> {
        hits.iter()
            .map(|h| {
                h.path
                    .strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect()
    }

    fn opts(root: &Path) -> SearchOptions {
        SearchOptions {
            roots: vec![root.to_path_buf()],
            ..Default::default()
        }
    }

    #[test]
    fn name_wildcards() {
        let d = fixture();
        let mut o = opts(d.path());
        o.name_pattern = "*.rs".into();
        let (h, _) = search(&o);
        assert_eq!(names(&h, d.path()), ["src/main.rs", "src/nested/lib.rs"]);

        o.name_pattern = "*.RS; *.txt; !lib*".into();
        let (h, _) = search(&o);
        assert_eq!(
            names(&h, d.path()),
            [
                "ignored_dir/x.txt",
                "latin1.txt",
                "notes.txt",
                "src/main.rs",
                "wide.txt"
            ]
        );
    }

    #[test]
    fn name_regex_and_bare_word() {
        let d = fixture();
        let mut o = opts(d.path());
        o.name_mode = NameMode::Regex;
        o.name_pattern = r"^(main|lib)\.rs$".into();
        assert_eq!(search(&o).0.len(), 2);
        o.name_mode = NameMode::Wildcard;
        o.name_pattern = "note".into(); // bare word = contains
        assert_eq!(names(&search(&o).0, d.path()), ["notes.txt"]);
    }

    #[test]
    fn content_case_insensitive_counts_hits() {
        let d = fixture();
        let mut o = opts(d.path());
        o.name_pattern = "*.rs".into();
        o.content = "hello".into();
        let (h, stats) = search(&o);
        assert_eq!(h.len(), 2);
        let lib = h.iter().find(|h| h.path.ends_with("lib.rs")).unwrap();
        assert_eq!(lib.hits, 3);
        assert_eq!(lib.hit_lines, 2);
        let first = lib.lines.iter().find(|l| l.is_hit).unwrap();
        assert_eq!(first.number, 1);
        assert_eq!(first.ranges, vec![(3, 8)]);
        assert_eq!(stats.files_matched, 2);
        assert_eq!(stats.total_hits, 4);
    }

    #[test]
    fn content_case_sensitive_and_whole_word() {
        let d = fixture();
        let mut o = opts(d.path());
        o.content = "hello".into();
        o.case_sensitive = true;
        o.whole_word = true;
        let n = names(&search(&o).0, d.path());
        // notes.txt has "helloworld" (not whole word); main.rs has "Hello".
        assert!(!n.contains(&"notes.txt".to_string()));
        assert!(!n.contains(&"src/main.rs".to_string()));
        assert!(n.contains(&"src/nested/lib.rs".to_string()));
    }

    #[test]
    fn regex_content() {
        let d = fixture();
        let mut o = opts(d.path());
        o.content = r"fn\s+\w+\(".into();
        o.content_is_regex = true;
        assert_eq!(search(&o).0.len(), 2);
        o.content = "(unclosed".into();
        assert!(start(o, || {}).is_err());
    }

    #[test]
    fn excludes_hidden_and_gitignore() {
        let d = fixture();
        let mut o = opts(d.path());
        o.content = "hello".into();
        let n = names(&search(&o).0, d.path());
        assert!(!n.iter().any(|p| p.starts_with("node_modules")));
        assert!(!n.iter().any(|p| p.starts_with(".hidden")));
        assert!(n.contains(&"ignored_dir/x.txt".to_string()));

        o.include_hidden = true;
        o.exclude_dirs.clear();
        o.respect_gitignore = true;
        let n = names(&search(&o).0, d.path());
        assert!(n.contains(&"node_modules/pkg/index.js".to_string()));
        assert!(n.contains(&".hidden/secret.txt".to_string()));
        assert!(!n.contains(&"ignored_dir/x.txt".to_string()));
    }

    #[test]
    fn binary_and_encodings() {
        let d = fixture();
        let mut o = opts(d.path());
        o.content = "hello".into();
        let n = names(&search(&o).0, d.path());
        assert!(!n.contains(&"blob.bin".to_string()));
        assert!(n.contains(&"latin1.txt".to_string()));
        assert!(
            n.contains(&"wide.txt".to_string()),
            "UTF-16 BOM file found: {n:?}"
        );
        o.skip_binary = false;
        let n = names(&search(&o).0, d.path());
        assert!(n.contains(&"blob.bin".to_string()));
    }

    #[test]
    fn no_recursion_and_context() {
        let d = fixture();
        let mut o = opts(d.path());
        o.recurse = false;
        o.content = "helloworld".into();
        o.context_lines = 1;
        let (h, _) = search(&o);
        assert_eq!(names(&h, d.path()), ["notes.txt"]);
        let nums: Vec<u64> = h[0].lines.iter().map(|l| l.number).collect();
        assert_eq!(nums, [1, 2, 3]);
        o.content = "nested".into();
        assert!(search(&o).0.is_empty());
    }

    #[test]
    fn size_and_date_filters() {
        let d = fixture();
        let mut o = opts(d.path());
        // main.rs is 42 bytes, notes.txt is 31 bytes.
        o.min_size = Some(35);
        let n = names(&search(&o).0, d.path());
        assert!(n.contains(&"src/main.rs".to_string()));
        assert!(!n.contains(&"notes.txt".to_string()));
        o.min_size = None;
        o.max_size = Some(5);
        assert!(search(&o).0.is_empty());
        o.max_size = None;
        o.modified_after = parse_date("2999-01-01", false).unwrap();
        assert!(search(&o).0.is_empty());
        o.modified_after = parse_date("2000-01-01", false).unwrap();
        o.modified_before = parse_date("2999-01-01", true).unwrap();
        assert!(!search(&o).0.is_empty());
    }

    #[test]
    fn multiple_roots_and_errors() {
        let d = fixture();
        let mut o = opts(&d.path().join("src"));
        o.roots.push(d.path().join("node_modules"));
        o.exclude_dirs.clear();
        o.content = "hello".into();
        assert_eq!(search(&o).0.len(), 3);
        o.roots = vec![d.path().join("does-not-exist")];
        assert!(start(o, || {}).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn permission_denied_is_reported_not_fatal() {
        use std::os::unix::fs::PermissionsExt;
        let d = fixture();
        let locked = d.path().join("locked");
        fs::create_dir(&locked).unwrap();
        fs::write(locked.join("a.txt"), "hello").unwrap();
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
        let mut o = opts(d.path());
        o.content = "hello".into();
        let (h, stats) = search(&o);
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(!h.is_empty());
        // Running as root bypasses permissions, so only check when not root.
        if !nix_is_root() {
            assert!(stats.errors >= 1);
        }
    }

    #[cfg(unix)]
    fn nix_is_root() -> bool {
        std::env::var("USER").map(|u| u == "root").unwrap_or(false) || fs::read_dir("/root").is_ok()
    }

    #[test]
    fn cancel_stops_quickly() {
        let d = fixture();
        let o = opts(d.path());
        let h = start(o, || {}).unwrap();
        h.cancel();
        let mut done = false;
        for ev in h.rx.iter() {
            if let Event::Done(_) = ev {
                done = true;
                break;
            }
        }
        assert!(done);
    }

    #[test]
    fn sizes_and_dates_parse() {
        assert_eq!(parse_size("").unwrap(), None);
        assert_eq!(parse_size("100").unwrap(), Some(100));
        assert_eq!(parse_size("10K").unwrap(), Some(10240));
        assert_eq!(parse_size("1.5 MB").unwrap(), Some(1572864));
        assert_eq!(parse_size("2gib").unwrap(), Some(2 << 30));
        assert!(parse_size("ten").is_err());
        assert!(parse_size("5X").is_err());
        assert_eq!(format_size(512), "512 B");
        assert_eq!(format_size(1536), "1.5 KB");
        assert!(parse_date("2024-02-30", false).is_err());
        let a = parse_date("2024-03-01", false).unwrap().unwrap();
        let b = parse_date("2024-03-01", true).unwrap().unwrap();
        assert_eq!(b.duration_since(a).unwrap().as_secs(), 86400);
    }

    #[test]
    fn roots_parse() {
        let r = parse_roots(" /a ; /b/c;; ");
        assert_eq!(r, vec![PathBuf::from("/a"), PathBuf::from("/b/c")]);
    }

    #[test]
    fn long_lines_truncated_and_crlf_stripped() {
        let t = line_text(b"abc\r\n");
        assert_eq!(t, "abc");
        let long = vec![b'x'; MAX_LINE_LEN + 50];
        assert!(line_text(&long).chars().count() <= MAX_LINE_LEN + 1);
    }
}
