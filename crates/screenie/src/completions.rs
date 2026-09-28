//! Shell completions, made from the CLI's definition like the man pages:
//! `screenie completions SHELL` prints one. `tools/dist.py` puts bash's, zsh's and fish's
//! in the release archives, where those shells look for them.

use clap::ValueEnum;
use clap_complete::Generator;
use clap_complete_nushell::Nushell;

#[derive(Clone, Copy, ValueEnum)]
pub enum Shell {
    Bash,
    Elvish,
    Fish,
    Nushell,
    Powershell,
    Zsh,
}

/// `shell`'s completion script for `cli`.
pub fn script(cli: &clap::Command, shell: Shell) -> Vec<u8> {
    use clap_complete::Shell as Clap;
    let mut cli = without_hidden(cli);
    match shell {
        Shell::Bash => generate(Clap::Bash, &mut cli),
        Shell::Elvish => generate(Clap::Elvish, &mut cli),
        Shell::Fish => generate(Clap::Fish, &mut cli),
        Shell::Nushell => generate(Nushell, &mut cli),
        Shell::Powershell => generate(Clap::PowerShell, &mut cli),
        Shell::Zsh => generate(Clap::Zsh, &mut cli),
    }
}

fn generate(generator: impl Generator, cli: &mut clap::Command) -> Vec<u8> {
    let mut script = Vec::new();
    let name = cli.get_name().to_owned();
    clap_complete::generate(generator, cli, name, &mut script);
    script
}

/// `cli` without its hidden commands (packaging's `man` and `share`), which clap's
/// scripts would offer with the rest. clap can't remove a subcommand, so this is the top
/// level made again, which is all that has hidden ones: its arguments, and the other
/// commands as they are.
fn without_hidden(cli: &clap::Command) -> clap::Command {
    let mut visible = clap::Command::new(cli.get_name().to_owned())
        .args(cli.get_arguments().cloned())
        .subcommands(cli.get_subcommands().filter(|s| !s.is_hide_set()).cloned());
    if let Some(version) = cli.get_version() {
        visible = visible.version(version.to_owned());
    }
    if let Some(about) = cli.get_about() {
        visible = visible.about(about.clone());
    }
    visible
}

#[cfg(test)]
mod tests {
    use clap::CommandFactory;

    use super::*;

    #[test]
    fn every_shell_gets_a_script() {
        for shell in Shell::value_variants() {
            let script = String::from_utf8(script(&crate::Cli::command(), *shell)).unwrap();
            // The commands, down to aliases.
            assert!(script.contains("screenshot"), "{script}");
        }
    }

    #[test]
    fn only_visible_commands_are_offered() {
        let names = |cli: &clap::Command| -> Vec<String> {
            let visible = cli.get_subcommands().filter(|s| !s.is_hide_set());
            visible.map(|s| s.get_name().to_owned()).collect()
        };
        let cli = crate::Cli::command();
        let offered: Vec<String> = without_hidden(&cli)
            .get_subcommands()
            .map(|s| s.get_name().to_owned())
            .collect();
        assert_eq!(offered, names(&cli));
        assert!(!offered.iter().any(|name| name == "man"));
    }
}
