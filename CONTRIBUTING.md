# Contributing

Thanks for helping! Bug reports, ideas and pull requests are all welcome.

1. Fork and create a branch.
2. Make your change. Keep the search engine (`src/search.rs`) free of GUI code
   so it stays unit-testable.
3. Before opening a PR, run:
   ```sh
   cargo fmt
   cargo clippy --all-targets -- -D warnings
   cargo test
   ```
4. Describe what changed and how you tested it. Screenshots help for UI changes.

Layout:

| File | Purpose |
|------|---------|
| `src/search.rs` | Parallel walker + content search (ignore / grep crates), filters, tests |
| `src/ui.rs` | egui interface |
| `src/cli.rs` | Argument parsing and `--cli` mode |
| `src/export.rs` | CSV / text export |
| `src/platform.rs` | Open file, show in folder, open terminal |

By contributing you agree your work is released under the GNU General Public
License, version 3 or (at your option) any later version (GPL-3.0-or-later).
New source files should start with:

```rust
// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Eric Allen
```
