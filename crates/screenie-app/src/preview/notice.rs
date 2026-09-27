//! Captures told of in desktop notifications, where there's no layer-shell for cards
//! (GNOME): a thumbnail, what was done with it (copied, saved where), and buttons for
//! what's left to do. Clicking the notification opens the capture, as clicking a card
//! does.
//!
//! A recording is told of as soon as it's stopped ("Saving the recording…"), and the
//! notification is replaced once its file is written.

use gpui::{App, Global};
use screenie_desktop::notify::{Event, Notification, Notifications};

use super::{Action, Media, PreviewItem};

/// The largest thumbnail edge sent (notifications show them small).
const THUMB_MAX: u32 = 256;

/// The notifications shown, by their ids.
#[derive(Default)]
struct Notices {
    /// Connected on first use; `None` inside if there's no notification server.
    server: Option<Option<Notifications>>,
    shown: Vec<(u32, PreviewItem)>,
}

impl Global for Notices {}

pub(super) fn show(item: PreviewItem, cx: &mut App) {
    notify(item, None, cx);
}

pub(super) fn saved(
    id: u64,
    finished: &screenie_record::Finished,
    copied: bool,
    cx: &mut App,
) -> bool {
    let Some((notification, mut item)) = take(|item| item.id == id, cx) else {
        return false;
    };
    item.saved(finished, copied);
    notify(item, Some(notification), cx);
    true
}

pub(super) fn discard(id: u64, cx: &mut App) {
    if let Some((notification, _)) = take(|item| item.id == id, cx)
        && let Some(Some(server)) = &cx.default_global::<Notices>().server
    {
        server.close(notification);
    }
}

/// Stop keeping the item `matches` picks, and its notification's id.
fn take(matches: impl Fn(&PreviewItem) -> bool, cx: &mut App) -> Option<(u32, PreviewItem)> {
    let shown = &mut cx.default_global::<Notices>().shown;
    let i = shown.iter().position(|(_, item)| matches(item))?;
    Some(shown.remove(i))
}

/// Tell of `item`, in place of notification `replaces` if it's still up.
fn notify(item: PreviewItem, replaces: Option<u32>, cx: &mut App) {
    let notification = describe(&item, cx);
    let Some(server) = server(cx) else { return };
    match server.show(&notification, replaces) {
        Ok(id) => cx.default_global::<Notices>().shown.push((id, item)),
        Err(e) => tracing::warn!("showing a notification: {e}"),
    }
}

/// The notification server, connected (and listened to) on first use.
fn server(cx: &mut App) -> Option<&Notifications> {
    if cx.default_global::<Notices>().server.is_none() {
        let server = Notifications::connect()
            .inspect_err(|e| tracing::warn!("no notifications to tell of captures in: {e}"))
            .ok();
        if let Some(server) = &server {
            let events = server.events();
            cx.spawn(async move |cx| {
                while let Ok(event) = events.recv().await {
                    cx.update(|cx| handle(event, cx));
                }
            })
            .detach();
        }
        cx.global_mut::<Notices>().server = Some(server);
    }
    cx.global::<Notices>().server.as_ref()?.as_ref()
}

/// Do what was asked of a notification.
fn handle(event: Event, cx: &mut App) {
    let (id, key) = match event {
        Event::Action { id, key } => (id, key),
        Event::Closed { id } => {
            take_notification(id, cx);
            return;
        }
    };
    let Some(mut item) = take_notification(id, cx) else {
        return;
    };
    let action = Action::ALL.into_iter().find(|a| key == action_key(*a));
    match action {
        None if key == "default" => {
            item.open(cx);
        }
        None => {}
        Some(Action::Copy) => {
            let copying = item.copy(cx);
            cx.spawn(async move |_| {
                if let Err(e) = copying.await {
                    tracing::warn!("{e:#}");
                }
            })
            .detach();
        }
        Some(Action::Save) => {
            item.save(cx);
            // Told of again, now with where it went.
            notify(item, None, cx);
        }
        Some(Action::Reveal) => {
            item.reveal(cx);
        }
        Some(Action::Delete) => item.delete(cx),
        Some(Action::Annotate) => {
            item.edit(None, cx);
        }
        Some(Action::Dismiss) => {}
    }
}

fn take_notification(id: u32, cx: &mut App) -> Option<PreviewItem> {
    let shown = &mut cx.default_global::<Notices>().shown;
    let i = shown.iter().position(|(n, _)| *n == id)?;
    Some(shown.remove(i).1)
}

fn action_key(action: Action) -> &'static str {
    match action {
        Action::Copy => "copy",
        Action::Save => "save",
        Action::Reveal => "reveal",
        Action::Dismiss => "dismiss",
        Action::Delete => "delete",
        Action::Annotate => "annotate",
    }
}

/// What the notification for `item` says, shows and offers.
fn describe(item: &PreviewItem, cx: &App) -> Notification {
    // The folder: a banner has room for one line, and the file's name says little.
    let saved_to = item
        .path
        .as_deref()
        .and_then(|path| path.parent())
        .map(|dir| {
            let home = std::env::home_dir();
            let dir = match home.as_deref().and_then(|h| dir.strip_prefix(h).ok()) {
                Some(relative) => format!("~/{}", relative.display()),
                None => dir.display().to_string(),
            };
            format!("Saved in {dir}")
        });
    // One line: banners show no more.
    let (summary, body) = match &item.media {
        Media::Screenshot { .. } => {
            let body = match (item.is_copied(), saved_to) {
                (true, Some(saved)) => format!("Copied to the clipboard, and s{}", &saved[1..]),
                (true, None) => "Copied to the clipboard".to_string(),
                (false, Some(saved)) => saved,
                (false, None) => item.caption(),
            };
            ("Screenshot captured", body)
        }
        Media::Recording { saving: true, .. } => ("Saving the recording…", item.caption()),
        Media::Recording { .. } => (
            "Recording saved",
            [saved_to, Some(item.caption())]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>()
                .join(" · "),
        ),
    };
    // What's left to do, most useful first; notifications show three buttons at most.
    let order = [
        Action::Annotate,
        Action::Copy,
        Action::Save,
        Action::Reveal,
        Action::Delete,
    ];
    let actions = order
        .into_iter()
        .filter(|a| !item.is_saving() && a.offered(item, cx))
        .take(3)
        .map(|a| (action_key(a).to_string(), a.name().to_string()))
        .collect();
    let thumb = &item.thumb;
    let fit = (THUMB_MAX as f32 / thumb.width().max(1) as f32)
        .min(THUMB_MAX as f32 / thumb.height().max(1) as f32)
        .min(1.0);
    let (w, h) = (
        ((thumb.width() as f32 * fit) as u32).max(1),
        ((thumb.height() as f32 * fit) as u32).max(1),
    );
    let small = thumb.resize(w, h);
    Notification {
        summary: summary.to_string(),
        body,
        image: Some((w, h, small.to_rgba8())),
        actions,
        default_action: (!item.is_saving()).then(|| "default".to_string()),
    }
}
