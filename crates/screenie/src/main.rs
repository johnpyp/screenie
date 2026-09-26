//! `screenie`: screenshots and screen recordings for Wayland.
//!
//! Every command talks to the resident daemon, starting it if needed. Bind the commands
//! you like to keys in your compositor, e.g. for Hyprland:
//!
//! ```text
//! bind = , Print, exec, screenie shot
//! bind = SHIFT, Print, exec, screenie shot screen
//! bind = ALT, Print, exec, screenie record
//! ```

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::Context;
use clap::{Args, Parser, Subcommand, ValueEnum};
use screenie_core::Rect;
use screenie_ipc::{
    ActionOverrides, CaptureKind, Client, RecordRequest, Request, Response, ScreenshotRequest,
    SelectMode, State, Status, Target,
};

/// `0.1.0 (46bce20 2026-09-25 23:20)`
const VERSION: &str = concat!(
    env!("CARGO_PKG_VERSION"),
    " (",
    env!("SCREENIE_COMMIT"),
    ")"
);

#[derive(Parser)]
#[command(
    name = "screenie",
    version = VERSION,
    about = "Screenshots and screen recordings for Wayland",
    arg_required_else_help = true
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Take a screenshot.
    #[command(visible_alias = "screenshot")]
    Shot(ShotArgs),
    /// Record the screen, or control the running recording.
    Record(RecordArgs),
    /// Open an image in the annotation editor.
    Edit { file: PathBuf },
    /// Ask what screenie is doing or has done, for scripts and status bars. Never
    /// starts the daemon: if it isn't running, screenie is idle.
    #[command(subcommand)]
    Query(Query),
    /// What screenie is doing (short for `query status`).
    Status(StatusArgs),
    /// Run the daemon in the foreground (normally started automatically). Upgrades a
    /// daemon running another build, unless it is in use.
    Daemon,
    /// Stop the daemon.
    Quit,
}

// A target carries its own options, which follow it: `screenie shot window --copy`.
// Without a target, the options go straight after the command: `screenie shot --copy`.

/// What to capture. With none, `pick`.
#[derive(Subcommand)]
enum ShotTarget {
    /// Drag an area, click a window, or press Enter to capture the whole screen.
    Pick(ShotOptions),
    /// The focused window.
    Window {
        #[command(flatten)]
        window: WindowTarget,
        #[command(flatten)]
        options: ShotOptions,
    },
    /// The focused screen, or the one named.
    Screen {
        #[command(flatten)]
        screen: ScreenTarget,
        #[command(flatten)]
        options: ShotOptions,
    },
    /// All screens as one image.
    All(ShotOptions),
    /// The same region as last time.
    Last(ShotOptions),
    /// A region of the desktop.
    Region {
        #[command(flatten)]
        region: RegionTarget,
        #[command(flatten)]
        options: ShotOptions,
    },
}

impl ShotTarget {
    fn resolve(self) -> (Target, ShotOptions) {
        match self {
            ShotTarget::Pick(options) => (pick(), options),
            ShotTarget::Window { window, options } => (window.target(), options),
            ShotTarget::Screen { screen, options } => (screen.target(), options),
            ShotTarget::All(options) => (Target::AllScreens, options),
            ShotTarget::Last(options) => (Target::LastRegion, options),
            ShotTarget::Region { region, options } => (region.target(), options),
        }
    }
}

/// What to record, or what to do with the running recording. With none, `pick`.
#[derive(Subcommand)]
enum RecordCommand {
    /// Pick an area, window or screen, then press Record.
    Pick(RecordOptions),
    /// The focused window.
    Window {
        #[command(flatten)]
        window: WindowTarget,
        #[command(flatten)]
        options: RecordOptions,
    },
    /// The focused screen, or the one named.
    Screen {
        #[command(flatten)]
        screen: ScreenTarget,
        #[command(flatten)]
        options: RecordOptions,
    },
    /// The same region as last time.
    Last(RecordOptions),
    /// A region of the desktop.
    Region {
        #[command(flatten)]
        region: RegionTarget,
        #[command(flatten)]
        options: RecordOptions,
    },
    /// Stop the running recording and save it.
    Stop,
    /// Pause or resume the running recording.
    Pause,
    /// Stop the running recording and discard it.
    Cancel,
}

