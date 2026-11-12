# Changelog

All notable changes to Seeker are listed here. The format follows
[Keep a Changelog](https://keepachangelog.com/) and versions follow [SemVer](https://semver.org/).

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
