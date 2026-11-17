// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Eric Allen

//! Command-line argument parsing and the `--cli` terminal mode.

use std::io::Write;
use std::process::ExitCode;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use crate::search::{self, Event, NameMode, Progress, SearchOptions};
use crate::{APP_ID, APP_NAME};

#[derive(Debug, Default, Clone)]
pub struct Args {
    pub cli: bool,
    pub help: bool,
    pub version: bool,
    /// GUI only: start the search immediately.
    pub auto: bool,
    pub folders: Vec<String>,
    pub name: Option<String>,
    pub name_regex: bool,
    pub text: Option<String>,
    pub regex: bool,
    pub case_sensitive: bool,
    pub word: bool,
    pub hidden: bool,
    pub skip_hidden: bool,
    pub gitignore: bool,
    pub no_recurse: bool,
    pub follow: bool,
    pub binary: bool,
    pub exclude: Option<String>,
    /// `--type`: (include files, include folders).
    pub types: Option<(bool, bool)>,
    pub context: Option<usize>,
    pub files_only: bool,
}

pub fn help() -> String {
    format!(
        "{APP_NAME} {ver} — fast file search

Usage:
  {APP_ID} [OPTIONS] [FOLDER...]          open the GUI (prefilled)
  {APP_ID} --cli [OPTIONS] [FOLDER...]    search from the terminal

Options:
  -n, --name PATTERN     file/folder-name wildcards, e.g. \"*.rs;*.toml;!*.lock\"
      --name-regex       treat --name as a regular expression
      --type f|d|both    list files (f), folders (d) or both (default: both;
                         text searches list files only)
  -t, --text TEXT        text to find inside files
  -e, --regex            treat --text as a regular expression
  -s, --case             case-sensitive text search
  -w, --word             whole words only
  -H, --hidden           include hidden files and folders (the default)
      --skip-hidden      skip hidden files and folders
  -g, --gitignore        respect .gitignore / .ignore files
      --no-recurse       do not search subfolders
  -L, --follow           follow symbolic links
  -b, --binary           also search binary files
  -x, --exclude LIST     terminal mode: folders not to enter, ';'-separated
  -C, --context N        context lines (GUI preview)
  -l, --files-only       CLI: print only file paths
      --auto             GUI: start searching immediately
      --cli              terminal mode (no window)
  -h, --help             show this help
  -V, --version          show version

CLI exit status: 0 = found, 1 = nothing found, 2 = error.
",
        ver = crate::VERSION
    )
}

impl Args {
    pub fn parse(it: impl Iterator<Item = String>) -> Result<Self, String> {
        let mut a = Args::default();
        let mut it = it.peekable();
        let mut only_positional = false;
        while let Some(arg) = it.next() {
            if only_positional || !arg.starts_with('-') || arg == "-" {
                a.folders.push(arg);
                continue;
            }
            let mut val = |name: &str| it.next().ok_or_else(|| format!("{name} needs a value"));
            match arg.as_str() {
                "--" => only_positional = true,
                "--cli" => a.cli = true,
                "-h" | "--help" => a.help = true,
                "-V" | "--version" => a.version = true,
                "--auto" => a.auto = true,
                "-n" | "--name" => a.name = Some(val(&arg)?),
                "--name-regex" => a.name_regex = true,
                "-t" | "--text" => a.text = Some(val(&arg)?),
                "-e" | "--regex" => a.regex = true,
                "-s" | "--case" => a.case_sensitive = true,
                "-w" | "--word" => a.word = true,
                "-H" | "--hidden" => a.hidden = true,
                "--skip-hidden" | "--no-hidden" => a.skip_hidden = true,
                "-g" | "--gitignore" => a.gitignore = true,
                "--no-recurse" => a.no_recurse = true,
                "-L" | "--follow" => a.follow = true,
                "-b" | "--binary" => a.binary = true,
                "-x" | "--exclude" => a.exclude = Some(val(&arg)?),
                "--type" => {
                    let v = val(&arg)?;
                    a.types = Some(parse_type(&v)?);
                }
                "-l" | "--files-only" => a.files_only = true,
                "-C" | "--context" => {
                    let v = val(&arg)?;
                    a.context = Some(v.parse().map_err(|_| format!("bad --context '{v}'"))?);
                }
                other => return Err(format!("unknown option '{other}'")),
            }
        }
        Ok(a)
    }