fn pick() -> Target {
    Target::Select {
        mode: SelectMode::Area,
    }
}

#[derive(Args)]
struct WindowTarget {
    /// Click the window instead.
    #[arg(short, long)]
    interactive: bool,
}

impl WindowTarget {
    fn target(self) -> Target {
        if self.interactive {
            Target::Select {
                mode: SelectMode::Window,
            }
        } else {
            Target::ActiveWindow
        }
    }
}

#[derive(Args)]
struct ScreenTarget {
    /// The screen's connector name, such as DP-1.
    #[arg(conflicts_with = "interactive")]
    name: Option<String>,
    /// Click the screen instead.
    #[arg(short, long)]
    interactive: bool,
}

impl ScreenTarget {
    fn target(self) -> Target {
        if self.interactive {
            Target::Select {
                mode: SelectMode::Screen,
            }
        } else {
            Target::Screen { output: self.name }
        }
    }
}

#[derive(Args)]
struct RegionTarget {
    /// "X,Y WxH" or "WxH+X+Y", in logical coordinates.
    #[arg(value_parser = parse_rect)]
    rect: Rect,
}

impl RegionTarget {
    fn target(self) -> Target {
        Target::Region { rect: self.rect }
    }
}

/// What happens to a screenshot, overriding `screenshot.after_capture`.
#[derive(Args)]
struct Delivery {
    /// Copy to the clipboard.
    #[arg(long, overrides_with = "no_copy")]
    copy: bool,
    #[arg(long, hide = true)]
    no_copy: bool,
    /// Save to the screenshots folder.
    #[arg(long, overrides_with = "no_save")]
    save: bool,
    /// Don't save a file.
    #[arg(long)]
    no_save: bool,
    /// Don't show the preview card.
    #[arg(long)]
    no_preview: bool,
    /// Open the capture in the editor.
    #[arg(long)]
    edit: bool,
    /// Save to this file, or into this directory.
    #[arg(short, long)]
    output: Option<PathBuf>,
}

impl Delivery {
    fn overrides(&self) -> ActionOverrides {
        let flag = |yes: bool, no: bool| {
            if yes {
                Some(true)
            } else if no {
                Some(false)
            } else {
                None
            }
        };
        ActionOverrides {
            copy: flag(self.copy, self.no_copy),
            save: flag(self.save, self.no_save),
            preview: self.no_preview.then_some(false),
            edit: self.edit.then_some(true),
        }
    }
}

#[derive(Args)]
#[command(
    args_conflicts_with_subcommands = true,
    disable_help_subcommand = true,
    subcommand_value_name = "TARGET",
    subcommand_help_heading = "Targets"
)]
struct ShotArgs {
    #[command(subcommand)]
    target: Option<ShotTarget>,
    #[command(flatten)]
    options: ShotOptions,
}

#[derive(Args)]
struct ShotOptions {
    /// Wait this many seconds first.
    #[arg(short, long, default_value_t = 0)]
    delay: u32,
    /// Include the mouse cursor.
    #[arg(long)]
    cursor: bool,
    /// Write the PNG to stdout.
    #[arg(long)]
    stdout: bool,
    #[command(flatten)]
    delivery: Delivery,
}

#[derive(Args)]
#[command(args_conflicts_with_subcommands = true, disable_help_subcommand = true)]
struct RecordArgs {
    #[command(subcommand)]
    command: Option<RecordCommand>,
    #[command(flatten)]
    options: RecordOptions,
}

#[derive(Args)]
struct RecordOptions {
    /// Record system audio.
    #[arg(long)]
    audio: bool,
    /// Record the microphone.
    #[arg(long)]
    mic: bool,
    /// Save to this file.
    #[arg(short, long)]
    output: Option<PathBuf>,
    /// Fail instead of stopping a running recording.
    #[arg(long)]
    no_toggle: bool,
}

fn parse_rect(s: &str) -> Result<Rect, String> {
    s.parse()
}

