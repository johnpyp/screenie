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
use screenie_ipc::{ActionOverrides, Client, RecordRequest, Request, Response, ScreenshotRequest, SelectMode, Target};

#[derive(Parser)]
#[command(name = "screenie", version, about = "Screenshots and screen recordings for Wayland")]
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
    /// Show daemon and recording status.
    Status {
        /// Print JSON (one object per line with --watch).
        #[arg(long)]
        json: bool,
        /// Keep printing status on every change (for status bars).
        #[arg(long)]
        watch: bool,
    },
    /// Run the daemon in the foreground (normally started automatically).
    Daemon,
    /// Stop the daemon.
    Quit,
}

#[derive(Clone, Copy, ValueEnum, Default)]
enum ShotTarget {
    /// Drag a region, click a window, or press Enter for the screen.
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
        init_logging(true);
        return match screenie_app::run() {
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
        Command::Status { json, watch } => return status(json, watch),
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
        Response::RecordingStarted { path } => {
            eprintln!("recording to {}", path.display());
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

fn status(json: bool, watch: bool) -> anyhow::Result<ExitCode> {
    let print = |s: &screenie_ipc::Status| {
        if json {
            println!("{}", serde_json::to_string(s).unwrap_or_default());
        } else {
            match &s.recording {
                Some(r) => println!(
                    "recording {} ({:.0}s{})",
                    r.path.display(),
                    r.elapsed_secs,
                    if r.paused { ", paused" } else { "" }
                ),
                None => println!("idle (daemon pid {}, {} via {})", s.pid, s.compositor, s.capture_backend),
            }
        }
    };
    if watch {
        Client::connect_or_spawn()?.watch(|s| {
            print(&s);
            std::io::stdout().flush().is_ok()
        })?;
        return Ok(ExitCode::SUCCESS);
    }
    match request(&Request::Status)? {
        Response::Status(s) => {
            print(&s);
            Ok(ExitCode::SUCCESS)
        }
        other => report(other, false),
    }
}
