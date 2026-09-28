//! Read a video node's first picture from the session's PipeWire daemon, as a still is
//! read from a cast: `cargo run -p screenie-pipewire --example first_picture -- NODE`.
//! A test node: `gst-launch-1.0 videotestsrc ! pipewiresink mode=provide`.

use std::process::ExitCode;
use std::time::Duration;

use screenie_pipewire::{Keep, Pointer, Remote, Stream};

fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();
    let Some(node) = std::env::args().nth(1).and_then(|a| a.parse().ok()) else {
        eprintln!("usage: first_picture NODE");
        return ExitCode::from(2);
    };
    let picture = Stream::connect(Remote::Session, node, Pointer::InFrames, Keep::All)
        .and_then(|mut stream| stream.first_picture(Duration::from_secs(5)));
    match picture {
        Ok(image) => {
            println!("{}x{}", image.width(), image.height());
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}