fn main() -> ExitCode {
    let command = Cli::parse().command;

    if let Command::Daemon = command {
        let commit = env!("SCREENIE_COMMIT");
        let describe = |s: &screenie_ipc::Status| {
            let build = if s.commit.is_empty() {
                "an older build".to_string()
            } else {
                s.commit.clone()
            };
            format!("{build}, pid {}", s.pid)
        };
        match Client::take_over() {
            Ok(screenie_ipc::Takeover::NotRunning) => {}
            Ok(screenie_ipc::Takeover::Replaced(old)) if old.commit == commit => {
                eprintln!(
                    "screenie: replaced the running daemon (pid {}), a different binary of this commit",
                    old.pid
                );
            }
            Ok(screenie_ipc::Takeover::Replaced(old)) => {
                eprintln!(
                    "screenie: upgraded the running daemon ({}) to this build ({commit})",
                    describe(&old)
                );
            }
            Ok(screenie_ipc::Takeover::Current(s)) => {
                eprintln!(
                    "screenie: the daemon is already running this build ({})",
                    describe(&s)
                );
                return ExitCode::SUCCESS;
            }
            Ok(screenie_ipc::Takeover::Busy(s)) => {
                eprintln!(
                    "screenie: the running daemon ({}) is another build but is busy ({}); \
                     run this again once it's done, or `screenie quit` to stop it now",
                    describe(&s),
                    s.state.as_str()
                );
                return ExitCode::from(2);
            }
            Err(e) => {
                eprintln!("screenie: {e:#}");
                return ExitCode::from(2);
            }
        }
        let daemon = match screenie_app::claim() {
            Ok(daemon) => daemon,
            Err(e) => {
                eprintln!("screenie daemon: {e:#}");
                return ExitCode::from(2);
            }
        };
        init_daemon_logging();
        return match daemon.run(commit) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("screenie daemon: {e:#}");
                ExitCode::from(2)
            }
        };
    }
    init_logging();

    match run(command) {
        Ok(code) => code,
        // Whoever read our output stopped (`| head -1`): nothing left to say.
        Err(e) if stdout_closed(&e) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("screenie: {e:#}");
            ExitCode::from(2)
        }
    }
}

/// A command logs warnings to stderr.
fn init_logging() {
    use std::io::IsTerminal;
    tracing_subscriber::fmt()
        .with_env_filter(log_filter("warn"))
        .with_writer(std::io::stderr)
        .with_ansi(std::io::stderr().is_terminal())
        .with_timer(log_timer())
        .init();
}

/// The daemon always logs to its file, however it was started: the previous run's is
/// kept as `daemon.log.1`, so a crash's log survives the restart it causes. Where stderr
/// leads somewhere (a terminal, a pipe, the journal) it's logged there too; where it
/// goes nowhere (a daemon the CLI started), stdout and stderr are pointed at the file,
/// so panics and what native libraries print land in it too.
fn init_daemon_logging() {
    use std::io::IsTerminal;
    use tracing_subscriber::prelude::*;

    let path = screenie_ipc::daemon_log_path();
    let file = path
        .parent()
        .map_or(Ok(()), std::fs::create_dir_all)
        .and_then(
            |()| match std::fs::rename(&path, path.with_extension("log.1")) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
                _ => Ok(()),
            },
        )
        .and_then(|()| std::fs::File::create(&path));
    let file = match file {
        Ok(file) => Some(file),
        Err(e) => {
            eprintln!("screenie daemon: can't write {}: {e}", path.display());
            None
        }
    };
    let stderr_nowhere = stderr_is_null();
    let (to_file, to_stderr) = match file {
        Some(file) if stderr_nowhere => {
            let _ = rustix::stdio::dup2_stdout(&file);
            let _ = rustix::stdio::dup2_stderr(&file);
            (None, true)
        }
        Some(file) => (Some(file), true),
        None => (None, !stderr_nowhere),
    };
    let tee_panics = to_file.is_some();
    tracing_subscriber::registry()
        .with(log_filter("info"))
        .with(to_file.map(|file| {
            tracing_subscriber::fmt::layer()
                .with_writer(std::sync::Mutex::new(file))
                .with_ansi(false)
                .with_timer(log_timer())
        }))
        .with(to_stderr.then(|| {
            tracing_subscriber::fmt::layer()
                .with_writer(std::io::stderr)
                .with_ansi(std::io::stderr().is_terminal())
                .with_timer(log_timer())
        }))
        .init();
    if tee_panics {
        // A panic prints to stderr only; the file gets it through the log.
        let print = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            tracing::error!("{info}");
            print(info);
        }));
    }
}

