//! The man pages, made from what they document so they can't drift from it:
//! `screenie(1)` and one page per command down to each target (`screenie-shot(1)`,
//! `screenie-shot-region(1)`) from the CLI's definition, and `screenie(5)` from the
//! config's reference ([`screenie_config::reference`]). Packaging writes them with the
//! hidden `screenie man DIR`.

use std::fs::File;
use std::io::{self, BufWriter, Write};
use std::path::Path;

use clap_mangen::Man;
use clap_mangen::roff::{Inline, Roff, bold, italic, roman};
use screenie_config::reference::{self, Key};

/// Write every page into `dir/man1` and `dir/man5`, creating them if needed.
pub fn write(cli: clap::Command, dir: &Path) -> io::Result<()> {
    let commands = dir.join("man1");
    std::fs::create_dir_all(&commands)?;
    let mut cli = without_help_subcommands(cli);
    // Names each subcommand for its page: `screenie-shot`.
    cli.build();
    write_command_page(&cli, &commands, true)?;

    let files = dir.join("man5");
    std::fs::create_dir_all(&files)?;
    let mut out = BufWriter::new(File::create(files.join("screenie.5"))?);
    config_page().to_writer(&mut out)?;
    out.flush()
}

const SOURCE: &str = concat!("screenie ", env!("CARGO_PKG_VERSION"));
const DATE: &str = env!("SCREENIE_COMMIT_DATE");

/// `screenie help shot` is `--help` again, so it gets no pages.
fn without_help_subcommands(cmd: clap::Command) -> clap::Command {
    let names: Vec<String> = cmd
        .get_subcommands()
        .map(|s| s.get_name().to_owned())
        .collect();
    names
        .iter()
        .fold(cmd.disable_help_subcommand(true), |cmd, name| {
            cmd.mut_subcommand(name, without_help_subcommands)
        })
}

fn write_command_page(cmd: &clap::Command, dir: &Path, top: bool) -> io::Result<()> {
    for sub in visible_subcommands(cmd) {
        write_command_page(sub, dir, false)?;
    }
    let mut cmd = cmd.clone();
    // Section headings are upper case: `.SH TARGETS`.
    if let Some(heading) = cmd.get_subcommand_help_heading() {
        let heading = heading.to_uppercase();
        cmd = cmd.subcommand_help_heading(heading);
    }
    let man = Man::new(cmd.clone()).source(SOURCE).date(DATE);
    let mut out = BufWriter::new(File::create(dir.join(man.get_filename()))?);
    man.render_title(&mut out)?;
    man.render_name_section(&mut out)?;
    man.render_synopsis_section(&mut out)?;
    // Without a longer description, NAME has already said it.
    if cmd.get_long_about().is_some() {
        man.render_description_section(&mut out)?;
    }
    if cmd.get_arguments().any(|a| !a.is_hide_set()) {
        man.render_options_section(&mut out)?;
    }
    if visible_subcommands(&cmd).next().is_some() {
        man.render_subcommands_section(&mut out)?;
    }
    if top {
        about_screenie().to_writer(&mut out)?;
        man.render_version_section(&mut out)?;
    } else {
        let mut roff = Roff::new();
        roff.control("SH", ["SEE ALSO"])
            .text(page_ref("screenie", 1));
        roff.to_writer(&mut out)?;
    }
    out.flush()
}

fn visible_subcommands(cmd: &clap::Command) -> impl Iterator<Item = &clap::Command> {
    cmd.get_subcommands().filter(|s| !s.is_hide_set())
}

/// What `screenie(1)` says beyond the CLI's definition.
fn about_screenie() -> Roff {
    let mut roff = Roff::new();
    list(
        &mut roff,
        "EXIT STATUS",
        &[
            ("0", "Done."),
            (
                "1",
                "Cancelled, or nothing to show: the selector or editor closed without a \
                 capture, `record stop` came during the countdown, or `query last` found \
                 none.",
            ),
            ("2", "An error, described on stderr."),
        ],
    );
    list(
        &mut roff,
        "ENVIRONMENT",
        &[
            (
                "SCREENIE_LOG",
                "What to log, as a tracing filter such as `debug` or \
                 `screenie_record=trace`. The daemon logs info and up to its log file, \
                 commands log warnings to stderr.",
            ),
            (
                "WAYLAND_DISPLAY",
                "The display screenie works on. Each display gets its own daemon.",
            ),
        ],
    );
    list(
        &mut roff,
        "FILES",
        &[
            (
                "$XDG_CONFIG_HOME/screenie/config.yaml",
                "The configuration (in ~/.config by default), described in screenie(5).",
            ),
            (
                "$XDG_STATE_HOME/screenie/state.yaml",
                "What screenie remembers between runs, such as the last region (in \
                 ~/.local/state by default).",
            ),
            (
                "$XDG_STATE_HOME/screenie/daemon.log",
                "The daemon's log. The previous run's is kept as daemon.log.1.",
            ),
            (
                "$XDG_RUNTIME_DIR/screenie/",
                "The daemon's socket, one per Wayland display.",
            ),
        ],
    );
    roff.control("SH", ["SEE ALSO"])
        .text(page_ref("screenie", 5));
    roff.control("PP", []).text([
        roman("Compositor support and troubleshooting: "),
        italic(env!("CARGO_PKG_REPOSITORY")),
    ]);
    roff
}

