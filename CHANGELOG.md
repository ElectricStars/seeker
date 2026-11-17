# Changelog

All notable changes to Seeker are listed here. The format follows
[Keep a Changelog](https://keepachangelog.com/) and versions follow [SemVer](https://semver.org/).

## [0.1.2] - Unreleased

### Added
- **Preferences** window (*Settings → Preferences…*) with a sidebar for its two pages,
  *Appearance* and *Font and Size* (#1, #2, #5). It opens as its own window that can be moved
  anywhere (#6); where separate windows are not supported it opens inside the main window.
  - Appearance: Light (new default), Dark, Follow system, or a custom background color
    (color picker plus presets). Text color is picked automatically for the best contrast,
    or can be chosen; the page shows the contrast ratio and warns when text would be hard to read.
  - Font and Size: one font for everything, from the built-in fonts or any font installed on
    the system (found with fontconfig's `fc-list`), and one text size picked from a list
    (10–32 pt) or stepped with − / +. Fields, buttons and spacing grow with the text.
  - Changes preview live in the main window. **Save** keeps them (the window stays open),
    **Cancel** goes back to the last saved settings, **Close** with unsaved changes asks
    whether to save or discard them.
- `--skip-hidden` command-line option.
- Wildcards typed in quotes (`"*.md"`) or separated by spaces (`*.md *.txt`) work.
- When a search finds nothing, the window says which options narrowed it
  (e.g. "Containing text" still filled in, hidden files skipped).

### Changed
- No sliders anywhere: the text size is a list with − / + buttons, and "Context lines" is a
  list (0–10). The separate interface scale and preview font are gone; Ctrl +/− no longer
  zoom the window.
- Larger default text (16 pt instead of 13 pt), larger fields and buttons, larger default window.
- A plain search now looks everywhere, like Agent Ransack: hidden files and folders are
  included and no folders are excluded by default. Skipping hidden files, `.gitignore` rules
  and excluded folders are opt-in. Saved forms from 0.1.1 that still had the old defaults are
  updated once.

### Fixed
- The buttons and options on the right of the search form ran past the window edge at large
  text sizes or narrow widths (#7). The form now wraps, the results columns shrink to fit, and
  the window has a minimum size.
- File name search `*.md` missed `.md` files (#3): files in hidden folders (such as
  `~/.config` or `~/.local`), hidden files, files under `node_modules`, `target`, `.git` and
  `__pycache__`, and symlinks to files were all skipped by default.
- Symbolic links to files are always listed; "Follow symlinks" now only controls entering
  symlinked folders.
- Wildcards without `/` are matched against the file name only. Before, they were also matched
  against the full path, so a bare word such as `!test` excluded every file under a folder whose
  path contained "test".

### Packaging
- `.deb` for Ubuntu 20.04+ / Debian 11+ built with `cargo deb` and attached to releases.
- Debian source packaging (`debian/`) for the `ppa:electricstars/seeker` Launchpad PPA, built
  offline from vendored crates with Ubuntu's `rustc-1.97`; `packaging/make-ppa-source.sh` builds it.
- Manual page `seeker(1)`.

## [0.1.1] - 2026-11-11

### Added
- AppStream metainfo (`data/io.github.ElectricStars.Seeker.metainfo.xml`) for software centers.
- CI job that validates the desktop file and metainfo.
- More screenshots.

### Changed
- Desktop file and icon renamed to the application id `io.github.ElectricStars.Seeker`;
  the window's app id matches it so Wayland desktops show the right icon.
  `install.sh` removes the 0.1.0 `seeker.desktop` / `seeker.svg` files when upgrading.
- Inside a Flatpak sandbox, "Open" and "Open containing folder" use the OpenURI desktop portal,
  and "Open terminal here" copies a `cd` command to the clipboard instead.
- Folders picked through the file-chooser portal are shown and searched by their real path,
  and exports saved through the portal land where the user chose.

### Fixed
- The window now repaints as soon as a folder or export dialog closes.

## [0.1.0] - 2026-11-11

### Added
- First release: name and content search, filters, live results, preview pane with
  highlighted hits, context menu actions, CSV/text export, `--cli` mode.

[0.1.1]: https://github.com/ElectricStars/seeker/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/ElectricStars/seeker/releases/tag/v0.1.0