/// `SCREENIE_LOG`, or `default`.
fn log_filter(default: &str) -> tracing_subscriber::EnvFilter {
    tracing_subscriber::EnvFilter::try_from_env("SCREENIE_LOG").unwrap_or_else(|_| default.into())
}

/// Local time, like the capture file names.
fn log_timer() -> tracing_subscriber::fmt::time::ChronoLocal {
    tracing_subscriber::fmt::time::ChronoLocal::rfc_3339()
}

/// Whether stderr is `/dev/null` (or closed).
fn stderr_is_null() -> bool {
    let Ok(stderr) = rustix::fs::fstat(std::io::stderr()) else {
        return true;
    };
    rustix::fs::stat("/dev/null").is_ok_and(|null| {
        rustix::fs::FileType::from_raw_mode(stderr.st_mode) == rustix::fs::FileType::CharacterDevice
            && stderr.st_rdev == null.st_rdev
    })
}

fn request(req: &Request) -> anyhow::Result<Response> {
    Ok(Client::connect_or_spawn()?.request(req)?)
}

/// Send `req` to the running daemon, or answer `otherwise` if there is none. For commands
/// that act on what the daemon is doing: starting one just to hear it's doing nothing
/// would leave it resident for no reason.
fn request_running(req: &Request, otherwise: Response) -> anyhow::Result<Response> {
    match Client::connect() {
        Ok(client) => Ok(client.request(req)?),
        Err(_) => Ok(otherwise),
    }
}

fn run(command: Command) -> anyhow::Result<ExitCode> {
    let response = match command {
        Command::Shot(args) => return shot(args),
        Command::Record(args) => record(args)?,
        Command::Edit { file } => request(&Request::Edit {
            path: existing(&file)?,
        })?,
        Command::Query(Query::Last { kind, json, watch }) => {
            let kind = kind.map(|k| match k {
                LastKind::Screenshot => CaptureKind::Screenshot,
                LastKind::Recording => CaptureKind::Recording,
            });
            let render = move |s: &Status| match (s.last(kind), json) {
                (Some(c), true) => serde_json::to_string(c).unwrap_or_default(),
                (None, true) => "null".into(),
                (Some(c), false) => {
                    let path = c
                        .path
                        .as_ref()
                        .map(|p| p.display().to_string())
                        .unwrap_or_default();
                    format!("{}\t{}\t{path}", c.kind.as_str(), c.time)
                }
                (None, false) => String::new(),
            };
            if watch {
                return follow(render);
            }
            let status = current_status()?;
            if status.last(kind).is_none() {
                if json {
                    print("null")?;
                }
                return Ok(ExitCode::from(1));
            }
            print(render(&status))?;
            return Ok(ExitCode::SUCCESS);
        }
        Command::Query(Query::Status(args)) | Command::Status(args) => {
            return status(
                if args.json {
                    StatusFormat::Json
                } else {
                    args.format
                },
                args.watch,
            );
        }
        Command::Quit => request_running(&Request::Quit, Response::Ok)?,
        Command::Daemon => unreachable!("handled in main"),
    };
    report(response, false)
}

/// Start a recording, or act on the running one.
fn record(args: RecordArgs) -> anyhow::Result<Response> {
    let (target, options) = match args.command {
        None => (pick(), args.options),
        Some(RecordCommand::Pick(options)) => (pick(), options),
        Some(RecordCommand::Window { window, options }) => (window.target(), options),
        Some(RecordCommand::Screen { screen, options }) => (screen.target(), options),
        Some(RecordCommand::Last(options)) => (Target::LastRegion, options),
        Some(RecordCommand::Region { region, options }) => (region.target(), options),
        Some(RecordCommand::Stop) => return request_running(&Request::RecordStop, not_recording()),
        Some(RecordCommand::Pause) => {
            return request_running(&Request::RecordPause, not_recording());
        }
        Some(RecordCommand::Cancel) => {
            return request_running(&Request::RecordCancel, not_recording());
        }
    };
    request(&Request::Record(RecordRequest {
        target,
        system_audio: options.audio.then_some(true),
        microphone: options.mic.then_some(true),
        output: options.output.map(absolute),
        actions: ActionOverrides::default(),
        toggle: !options.no_toggle,
    }))
}