/// `screenie(5)`: the config file, key by key.
fn config_page() -> Roff {
    let mut roff = Roff::new();
    roff.control("TH", ["screenie", "5", DATE, SOURCE, ""]);
    roff.control("SH", ["NAME"])
        .text([roman("screenie - configuration file for screenie")]);
    roff.control("SH", ["SYNOPSIS"])
        .text([bold("$XDG_CONFIG_HOME/screenie/config.yaml")]);
    roff.control("SH", ["DESCRIPTION"]).text(code(
        "screenie reads its settings from this YAML file, in ~/.config unless \
         XDG_CONFIG_HOME says otherwise. Every key is optional: one left out has its \
         default, and one screenie doesn't know is ignored, so an empty file, or none, is \
         fine. The daemon picks up changes as the file is saved.",
    ));
    roff.control("PP", []).text(code(
        "Keys are written here with dots, each with its default: `recording.framerate: 60` \
         is",
    ));
    literal(&mut roff, "recording:\n  framerate: 60");

    roff.control("SH", ["KEYS"]);
    for key in reference::keys() {
        config_key(&mut roff, &key);
    }

    roff.control("SH", ["EXAMPLE"]);
    literal(&mut roff, reference::EXAMPLE.trim_end());
    roff.control("SH", ["SEE ALSO"])
        .text(page_ref("screenie", 1));
    roff.control("PP", []).text([
        roman("Presets that make screenie work like other screenshot tools: "),
        italic(concat!(env!("CARGO_PKG_REPOSITORY"), "#presets")),
    ]);
    roff
}

fn config_key(roff: &mut Roff, key: &Key) {
    let mut paragraphs = key.description.iter();
    if key.is_group() && !key.path.contains('.') {
        // A top-level group is a subsection: `.SS screenshot`.
        roff.control("SS", [key.path.as_str()]);
        for paragraph in paragraphs {
            roff.control("PP", []).text(code(paragraph));
        }
        return;
    }
    let mut tag = vec![bold(&key.path)];
    if let Some(default) = &key.default {
        tag.extend([roman(": "), roman(default)]);
    }
    roff.control("TP", []).text(tag);
    if let Some(first) = paragraphs.next() {
        roff.text(code(first));
    }
    for paragraph in paragraphs {
        roff.control("sp", []).text(code(paragraph));
    }
    if key.values.iter().any(|(_, doc)| doc.is_some()) {
        for (i, (word, doc)) in key.values.iter().enumerate() {
            let mut line = vec![bold(word), roman(": ")];
            line.extend(code(doc.as_deref().unwrap_or_default()));
            roff.control(if i == 0 { "sp" } else { "br" }, [])
                .text(line);
        }
    } else if !key.values.is_empty() {
        let mut line = vec![roman("One of ")];
        for (i, (word, _)) in key.values.iter().enumerate() {
            match i {
                0 => {}
                _ if i + 1 == key.values.len() => line.push(roman(" or ")),
                _ => line.push(roman(", ")),
            }
            line.push(bold(word));
        }
        line.push(roman("."));
        roff.control("sp", []).text(line);
    }
}

/// Text with its `code` in bold.
fn code(text: &str) -> Vec<Inline> {
    text.split('`')
        .enumerate()
        .filter(|(_, part)| !part.is_empty())
        .map(|(i, part)| if i % 2 == 1 { bold(part) } else { roman(part) })
        .collect()
}

/// Lines shown as they are, indented.
fn literal(roff: &mut Roff, text: &str) {
    roff.control("RS", ["4"]).control("nf", []);
    for line in text.lines() {
        roff.text([roman(line)]);
    }
    roff.control("fi", []).control("RE", []);
}

/// A section of terms, each with a paragraph.
fn list(roff: &mut Roff, heading: &str, items: &[(&str, &str)]) {
    roff.control("SH", [heading]);
    for (term, text) in items {
        roff.control("TP", []).text([bold(*term)]).text(code(text));
    }
}

fn page_ref(page: &str, section: u8) -> Vec<Inline> {
    vec![bold(page), roman(format!("({section})"))]
}
