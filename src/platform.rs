// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Eric Allen

//! Desktop integration: open files, show folders, launch a terminal.

use std::path::Path;
use std::process::{Command, Stdio};

/// Opens a file with the default application (xdg-open).
pub fn open_file(path: &Path) -> Result<(), String> {
    open::that_detached(path).map_err(|e| format!("Could not open {}: {e}", path.display()))
}

/// Percent-encodes a path into a `file://` URI.
pub fn file_uri(path: &Path) -> String {
    use std::os::unix::ffi::OsStrExt;
    let mut s = String::from("file://");
    for &b in path.as_os_str().as_bytes() {
        if b.is_ascii_alphanumeric() || b"/-_.~".contains(&b) {
            s.push(b as char);
        } else {
            s.push_str(&format!("%{b:02X}"));
        }
    }
    s
}

/// Opens the containing folder, selecting the file when the file manager
/// supports the freedesktop `FileManager1` D-Bus interface (Nautilus, Nemo, Dolphin…).
pub fn show_in_folder(path: &Path) -> Result<(), String> {
    let shown = Command::new("dbus-send")
        .args([
            "--session",
            "--print-reply",
            "--dest=org.freedesktop.FileManager1",
            "--type=method_call",
            "/org/freedesktop/FileManager1",
            "org.freedesktop.FileManager1.ShowItems",
        ])
        .arg(format!("array:string:{}", file_uri(path)))
        .arg("string:")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if shown {
        return Ok(());
    }
    let dir = if path.is_dir() {
        path
    } else {
        path.parent().unwrap_or(path)
    };
    open::that_detached(dir).map_err(|e| format!("Could not open {}: {e}", dir.display()))
}

/// Opens a terminal in `dir`, trying common Linux terminals in turn.
/// `$TERMINAL` is honoured first.
pub fn open_terminal(dir: &Path) -> Result<(), String> {
    let mut candidates: Vec<String> = Vec::new();
    if let Ok(t) = std::env::var("TERMINAL")
        && !t.is_empty()
    {
        candidates.push(t);
    }
    for t in [
        "x-terminal-emulator",
        "gnome-terminal",
        "kgx",
        "ptyxis",
        "konsole",
        "xfce4-terminal",
        "mate-terminal",
        "tilix",
        "alacritty",
        "kitty",
        "wezterm",
        "foot",
        "xterm",
    ] {
        candidates.push(t.to_string());
    }
    for t in &candidates {
        let ok = Command::new(t)
            .current_dir(dir)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .is_ok();
        if ok {
            return Ok(());
        }
    }
    Err("No terminal emulator found (set $TERMINAL)".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn uri_encoding() {
        assert_eq!(
            file_uri(Path::new("/a b/c#d.txt")),
            "file:///a%20b/c%23d.txt"
        );
    }
}
