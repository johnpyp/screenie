//! Run the selector once over a fresh snapshot and print the choice.
//! `cargo run -p screenie-selector --example select -- [record] [adjust]`
use std::sync::Arc;

use screenie_capture::{CaptureContext, SnapshotOptions};
use screenie_selector::{Backdrop, Purpose, SelectorConfig, select};

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();
    let args: Vec<String> = std::env::args().collect();
    let record = args.iter().any(|a| a == "record");
    let adjust = args.iter().any(|a| a == "adjust");

    let ctx = CaptureContext::new();
    let t = std::time::Instant::now();
    let snapshot = ctx
        .snapshot(SnapshotOptions {
            cursor: false,
            backend: screenie_config::CaptureBackend::Auto,
            windows: true,
        })
        .expect("snapshot");
    println!("snapshot in {:?}", t.elapsed());

    gpui_kit::platform::application()
        .with_assets(screenie_ui_kit::Assets)
        .run(move |cx| {
            screenie_ui_kit::init(cx);
            let backdrop = if record {
                Backdrop::Live {
                    outputs: snapshot.outputs.iter().map(|o| o.output.clone()).collect(),
                    windows: snapshot.windows.clone(),
                }
            } else {
                Backdrop::Frozen(Arc::new(snapshot))
            };
            let config = SelectorConfig {
                purpose: if record {
                    Purpose::Recording
                } else {
                    Purpose::Screenshot
                },
                capture_on_release: !adjust,
                ..Default::default()
            };
            cx.spawn(async move |cx| {
                let choice = select(cx, backdrop, config).await;
                println!("CHOICE: {choice:?}");
                cx.update(|cx| cx.quit());
            })
            .detach();
        });
}
