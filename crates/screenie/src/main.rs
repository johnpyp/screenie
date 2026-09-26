//! `screenie`: screenshots and screen recordings for Wayland.
//!
//! Every command talks to the resident daemon, starting it if needed. Bind the commands
//! you like to keys in your compositor, e.g. for Hyprland:
//!
//! ```text
//! bind = , Print, exec, screenie
//! bind = SHIFT, Print, exec, screenie shot screen
//! bind = ALT, Print, exec, screenie record
//! ```

use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand, ValueEnum};
use screenie_core::Rect;
use screenie_ipc::{
    ActionOverrides, CaptureKind, Client, RecordRequest, Request, Response, ScreenshotRequest, SelectMode, State, Status, Target,
};

/// `0.1.0 (46bce20 2026-09-25 23:20)`
const VERSION: &str = concat!(env!("CARGO_PKG_VERSION"), " (", env!("SCREENIE_COMMIT"), ")");

#[derive(Parser)]
#[command(name = "screenie", version = VERSION, about = "Screenshots and screen recordings for Wayland")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Take a screenshot (the default command).
    #[command(visible_alias = "screenshot")]
    Shot(ShotArgs),
    /// Start a screen recording, or stop the running one.
    Record(RecordArgs),
    /// Stop the running recording and save it.
    Stop,
    /// Pause or resume the running recording.
    Pause,
    /// Stop the running recording and discard it.
    Cancel,
    /// Open the settings window.
    Settings,
    /// Open an image in the annotation editor.
    Edit { file: PathBuf },
    /// Pin an image to the screen.
    Pin { file: PathBuf },
    /// Ask what screenie is doing or has done, for scripts and status bars. Never
    /// starts the daemon: if it isn't running, screenie is idle.
    #[command(subcommand)]
    Query(Query),
    /// Run the daemon in the foreground (normally started automatically). Upgrades a
    /// daemon running another build, unless it is in use.
    Daemon,
    /// Stop the daemon.
    Quit,
}

#[derive(Clone, Copy, ValueEnum, Default)]
enum ShotTarget {
    /// Drag an area, click a window, or press Enter to capture the whole screen.
    #[default]
    Area,
    /// Pick a window.
    Window,
    /// Pick a screen interactively.
    PickScreen,
    /// The focused screen, immediately.
    Screen,
    /// All screens as one image, immediately.
    All,
    /// The focused window, immediately.
    Active,
    /// The same region as last time, immediately.
    Last,
}

#[derive(Args)]
struct Delivery {
    /// Copy to the clipboard (overrides settings).
    #[arg(long, overrides_with = "no_copy")]
    copy: bool,
    #[arg(long, hide = true)]
    no_copy: bool,
    /// Save to the screenshots folder (overrides settings).
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
    /// Save to this path.
    #[arg(short, long)]
    output: Option<PathBuf>,
}

impl Delivery {
    fn overrides(&self) -> ActionOverrides {
        let flag = |yes: bool, no: bool| if yes { Some(true) } else if no { Some(false) } else { None };
        ActionOverrides {
            copy: flag(self.copy, self.no_copy),
            save: flag(self.save, self.no_save),
            preview: self.no_preview.then_some(false),
            edit: self.edit.then_some(true),
        }
    }
}

