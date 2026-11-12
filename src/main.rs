// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Eric Allen

//! Seeker — a fast desktop file search tool for Linux.

mod cli;
mod export;
mod platform;
mod search;
mod ui;

/// Display name. To rename the app, change this, `APP_ID`, `DESKTOP_ID` and the
/// `name` in Cargo.toml (see README "Renaming").
pub const APP_NAME: &str = "Seeker";
/// Binary name / desktop-file id / settings folder name.
pub const APP_ID: &str = "seeker";
/// Reverse-DNS application id: desktop file, icon, AppStream and Flatpak id.
/// Wayland compositors use it to match the window to its desktop file.
pub const DESKTOP_ID: &str = "io.github.ElectricStars.Seeker";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

fn main() -> std::process::ExitCode {
    let args = match cli::Args::parse(std::env::args().skip(1)) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("{APP_ID}: {e}\nTry '{APP_ID} --help'.");
            return std::process::ExitCode::from(2);
        }
    };
    if args.help {
        print!("{}", cli::help());
        return std::process::ExitCode::SUCCESS;
    }
    if args.version {
        println!("{APP_NAME} {VERSION}");
        return std::process::ExitCode::SUCCESS;
    }
    if args.cli {
        return cli::run(&args);
    }
    match ui::run(args) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{APP_ID}: could not start the GUI: {e}");
            eprintln!("Tip: use '{APP_ID} --cli' for terminal mode.");
            std::process::ExitCode::FAILURE
        }
    }
}
