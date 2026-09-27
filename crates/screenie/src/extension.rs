//! `screenie extension`: screenie's GNOME Shell extension (`screenie_desktop::extension`).
//! Nothing to do with the daemon, so it's done here.

use std::process::ExitCode;

use screenie_desktop::Desktop;
use screenie_desktop::extension::{self, State};

use crate::ExtensionCommand;

pub fn run(action: ExtensionCommand) -> ExitCode {
    if Desktop::current() != Desktop::Gnome {
        println!(
            "Only GNOME needs screenie's extension: this is {}.",
            desktop_name()
        );
        return match action {
            ExtensionCommand::Install => ExitCode::from(2),
            _ => ExitCode::SUCCESS,
        };
    }
    let state = match action {
        ExtensionCommand::Show => extension::state(),
        ExtensionCommand::Install => match extension::install() {
            Ok(state) => state,
            Err(e) => return fail(e),
        },
        ExtensionCommand::Remove => match extension::remove() {
            Ok(()) => {
                println!("Removed screenie's GNOME Shell extension.");
                return ExitCode::SUCCESS;
            }
            Err(e) => return fail(e),
        },
    };
    println!("{}", describe(&state));
    ExitCode::SUCCESS
}

fn describe(state: &State) -> String {
    const WHAT: &str = "With it, screenshots are silent (no flash or sound, and no \
        permission to give), and windows can be picked.";
    match state {
        State::Running => format!("Screenie's GNOME Shell extension is running. {WHAT}"),
        State::NextLogin => "Screenie's GNOME Shell extension is installed, and runs from \
            your next login: log out and back in."
            .into(),
        State::Disabled => "Screenie's GNOME Shell extension is installed but turned off: \
            `screenie extension install` turns it on."
            .into(),
        State::ExtensionsOff => "Screenie's GNOME Shell extension is installed, but GNOME \
            has extensions turned off: turn them on in the Extensions app."
            .into(),
        State::OutOfDate => "Screenie's GNOME Shell extension isn't made for this version \
            of GNOME Shell; a newer screenie may have one that is."
            .into(),
        State::Failed(e) => format!("GNOME Shell couldn't run screenie's extension: {e}"),
        State::NotInstalled => format!(
            "Screenie's GNOME Shell extension isn't installed. {WHAT}\n\
             `screenie extension install` installs it; it runs from your next login."
        ),
    }
}

fn desktop_name() -> &'static str {
    match Desktop::current() {
        Desktop::Kde => "KDE Plasma",
        _ => "another desktop",
    }
}

fn fail(e: extension::Error) -> ExitCode {
    eprintln!("screenie: {e}");
    ExitCode::from(2)
}