#[derive(Args)]
struct ShotArgs {
    /// What to capture.
    #[arg(value_enum, default_value_t)]
    target: ShotTarget,
    /// Capture this region instead: "X,Y WxH" or "WxH+X+Y" in logical coordinates.
    #[arg(short, long, value_parser = parse_rect, conflicts_with = "target")]
    region: Option<Rect>,
    /// Capture this output (connector name, e.g. DP-1).
    #[arg(long)]
    output_name: Option<String>,
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

#[derive(Clone, Copy, ValueEnum, Default)]
enum RecordTarget {
    /// Pick an area, window or screen, then press Record.
    #[default]
    Area,
    Window,
    /// The focused screen, immediately.
    Screen,
    /// The same region as last time, immediately.
    Last,
}

#[derive(Args)]
struct RecordArgs {
    #[arg(value_enum, default_value_t)]
    target: RecordTarget,
    /// Record this region instead: "X,Y WxH" or "WxH+X+Y" in logical coordinates.
    #[arg(short, long, value_parser = parse_rect, conflicts_with = "target")]
    region: Option<Rect>,
    /// Record system audio.
    #[arg(long)]
    audio: bool,
    /// Record the microphone.
    #[arg(long)]
    mic: bool,
    /// Save to this path.
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
    let cli = Cli::parse();
    let command = cli.command.unwrap_or(Command::Shot(ShotArgs {
        target: ShotTarget::Area,
        region: None,
        output_name: None,
        delay: 0,
        cursor: false,
        stdout: false,
        delivery: Delivery {
            copy: false,
            no_copy: false,
            save: false,
            no_save: false,
            no_preview: false,
            edit: false,
            output: None,
        },
    }));

    if let Command::Daemon = command {
        let commit = env!("SCREENIE_COMMIT");
        let describe = |s: &screenie_ipc::Status| {
            let build = if s.commit.is_empty() { "an older build".to_string() } else { s.commit.clone() };
            format!("{build}, pid {}", s.pid)
        };
        match Client::take_over() {
            Ok(screenie_ipc::Takeover::NotRunning) => {}
            Ok(screenie_ipc::Takeover::Replaced(old)) if old.commit == commit => {
                eprintln!("screenie: replaced the running daemon (pid {}), a different binary of this commit", old.pid);
            }
            Ok(screenie_ipc::Takeover::Replaced(old)) => {
                eprintln!("screenie: upgraded the running daemon ({}) to this build ({commit})", describe(&old));
            }
            Ok(screenie_ipc::Takeover::Current(s)) => {
                eprintln!("screenie: the daemon is already running this build ({})", describe(&s));
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
        init_logging(true);
        return match screenie_app::run(commit) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("screenie daemon: {e:#}");
                ExitCode::from(2)
            }
        };
    }
    init_logging(false);

    match run(command) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("screenie: {e:#}");
            ExitCode::from(2)
        }
    }
}

fn init_logging(daemon: bool) {
    let default = if daemon { "info" } else { "warn" };
    let filter = tracing_subscriber::EnvFilter::try_from_env("SCREENIE_LOG").unwrap_or_else(|_| default.into());
    tracing_subscriber::fmt().with_env_filter(filter).with_writer(std::io::stderr).init();
}

fn request(req: &Request) -> anyhow::Result<Response> {
    Ok(Client::connect_or_spawn()?.request(req)?)
}

fn run(command: Command) -> anyhow::Result<ExitCode> {
    let response = match command {
        Command::Shot(args) => return shot(args),
        Command::Record(args) => {
            let target = match (args.region, args.target) {
                (Some(rect), _) => Target::Region { rect },
                (None, RecordTarget::Area) => Target::Select { mode: SelectMode::Area },
                (None, RecordTarget::Window) => Target::Select { mode: SelectMode::Window },
                (None, RecordTarget::Screen) => Target::Screen { output: None },
                (None, RecordTarget::Last) => Target::LastRegion,
            };
            request(&Request::Record(RecordRequest {
                target,
                system_audio: args.audio.then_some(true),
                microphone: args.mic.then_some(true),
                output: args.output.map(absolute),
                actions: ActionOverrides::default(),
                toggle: !args.no_toggle,
            }))?
        }
        Command::Stop => request(&Request::RecordStop)?,
        Command::Pause => request(&Request::RecordPause)?,
        Command::Cancel => request(&Request::RecordCancel)?,
        Command::Settings => request(&Request::Settings)?,
        Command::Edit { file } => request(&Request::Edit { path: std::fs::canonicalize(file)? })?,
        Command::Pin { file } => request(&Request::Pin { path: std::fs::canonicalize(file)? })?,
        Command::Query(Query::Last { kind, json, watch }) => {
            let kind = kind.map(|k| match k {
                LastKind::Screenshot => CaptureKind::Screenshot,
                LastKind::Recording => CaptureKind::Recording,
            });
            let render = move |s: &Status| match (s.last(kind), json) {
                (Some(c), true) => serde_json::to_string(c).unwrap_or_default(),
                (None, true) => "null".into(),
                (Some(c), false) => {
                    let path = c.path.as_ref().map(|p| p.display().to_string()).unwrap_or_default();
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
                    println!("null");
                }
                return Ok(ExitCode::from(1));
            }
            println!("{}", render(&status));
            return Ok(ExitCode::SUCCESS);
        }
        Command::Query(Query::Status { format, json, watch }) => {
            return status(if json { StatusFormat::Json } else { format }, watch);
        }
        Command::Quit => match Client::connect() {
            Ok(client) => client.request(&Request::Quit)?,
            Err(_) => Response::Ok, // not running
        },
        Command::Daemon => unreachable!("handled in main"),
    };
    report(response, false)
}