    pub fn to_options(&self) -> SearchOptions {
        let (include_files, include_folders) = self.types.unwrap_or((true, true));
        let roots = if self.folders.is_empty() {
            vec![std::env::current_dir().unwrap_or_else(|_| ".".into())]
        } else {
            self.folders
                .iter()
                .map(|f| search::expand_tilde(f))
                .collect()
        };
        let defaults = SearchOptions::default();
        SearchOptions {
            roots,
            name_pattern: self.name.clone().unwrap_or_default(),
            name_mode: if self.name_regex {
                NameMode::Regex
            } else {
                NameMode::Wildcard
            },
            content: self.text.clone().unwrap_or_default(),
            content_is_regex: self.regex,
            case_sensitive: self.case_sensitive,
            whole_word: self.word,
            include_hidden: self.hidden || !self.skip_hidden,
            respect_gitignore: self.gitignore,
            recurse: !self.no_recurse,
            include_files,
            include_folders,
            follow_symlinks: self.follow,
            skip_binary: !self.binary,
            exclude_dirs: match &self.exclude {
                Some(x) => x.split(';').map(|s| s.trim().to_string()).collect(),
                None => defaults.exclude_dirs.clone(),
            },
            context_lines: 0,
            ..defaults
        }
    }
}

/// `--type` value: `f`, `d` or `both` (long forms accepted).
fn parse_type(v: &str) -> Result<(bool, bool), String> {
    match v.to_ascii_lowercase().as_str() {
        "f" | "file" | "files" => Ok((true, false)),
        "d" | "dir" | "dirs" | "folder" | "folders" | "directory" | "directories" => {
            Ok((false, true))
        }
        "both" | "all" | "fd" | "df" => Ok((true, true)),
        _ => Err(format!("bad --type '{v}' (use f, d or both)")),
    }
}

pub fn run(args: &Args) -> ExitCode {
    let opts = args.to_options();
    let cancel = Arc::new(AtomicBool::new(false));
    let progress = Arc::new(Progress::default());
    let out = Mutex::new(std::io::BufWriter::new(std::io::stdout()));
    let files_only = args.files_only || opts.content.is_empty();
    let result = search::run(&opts, &cancel, &progress, |ev| match ev {
        Event::Hit(h) => {
            let mut out = out.lock().unwrap();
            let p = h.path.display();
            // Ignore broken pipes (e.g. `| head`).
            if files_only {
                let _ = writeln!(out, "{p}");
            } else {
                for l in h.lines.iter().filter(|l| l.is_hit) {
                    let _ = writeln!(out, "{p}:{}:{}", l.number, l.text);
                }
            }
        }
        Event::Error(e) => eprintln!("{APP_ID}: {e}"),
        Event::Done(_) => {}
    });
    let _ = out.lock().unwrap().flush();
    if let Ok(s) = &result
        && s.errors > 0
    {
        eprintln!("{APP_ID}: {} files or folders could not be read", s.errors);
    }
    match result {
        Ok(s) if s.files_matched > 0 => ExitCode::SUCCESS,
        Ok(_) => ExitCode::from(1),
        Err(e) => {
            eprintln!("{APP_ID}: {e}");
            ExitCode::from(2)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn p(v: &[&str]) -> Result<Args, String> {
        Args::parse(v.iter().map(|s| s.to_string()))
    }
    #[test]
    fn parses_flags() {
        let a = p(&[
            "--cli", "-n", "*.rs", "-t", "foo", "-e", "-w", "/tmp", "~/x",
        ])
        .unwrap();
        assert!(a.cli && a.regex && a.word);
        assert_eq!(a.name.as_deref(), Some("*.rs"));
        assert_eq!(a.folders, ["/tmp", "~/x"]);
        assert!(p(&["--bogus"]).is_err());
        assert!(p(&["--text"]).is_err());
        let o = p(&["-x", "a;b", "--binary", "/"]).unwrap().to_options();
        assert_eq!(o.exclude_dirs, ["a", "b"]);
        assert!(!o.skip_binary);
        // Hidden files are searched unless --skip-hidden is given.
        assert!(o.include_hidden);
        assert!(!p(&["--skip-hidden"]).unwrap().to_options().include_hidden);
        assert!(p(&[]).unwrap().to_options().exclude_dirs.is_empty());
        // --type: both by default.
        let o = p(&[]).unwrap().to_options();
        assert!(o.include_files && o.include_folders);
        let o = p(&["--type", "d"]).unwrap().to_options();
        assert!(!o.include_files && o.include_folders);
        let o = p(&["--type", "files"]).unwrap().to_options();
        assert!(o.include_files && !o.include_folders);
        assert!(p(&["--type", "both"]).unwrap().types == Some((true, true)));
        assert!(p(&["--type", "x"]).is_err());
        assert!(p(&["--type"]).is_err());
    }
}
