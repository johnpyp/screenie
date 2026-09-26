//! The daemon's socket server. Connections are handled on plain threads (each is one
//! short request, or a long-lived status watch) and forwarded to the UI thread.

use std::io::BufReader;
use std::os::unix::net::{UnixListener, UnixStream};

use screenie_ipc::{Request, Response, Status, read_message, write_message};

pub(crate) enum Incoming {
    Request {
        request: Request,
        reply: async_channel::Sender<Response>,
    },
    Watch {
        updates: async_channel::Sender<Status>,
    },
}

/// Accept connections forever, forwarding each to the returned channel.
pub(crate) fn start(listener: UnixListener) -> async_channel::Receiver<Incoming> {
    let (tx, rx) = async_channel::unbounded();
    std::thread::Builder::new()
        .name("ipc-accept".into())
        .spawn(move || {
            for stream in listener.incoming() {
                match stream {
                    Ok(stream) => {
                        let tx = tx.clone();
                        let _ =
                            std::thread::Builder::new()
                                .name("ipc-conn".into())
                                .spawn(move || {
                                    if let Err(e) = serve(stream, &tx) {
                                        tracing::debug!("ipc connection ended: {e}");
                                    }
                                });
                    }
                    Err(e) => tracing::warn!("accept failed: {e}"),
                }
            }
        })
        .expect("spawn accept thread");
    rx
}

fn serve(stream: UnixStream, tx: &async_channel::Sender<Incoming>) -> anyhow::Result<()> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let Some(request) = read_message::<Request>(&mut reader)? else {
        return Ok(());
    };
    tracing::debug!(?request, "request");
    if request == Request::Watch {
        let (updates, rx) = async_channel::bounded(16);
        tx.send_blocking(Incoming::Watch { updates })?;
        while let Ok(status) = rx.recv_blocking() {
            write_message(&stream, &Response::Status(Box::new(status)))?;
        }
        return Ok(());
    }
    let (reply, rx) = async_channel::bounded(1);
    tx.send_blocking(Incoming::Request { request, reply })?;
    let response = rx
        .recv_blocking()
        .unwrap_or_else(|_| Response::error("the daemon dropped the request"));
    write_message(&stream, &response)?;
    Ok(())
}
