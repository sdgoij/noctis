//! `--watch`: reload editor extensions when their files change.
//!
//! A windowed development run may pass `--watch`. The extension directories are
//! watched recursively on a `notify` background thread, which only sends paths;
//! the driver drains them once per frame and, if anything changed, re-discovers
//! and rebuilds the host (so an edited panel or verb appears without a restart).
//!
//! A rebuild is a full reload: the previous host is dropped and a fresh one
//! evaluates the extensions again. Extension state held in a JavaScript module is
//! therefore reset, exactly as a page reload resets a script — the scene edits it
//! made are already applied and live on in the editor's own undo history.

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, channel};

use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};

/// A recursive watcher over the extension directories.
pub struct ExtensionWatch {
    // Dropping the watcher stops the watch, so keep it alive for the run.
    _watcher: RecommendedWatcher,
    rx: Receiver<PathBuf>,
}

impl std::fmt::Debug for ExtensionWatch {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ExtensionWatch")
            .finish_non_exhaustive()
    }
}

impl ExtensionWatch {
    /// Watch `dirs` recursively for changes. Only existing directories are
    /// watched; a missing one is skipped (nothing can change in it yet).
    pub(crate) fn new(dirs: &[PathBuf]) -> notify::Result<Self> {
        let (tx, rx) = channel();
        let mut watcher = notify::recommended_watcher(move |event: notify::Result<Event>| {
            if let Ok(event) = event
                && is_relevant(&event.kind)
            {
                for path in event.paths {
                    // A send error just means the driver has gone away.
                    let _ = tx.send(path);
                }
            }
        })?;
        for dir in dirs {
            if dir.is_dir() {
                watcher.watch(dir, RecursiveMode::Recursive)?;
            }
        }
        Ok(Self {
            _watcher: watcher,
            rx,
        })
    }

    /// Whether any change was seen since the last call (and drain them).
    pub(crate) fn changed(&self) -> bool {
        let mut changed = false;
        while self.rx.try_recv().is_ok() {
            changed = true;
        }
        changed
    }
}

/// What a windowed run needs to rebuild the host when a file changes.
#[derive(Debug)]
pub struct ExtensionReload {
    /// The filesystem watcher.
    pub(crate) watch: ExtensionWatch,
    /// The directories the host was discovered from.
    pub(crate) dirs: Vec<PathBuf>,
    /// Project-level disabled ids (the user config is re-read from the driver).
    pub(crate) disabled: Vec<String>,
}

/// `true` for the event kinds that mean a file may have changed.
fn is_relevant(kind: &EventKind) -> bool {
    matches!(
        kind,
        EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creates_modifies_and_removes_are_relevant() {
        assert!(is_relevant(&EventKind::Create(
            notify::event::CreateKind::File
        )));
        assert!(is_relevant(&EventKind::Modify(
            notify::event::ModifyKind::Any
        )));
        assert!(is_relevant(&EventKind::Remove(
            notify::event::RemoveKind::File
        )));
        assert!(!is_relevant(&EventKind::Access(
            notify::event::AccessKind::Any
        )));
    }
}