fn shot(args: ShotArgs) -> anyhow::Result<ExitCode> {
    let (target, args) = match args.target {
        Some(target) => target.resolve(),
        None => (pick(), args.options),
    };
    let response = request(&Request::Screenshot(ScreenshotRequest {
        target,
        delay: args.delay,
        actions: args.delivery.overrides(),
        output: args.delivery.output.map(absolute),
        want_file: args.stdout,
        cursor: args.cursor.then_some(true),
    }))?;
    report(response, args.stdout)
}

/// What the daemon itself says when asked to stop a recording that isn't there.
fn not_recording() -> Response {
    Response::error("nothing is being recorded")
}

/// The absolute path of a file the user named, which must exist.
fn existing(file: &Path) -> anyhow::Result<PathBuf> {
    std::fs::canonicalize(file).with_context(|| format!("can't open {}", file.display()))
}

fn absolute(path: PathBuf) -> PathBuf {
    if path.is_absolute() {
        path
    } else {
        std::env::current_dir()
            .map(|d| d.join(&path))
            .unwrap_or(path)
    }
}

/// Print a response the way scripts want it: paths on stdout, errors on stderr, and an
/// exit code of 0 (done), 1 (cancelled) or 2 (failed).
fn report(response: Response, to_stdout: bool) -> anyhow::Result<ExitCode> {
    match response {
        Response::Ok => Ok(ExitCode::SUCCESS),
        Response::Captured { path, temporary } => {
            match (&path, to_stdout) {
                (Some(p), true) => {
                    let bytes = std::fs::read(p)?;
                    std::io::stdout().write_all(&bytes)?;
                    if temporary {
                        let _ = std::fs::remove_file(p);
                    }
                }
                (Some(p), false) => print(p.display())?,
                (None, _) => {}
            }
            Ok(ExitCode::SUCCESS)
        }
        // Like captures: the file's path on stdout, so `f=$(screenie record)` works.
        Response::RecordingStarted { path } => {
            print(path.display())?;
            Ok(ExitCode::SUCCESS)
        }
        Response::Cancelled => Ok(ExitCode::from(1)),
        Response::Status(status) => {
            print(serde_json::to_string_pretty(&status)?)?;
            Ok(ExitCode::SUCCESS)
        }
        Response::Error { message } => {
            eprintln!("screenie: {message}");
            Ok(ExitCode::from(2))
        }
    }
}

#[derive(Clone, Copy, ValueEnum, Default, PartialEq, Eq)]
enum StatusFormat {
    /// One line of three tab-separated fields, always all present (empty when they
    /// don't apply): state, elapsed time (`m:ss`), recording path. `cut -f1` is the state.
    #[default]
    Text,
    /// The full status object.
    Json,
    /// A waybar custom module (`"return-type": "json"`): empty text when idle.
    Waybar,
}

#[derive(Subcommand)]
enum Query {
    /// What screenie is doing: idle, selecting, editing, countdown, recording, paused or
    /// saving, with the elapsed time and path while recording.
    Status(StatusArgs),
    /// The most recent capture, as three tab-separated fields: kind (`screenshot` or
    /// `recording`), time (Unix seconds) and path (empty if it was only copied). Prints
    /// nothing, and exits 1, if there's none yet.
    Last {
        /// Which kind; both by default.
        #[arg(value_enum)]
        kind: Option<LastKind>,
        /// Print JSON instead.
        #[arg(long)]
        json: bool,
        /// Keep printing a line whenever it changes (an empty line when there is none).
        #[arg(long)]
        watch: bool,
    },
}

#[derive(clap::Args)]
struct StatusArgs {
    /// Output format.
    #[arg(long, value_enum, default_value_t)]
    format: StatusFormat,
    /// Same as `--format json`.
    #[arg(long, conflicts_with = "format")]
    json: bool,
    /// Keep printing a line on every change (every second while recording). Keeps
    /// running across daemon restarts, for status bars.
    #[arg(long)]
    watch: bool,
}

#[derive(Clone, Copy, ValueEnum)]
enum LastKind {
    Screenshot,
    Recording,
}

fn status(format: StatusFormat, watch: bool) -> anyhow::Result<ExitCode> {
    let render = move |s: &Status| format_status(s, format);
    if watch {
        return follow(render);
    }
    print(render(&current_status()?))?;
    Ok(ExitCode::SUCCESS)
}

