// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Eric Allen

//! Export search results to CSV or plain text.

use std::io::{self, Write};
use std::time::SystemTime;

use crate::search::FileHit;

pub fn format_time(t: Option<SystemTime>) -> String {
    match t {
        Some(t) => chrono::DateTime::<chrono::Local>::from(t)
            .format("%Y-%m-%d %H:%M")
            .to_string(),
        None => String::new(),
    }
}

/// CSV with one row per file.
pub fn write_csv<W: Write>(w: W, hits: &[FileHit]) -> io::Result<()> {
    let mut wtr = csv::Writer::from_writer(w);
    wtr.write_record([
        "path",
        "name",
        "folder",
        "size_bytes",
        "modified",
        "hits",
        "hit_lines",
    ])?;
    for h in hits {
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
        wtr.write_record([
            h.path.to_string_lossy().as_ref(),
            name.as_ref(),
            folder.as_ref(),
            &h.size.to_string(),
            &format_time(h.modified),
            &h.hits.to_string(),
            &h.hit_lines.to_string(),
        ])?;
    }
    wtr.flush()
}

/// Plain text: each file followed by its matching lines.
pub fn write_text<W: Write>(mut w: W, hits: &[FileHit]) -> io::Result<()> {
    for h in hits {
        if h.hit_lines == 0 {
            writeln!(w, "{}", h.path.display())?;
            continue;
        }
        writeln!(w, "{}  ({} hits)", h.path.display(), h.hits)?;
        for l in h.lines.iter().filter(|l| l.is_hit) {
            writeln!(w, "  {:>6}: {}", l.number, l.text)?;
        }
        if h.truncated {
            writeln!(w, "  … (more lines not shown)")?;
        }
        writeln!(w)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::search::Line;
    use std::path::PathBuf;

    fn sample() -> Vec<FileHit> {
        vec![FileHit {
            path: PathBuf::from("/tmp/a, b/file.txt"),
            size: 12,
            modified: None,
            hits: 2,
            hit_lines: 1,
            lines: vec![Line {
                number: 7,
                text: "foo foo".into(),
                ranges: vec![(0, 3), (4, 7)],
                is_hit: true,
            }],
            truncated: false,
        }]
    }

    #[test]
    fn csv_quotes_commas() {
        let mut out = Vec::new();
        write_csv(&mut out, &sample()).unwrap();
        let s = String::from_utf8(out).unwrap();
        assert!(s.starts_with("path,name,folder,size_bytes"));
        assert!(s.contains("\"/tmp/a, b/file.txt\",file.txt,\"/tmp/a, b\",12,,2,1"));
    }

    #[test]
    fn text_lists_lines() {
        let mut out = Vec::new();
        write_text(&mut out, &sample()).unwrap();
        let s = String::from_utf8(out).unwrap();
        assert!(s.contains("(2 hits)"));
        assert!(s.contains("     7: foo foo"));
    }
}
