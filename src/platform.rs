// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Eric Allen

//! Desktop integration: open files, show folders, launch a terminal.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Prints a diagnostic line to stderr when `SEEKER_DEBUG` is set.
pub fn debug(msg: &str) {
    if std::env::var_os("SEEKER_DEBUG").is_some() {
        eprintln!("seeker: {msg}");
    }
}

/// True when running inside a Flatpak sandbox.
pub fn in_flatpak() -> bool {
    Path::new("/.flatpak-info").exists()
}

/// If `path` lies inside the document portal's FUSE mount
/// (`<runtime dir>/doc/<id>/<name>/...`, normally `/run/user/<uid>/doc`),
/// returns the document id and the path below the exported item.
pub fn split_doc_path(path: &Path) -> Option<(String, PathBuf)> {
    let comps: Vec<_> = path.components().collect();
    // Use the last "doc" component that is followed by an id and a name.
    let i = (0..comps.len().saturating_sub(2))
        .rev()
        .find(|&i| comps[i].as_os_str() == "doc")?;
    let id = comps[i + 1].as_os_str().to_str()?;
    if id.is_empty() || id.len() > 32 || !id.chars().all(|c| c.is_ascii_alphanumeric()) {
        return None;
    }
    let rest: PathBuf = comps[i + 3..].iter().collect();
    Some((id.to_string(), rest))
}

/// The portal reports document paths relative to the *host's* runtime
/// directory. Inside the sandbox the document store is mounted at
/// `$XDG_RUNTIME_DIR/doc`; if the reported prefix isn't visible here, map it.
pub fn sandbox_doc_path(path: PathBuf) -> PathBuf {
    if !in_flatpak() || path.parent().is_some_and(|p| p.exists()) {
        return path;
    }
    let (Some(runtime), Some((id, _))) =
        (std::env::var_os("XDG_RUNTIME_DIR"), split_doc_path(&path))
    else {
        return path;
    };
    let comps: Vec<_> = path.components().collect();
    let Some(i) = comps.iter().position(|c| c.as_os_str() == id.as_str()) else {
        return path;
    };
    let mut mapped = PathBuf::from(runtime).join("doc");
    mapped.extend(&comps[i..]);
    debug(&format!(
        "mapped {} to {}",
        path.display(),
        mapped.display()
    ));
    mapped
}

/// Folders picked through the FileChooser portal arrive as document-portal
/// paths inside Flatpak. When the sandbox can read the real location anyway
/// (Seeker has read-only host access), translate back to the real path so
/// results show normal paths and searching avoids the slower FUSE layer.
pub fn resolve_portal_path(path: PathBuf) -> PathBuf {
    if !in_flatpak() {
        return path;
    }
    let Some((id, rest)) = split_doc_path(&path) else {
        return path;
    };
    let host = pollster::block_on(async {
        let docs = ashpd::documents::Documents::new()
            .await
            .map_err(|e| debug(&format!("documents portal: {e}")))
            .ok()?;
        let id = ashpd::documents::DocumentID::from(id);
        let map = docs
            .host_paths(std::slice::from_ref(&id))
            .await
            .map_err(|e| debug(&format!("GetHostPaths: {e}")))
            .ok()?;
        map.get(&id).map(|p| AsRef::<Path>::as_ref(p).to_path_buf())
    });
    debug(&format!("host path for {}: {host:?}", path.display()));
    match host {
        Some(h) => {
            let real = if rest.as_os_str().is_empty() {
                h
            } else {
                h.join(rest)
            };
            if real.exists() { real } else { path }
        }
        None => path,
    }
}

/// Asks the desktop's OpenURI portal to open a file (or to show it in its
/// folder). The file is passed as a file descriptor, so this works from inside
/// the sandbox without any extra permissions.
fn portal_open(path: &Path, show_in_folder: bool) -> Result<(), String> {
    use ashpd::desktop::open_uri::{OpenDirectoryRequest, OpenFileRequest};
    let file =
        std::fs::File::open(path).map_err(|e| format!("Could not open {}: {e}", path.display()))?;
    pollster::block_on(async {
        if show_in_folder {
            OpenDirectoryRequest::default()
                .send(&file)
                .await
                .map(|_| ())
        } else {
            OpenFileRequest::default()
                .ask(false)
                .send_file(&file)
                .await
                .map(|_| ())
        }
    })
    .map_err(|e| format!("The desktop portal could not open {}: {e}", path.display()))
}

/// Opens a file with the default application (xdg-open, or the OpenURI portal
/// inside Flatpak).
pub fn open_file(path: &Path) -> Result<(), String> {
    if in_flatpak() {
        return portal_open(path, false);
    }
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
    if in_flatpak() {
        return portal_open(path, true);
    }
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

/// Returned by [`open_terminal`] inside Flatpak, where host programs can't be started.
pub const TERMINAL_UNAVAILABLE: &str = "terminal-unavailable-in-sandbox";

/// Opens a terminal in `dir`, trying common Linux terminals in turn.
/// `$TERMINAL` is honoured first.
pub fn open_terminal(dir: &Path) -> Result<(), String> {
    if in_flatpak() {
        return Err(TERMINAL_UNAVAILABLE.into());
    }
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
    use std::path::PathBuf;
    #[test]
    fn doc_paths() {
        assert_eq!(
            split_doc_path(Path::new("/run/user/1000/doc/ab12/proj/src")),
            Some(("ab12".to_string(), PathBuf::from("src")))
        );
        assert_eq!(
            split_doc_path(Path::new("/run/user/1000/doc/ab12/proj")),
            Some(("ab12".to_string(), PathBuf::new()))
        );
        assert_eq!(split_doc_path(Path::new("/home/me/proj")), None);
        assert_eq!(split_doc_path(Path::new("/run/user/1000/doc/ab12")), None);
        assert_eq!(split_doc_path(Path::new("/home/me/doc/my notes/x")), None);
    }
    #[test]
    fn uri_encoding() {
        assert_eq!(
            file_uri(Path::new("/a b/c#d.txt")),
            "file:///a%20b/c%23d.txt"
        );
    }
}