/// Write a line to stdout. Unlike `println!`, a reader that went away is an error to
/// return (see [`stdout_closed`]), not a panic.
fn print(line: impl std::fmt::Display) -> std::io::Result<()> {
    let mut out = std::io::stdout().lock();
    writeln!(out, "{line}")?;
    out.flush()
}

fn stdout_closed(e: &anyhow::Error) -> bool {
    e.downcast_ref::<std::io::Error>()
        .is_some_and(|e| e.kind() == std::io::ErrorKind::BrokenPipe)
}

/// The daemon's status. No daemon counts as idle: a bar polling this shouldn't start one.
fn current_status() -> anyhow::Result<Status> {
    match Client::connect().map(|c| c.request(&Request::Status)) {
        Ok(Ok(Response::Status(s))) => Ok(*s),
        Ok(Ok(Response::Error { message })) => anyhow::bail!(message),
        Ok(Ok(_)) => anyhow::bail!("unexpected reply to a status request"),
        Ok(Err(e)) => Err(e.into()),
        Err(_) => Ok(Status::default()),
    }
}

/// Print `render(status)` whenever it changes, forever, across daemon restarts (no
/// daemon counts as idle). Stops only when stdout goes away: a status bar starts this
/// once per session and won't restart it.
fn follow(render: impl Fn(&Status) -> String) -> anyhow::Result<ExitCode> {
    exit_when_stdout_closes();
    let mut last: Option<String> = None;
    // False once stdout is gone.
    let mut emit = |s: &Status| {
        let line = render(s);
        if last.as_ref() == Some(&line) {
            return true;
        }
        let open = print(&line).is_ok();
        last = Some(line);
        open
    };
    loop {
        let mut open = true;
        // Any failure counts as the daemon being away: none running, one quitting as we
        // connect, or a message this build can't read (from a newer daemon). Show idle
        // and try again shortly.
        let watched = Client::connect().and_then(|client| {
            client.watch(|s| {
                open = emit(&s);
                open
            })
        });
        if let Err(e) = watched {
            tracing::debug!("status watch: {e}");
        }
        if !open || !emit(&Status::default()) {
            return Ok(ExitCode::SUCCESS);
        }
        std::thread::sleep(std::time::Duration::from_secs(1));
    }
}

/// End the process once whoever reads stdout has gone. Writing would notice too, but
/// [`follow`] only writes when something changes, which may be never.
fn exit_when_stdout_closes() {
    use rustix::event::{PollFd, PollFlags, poll};
    std::thread::spawn(|| {
        let stdout = std::io::stdout();
        // Asking for no events still reports these: the reader closing a pipe, or a
        // terminal hanging up.
        let mut fds = [PollFd::new(&stdout, PollFlags::empty())];
        while poll(&mut fds, None).is_err_and(|e| e == rustix::io::Errno::INTR) {}
        if fds[0].revents().intersects(PollFlags::ERR | PollFlags::HUP) {
            std::process::exit(0);
        }
    });
}

fn format_status(s: &Status, format: StatusFormat) -> String {
    let timing = matches!(s.state, State::Recording | State::Paused);
    let elapsed = s
        .recording
        .as_ref()
        .filter(|_| timing)
        .map(|r| format_elapsed(r.elapsed_secs));
    match format {
        StatusFormat::Json => serde_json::to_string(s).unwrap_or_default(),
        StatusFormat::Text => {
            let path = s
                .recording
                .as_ref()
                .map(|r| r.path.display().to_string())
                .unwrap_or_default();
            format!(
                "{}\t{}\t{path}",
                s.state.as_str(),
                elapsed.as_deref().unwrap_or("")
            )
        }
        StatusFormat::Waybar => {
            let text = match (s.state, &elapsed) {
                (State::Recording, Some(e)) => format!("● {e}"),
                (State::Paused, Some(e)) => format!("⏸ {e}"),
                (State::Countdown, _) => "● …".to_string(),
                (State::Saving, _) => "Saving…".to_string(),
                _ => String::new(),
            };
            let tooltip = match &s.recording {
                Some(r) => format!("{} {}", s.state.as_str(), r.path.display()),
                None => s.state.as_str().to_string(),
            };
            serde_json::json!({ "text": text, "alt": s.state.as_str(), "class": s.state.as_str(), "tooltip": tooltip })
                .to_string()
        }
    }
}

fn format_elapsed(secs: f64) -> String {
    let s = secs.max(0.0) as u64;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}
