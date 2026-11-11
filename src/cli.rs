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
    pub gitignore: bool,
    pub no_recurse: bool,
    pub follow: bool,
    pub binary: bool,
    pub exclude: Option<String>,
    pub context: Option<usize>,
    pub files_only: bool,
}

pub fn help() -> String {
    format!(
        "{APP_NAME} {ver} — fast file search\n\n\
Usage:\n  {APP_ID} [OPTIONS] [FOLDER...]          open the GUI (prefilled)\n  {APP_ID} --cli [OPTIONS] [FOLDER...]    search from the terminal\n\n\
Options:\n\
  -n, --name PATTERN     file-name wildcards, e.g. \"*.rs;*.toml;!*.lock\"\n\
      --name-regex       treat --name as a regular expression\n\
  -t, --text TEXT        text to find inside files\n\
  -e, --regex            treat --text as a regular expression\n\
  -s, --case             case-sensitive text search\n\
  -w, --word             whole words only\n\
  -H, --hidden           include hidden files and folders\n\
  -g, --gitignore        respect .gitignore / .ignore files\n\
      --no-recurse       do not search subfolders\n\
  -L, --follow           follow symbolic links\n\
  -b, --binary           also search binary files\n\
  -x, --exclude LIST     folders to skip, ';'-separated (default: .git;node_modules;target;__pycache__)\n\
  -C, --context N        context lines (GUI preview)\n\
  -l, --files-only       CLI: print only file paths\n\
      --auto             GUI: start searching immediately\n\
      --cli              terminal mode (no window)\n\
  -h, --help             show this help\n\
  -V, --version          show version\n\n\
CLI exit status: 0 = found, 1 = nothing found, 2 = error.\n",
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
                "-g" | "--gitignore" => a.gitignore = true,
                "--no-recurse" => a.no_recurse = true,
                "-L" | "--follow" => a.follow = true,
                "-b" | "--binary" => a.binary = true,
                "-x" | "--exclude" => a.exclude = Some(val(&arg)?),
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
            include_hidden: self.hidden,
            respect_gitignore: self.gitignore,
            recurse: !self.no_recurse,
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
    }
}
