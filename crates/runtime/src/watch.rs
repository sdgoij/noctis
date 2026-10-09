//! Development hot reload: watch the project for script edits.
//!
//! A windowed development run loads scripts from disk (see `docs/scripts.md`).
//! [`ScriptWatch`] wraps a `notify` watcher on the project root and collects the
//! `.js`/`.mjs`/`.wasm` files that changed; [`Runtime`](crate::Runtime) re-reads
//! them into the asset registry, and the script runtime reloads them because its
//! cached module is keyed by source.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, channel};

use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};

/// A recursive filesystem watcher that collects the script files that changed.
///
/// The watcher runs on a `notify` background thread and only sends paths; the
/// runtime reads the files on its own thread, so nothing crosses threads except
/// `PathBuf`s.
pub(crate) struct ScriptWatch {
    root: PathBuf,
    // Dropping the watcher stops the watch, so keep it alive for the run.
    _watcher: RecommendedWatcher,
    rx: Receiver<PathBuf>,
}

impl ScriptWatch {
    /// Watch `root` recursively for `.js`/`.mjs`/`.wasm` changes.
    pub(crate) fn new(root: &Path) -> notify::Result<Self> {
        let (tx, rx) = channel();
        let mut watcher = notify::recommended_watcher(move |event: notify::Result<Event>| {
            if let Ok(event) = event
                && is_relevant(&event.kind)
            {
                for path in event.paths {
                    if is_script(&path) {
                        // A send error just means the runtime has gone away.
                        let _ = tx.send(path);
                    }
                }
            }
        })?;
        watcher.watch(root, RecursiveMode::Recursive)?;
        Ok(Self {
            root: root.to_path_buf(),
            _watcher: watcher,
            rx,
        })
    }

    /// The watched root directory.
    pub(crate) fn root(&self) -> &Path {
        &self.root
    }

    /// The deduplicated changed script paths seen since the last call.
    pub(crate) fn drain(&self) -> Vec<PathBuf> {
        let mut changed: Vec<PathBuf> = Vec::new();
        while let Ok(path) = self.rx.try_recv() {
            if !changed.contains(&path) {
                changed.push(path);
            }
        }
        changed
    }
}

/// `true` for the event kinds that mean a file's contents may have changed.
fn is_relevant(kind: &EventKind) -> bool {
    matches!(
        kind,
        EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
    )
}

/// `true` if `path` names a script the runtime can reload.
fn is_script(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|extension| extension.to_str())
            .map(str::to_ascii_lowercase)
            .as_deref(),
        Some("js" | "mjs" | "wasm")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_script_extensions_are_scripts() {
        assert!(is_script(Path::new("scripts/a.js")));
        assert!(is_script(Path::new("scripts/a.mjs")));
        assert!(is_script(Path::new("scripts/a.WASM")));
        assert!(!is_script(Path::new("scripts/a.txt")));
        assert!(!is_script(Path::new("scripts/a.js.bak")));
        assert!(!is_script(Path::new("scripts/noext")));
    }

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
