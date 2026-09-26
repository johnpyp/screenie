//! The latest capture of each kind, for `screenie query last` and status watchers.
//!
//! A capture can get its file after it's noted (Save on its preview card) or lose it
//! (Delete), so each one noted gets an id to find its entry again.

use std::path::{Path, PathBuf};

use screenie_ipc::{CaptureKind, LastCapture};

/// A capture noted with [`LastCaptures::note`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CaptureId(u64);

#[derive(Default)]
pub(crate) struct LastCaptures {
    screenshot: Option<(CaptureId, LastCapture)>,
    recording: Option<(CaptureId, LastCapture)>,
    noted: u64,
}

impl LastCaptures {
    pub fn get(&self, kind: CaptureKind) -> Option<&LastCapture> {
        let last = match kind {
            CaptureKind::Screenshot => &self.screenshot,
            CaptureKind::Recording => &self.recording,
        };
        last.as_ref().map(|(_, capture)| capture)
    }

    /// A capture was taken at `time` (Unix seconds), saved at `path` if it was.
    pub fn note(&mut self, kind: CaptureKind, path: Option<PathBuf>, time: u64) -> CaptureId {
        self.noted += 1;
        let id = CaptureId(self.noted);
        *self.last_mut(kind) = Some((id, LastCapture { kind, path, time }));
        id
    }

    /// Capture `id` was saved to `path` later, at `time`. Its entry gets the path if
    /// it's still the last of its kind. If not, it's the newest file, and becomes the
    /// last one (as a save from the editor does).
    pub fn saved(&mut self, id: CaptureId, kind: CaptureKind, path: PathBuf, time: u64) {
        match self.last_mut(kind) {
            Some((last, capture)) if *last == id => capture.path = Some(path),
            _ => {
                self.note(kind, Some(path), time);
            }
        }
    }

    /// The file at `path` was deleted: nothing points at it any more. Whether anything
    /// did.
    pub fn deleted(&mut self, path: &Path) -> bool {
        let mut changed = false;
        for (_, capture) in [&mut self.screenshot, &mut self.recording]
            .into_iter()
            .flatten()
        {
            if capture.path.as_deref() == Some(path) {
                capture.path = None;
                changed = true;
            }
        }
        changed
    }

    /// The captures remembered from an earlier run.
    pub fn restore(remembered: &screenie_state::LastState) -> Self {
        let mut last = Self::default();
        let kinds = [
            (CaptureKind::Screenshot, &remembered.screenshot),
            (CaptureKind::Recording, &remembered.recording),
        ];
        for (kind, capture) in kinds {
            if let Some(c) = capture {
                last.note(kind, c.path.clone(), c.time);
            }
        }
        last
    }

    /// Write the captures into `remembered`, for the next run.
    pub fn persist(&self, remembered: &mut screenie_state::LastState) {
        let capture = |kind| {
            self.get(kind).map(|c| screenie_state::Capture {
                path: c.path.clone(),
                time: c.time,
            })
        };
        remembered.screenshot = capture(CaptureKind::Screenshot);
        remembered.recording = capture(CaptureKind::Recording);
    }

    fn last_mut(&mut self, kind: CaptureKind) -> &mut Option<(CaptureId, LastCapture)> {
        match kind {
            CaptureKind::Screenshot => &mut self.screenshot,
            CaptureKind::Recording => &mut self.recording,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHOT: CaptureKind = CaptureKind::Screenshot;

    fn path(last: &LastCaptures, kind: CaptureKind) -> Option<&Path> {
        last.get(kind)?.path.as_deref()
    }

    #[test]
    fn saving_from_the_card_fills_in_its_entry() {
        let mut last = LastCaptures::default();
        let id = last.note(SHOT, None, 10);
        last.saved(id, SHOT, "/a.png".into(), 20);
        let entry = last.get(SHOT).unwrap();
        assert_eq!(entry.path.as_deref(), Some(Path::new("/a.png")));
        assert_eq!(entry.time, 10, "still when it was taken");
    }

    #[test]
    fn saving_an_older_capture_makes_it_the_last() {
        let mut last = LastCaptures::default();
        let older = last.note(SHOT, None, 10);
        last.note(SHOT, None, 20);
        last.saved(older, SHOT, "/older.png".into(), 30);
        let entry = last.get(SHOT).unwrap();
        assert_eq!(entry.path.as_deref(), Some(Path::new("/older.png")));
        assert_eq!(entry.time, 30);
    }

    #[test]
    fn deleting_the_file_clears_only_entries_pointing_at_it() {
        let mut last = LastCaptures::default();
        last.note(SHOT, Some("/a.png".into()), 10);
        last.note(CaptureKind::Recording, Some("/b.mp4".into()), 10);
        assert!(!last.deleted(Path::new("/c.png")));
        assert!(last.deleted(Path::new("/a.png")));
        assert_eq!(path(&last, SHOT), None);
        assert!(
            last.get(SHOT).is_some(),
            "the capture itself is still the last"
        );
        assert_eq!(
            path(&last, CaptureKind::Recording),
            Some(Path::new("/b.mp4"))
        );
    }

    #[test]
    fn what_is_persisted_is_restored() {
        let mut last = LastCaptures::default();
        last.note(SHOT, Some("/a.png".into()), 10);
        last.note(CaptureKind::Recording, None, 20);
        let mut remembered = screenie_state::LastState::default();
        last.persist(&mut remembered);
        let restored = LastCaptures::restore(&remembered);
        assert_eq!(path(&restored, SHOT), Some(Path::new("/a.png")));
        assert_eq!(restored.get(SHOT).unwrap().time, 10);
        assert_eq!(restored.get(CaptureKind::Recording).unwrap().time, 20);
    }
}