fn shot(args: ShotArgs) -> anyhow::Result<ExitCode> {
    let target = match (args.region, args.output_name) {
        (Some(rect), _) => Target::Region { rect },
        (None, Some(name)) => Target::Screen { output: Some(name) },
        (None, None) => match args.target {
            ShotTarget::Area => Target::Select { mode: SelectMode::Area },
            ShotTarget::Window => Target::Select { mode: SelectMode::Window },
            ShotTarget::PickScreen => Target::Select { mode: SelectMode::Screen },
            ShotTarget::Screen => Target::Screen { output: None },
            ShotTarget::All => Target::AllScreens,
            ShotTarget::Active => Target::ActiveWindow,
            ShotTarget::Last => Target::LastRegion,
        },
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

fn absolute(path: PathBuf) -> PathBuf {
    if path.is_absolute() { path } else { std::env::current_dir().map(|d| d.join(&path)).unwrap_or(path) }
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
                (Some(p), false) => println!("{}", p.display()),
                (None, _) => {}
            }
            Ok(ExitCode::SUCCESS)
        }
        // Like captures: the file's path on stdout, so `f=$(screenie record)` works.
        Response::RecordingStarted { path } => {
            println!("{}", path.display());
            Ok(ExitCode::SUCCESS)
        }
        Response::Cancelled => Ok(ExitCode::from(1)),
        Response::Status(status) => {
            println!("{}", serde_json::to_string_pretty(&status)?);
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
    Status {
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
    },
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
    println!("{}", render(&current_status()?));
    Ok(ExitCode::SUCCESS)
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
/// daemon counts as idle). Stops when stdout goes away.
fn follow(render: impl Fn(&Status) -> String) -> anyhow::Result<ExitCode> {
    let mut last: Option<String> = None;
    let mut emit = |s: &Status| {
        let line = render(s);
        if last.as_ref() != Some(&line) {
            println!("{line}");
            last = Some(line);
        }
        std::io::stdout().flush().is_ok()
    };
    loop {
        if let Ok(client) = Client::connect() {
            let mut open = true;
            client.watch(|s| {
                open = emit(&s);
                open
            })?;
            if !open {
                return Ok(ExitCode::SUCCESS);
            }
        }
        if !emit(&Status::default()) {
            return Ok(ExitCode::SUCCESS);
        }
        std::thread::sleep(std::time::Duration::from_secs(1));
    }
}

fn format_status(s: &Status, format: StatusFormat) -> String {
    let timing = matches!(s.state, State::Recording | State::Paused);
    let elapsed = s.recording.as_ref().filter(|_| timing).map(|r| format_elapsed(r.elapsed_secs));
    match format {
        StatusFormat::Json => serde_json::to_string(s).unwrap_or_default(),
        StatusFormat::Text => {
            let path = s.recording.as_ref().map(|r| r.path.display().to_string()).unwrap_or_default();
            format!("{}\t{}\t{path}", s.state.as_str(), elapsed.as_deref().unwrap_or(""))
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
    if s >= 3600 { format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60) } else { format!("{}:{:02}", s / 60, s % 60) }
}
