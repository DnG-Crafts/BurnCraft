//! Auto-save and crash recovery.
//!
//! While there are unsaved changes, the design is copied to a recovery file in the app's data
//! folder every minute (on a background thread). A clean exit deletes it; if BurnCraft finds one
//! at startup, the last session didn't close properly and the design can be restored.

use bc_doc::Document;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// How often unsaved changes are backed up.
const INTERVAL: Duration = Duration::from_secs(60);

/// What the recovery file was a copy of.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RecoveryInfo {
    /// The project it belongs to, if it had been saved before.
    pub file: Option<PathBuf>,
    /// When it was written (seconds since 1970).
    pub saved_at: u64,
}

impl RecoveryInfo {
    /// "3 minutes ago"-style age.
    pub fn age(&self) -> String {
        let now = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(self.saved_at);
        let mins = now.saturating_sub(self.saved_at) / 60;
        match mins {
            0 => "less than a minute ago".into(),
            1 => "1 minute ago".into(),
            m if m < 120 => format!("{m} minutes ago"),
            m if m < 48 * 60 => format!("{} hours ago", m / 60),
            m => format!("{} days ago", m / (24 * 60)),
        }
    }
}

pub struct AutoSaver {
    dir: Option<PathBuf>,
    /// Revision last backed up, and when.
    saved_revision: Option<u64>,
    last: Option<std::time::Instant>,
    worker: Option<std::thread::JoinHandle<()>>,
}

impl AutoSaver {
    pub fn new() -> Self {
        Self::in_dir(eframe::storage_dir("BurnCraft").map(|d| d.join("recovery")))
    }

    pub fn in_dir(dir: Option<PathBuf>) -> Self {
        Self { dir, saved_revision: None, last: None, worker: None }
    }

    fn files(&self) -> Option<(PathBuf, PathBuf)> {
        let d = self.dir.as_ref()?;
        Some((d.join("autosave.bcproj"), d.join("autosave.json")))
    }

    /// A recovery file left behind by a session that didn't close properly.
    pub fn pending(&self) -> Option<(PathBuf, RecoveryInfo)> {
        let (project, info) = self.files()?;
        let info: RecoveryInfo = serde_json::from_slice(&std::fs::read(info).ok()?).ok()?;
        project.exists().then_some((project, info))
    }

    /// Backs up `doc` if it has unsaved changes that haven't been backed up, at most once a minute.
    /// `dirty` means the document differs from its saved file (or was never saved).
    pub fn tick(&mut self, doc: &Document, revision: u64, dirty: bool, file: Option<&Path>) {
        if !dirty || doc.shapes.is_empty() || self.saved_revision == Some(revision) {
            return;
        }
        if self.last.is_some_and(|t| t.elapsed() < INTERVAL) || self.worker.as_ref().is_some_and(|w| !w.is_finished()) {
            return;
        }
        self.save_now(doc, revision, file);
    }

    /// Writes the backup now (in the background).
    pub fn save_now(&mut self, doc: &Document, revision: u64, file: Option<&Path>) {
        let Some((project, info_path)) = self.files() else { return };
        // Cheap: image data is shared, so this copies only the shape list.
        let doc = doc.clone();
        let info = RecoveryInfo { file: file.map(Path::to_path_buf), saved_at: SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0) };
        self.saved_revision = Some(revision);
        self.last = Some(std::time::Instant::now());
        self.worker = std::thread::Builder::new()
            .name("autosave".into())
            .spawn(move || {
                if let Some(dir) = project.parent() {
                    let _ = std::fs::create_dir_all(dir);
                }
                if doc.save(&project).is_ok() {
                    let _ = std::fs::write(info_path, serde_json::to_vec(&info).unwrap_or_default());
                }
            })
            .ok();
    }

    /// Removes the backup (after a clean exit, or once it has been restored or declined).
    pub fn clear(&mut self) {
        if let Some(w) = self.worker.take() {
            let _ = w.join();
        }
        if let Some((project, info)) = self.files() {
            let _ = std::fs::remove_file(project);
            let _ = std::fs::remove_file(info);
        }
        self.saved_revision = None;
    }

    /// Waits for a backup in progress (for tests).
    #[cfg(test)]
    fn wait(&mut self) {
        if let Some(w) = self.worker.take() {
            w.join().unwrap();
        }
    }
}

/// Most recent files kept in File > Open Recent.
pub const RECENT_LIMIT: usize = 10;

/// Puts `path` at the front of the recent list.
pub fn remember(recent: &mut Vec<PathBuf>, path: &Path) {
    recent.retain(|p| p != path);
    recent.insert(0, path.to_path_buf());
    recent.truncate(RECENT_LIMIT);
}

#[cfg(test)]
mod tests {
    use super::*;
    use bc_doc::ShapeKind;
    use bc_geom::Affine;

    fn temp_dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("burncraft-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn backs_up_changes_and_recovers_them() {
        let dir = temp_dir("autosave");
        let mut saver = AutoSaver::in_dir(Some(dir.clone()));
        let mut doc = Document::default();
        // Nothing to back up: clean, or empty.
        saver.tick(&doc, 1, true, None);
        assert!(saver.pending().is_none());
        doc.add_shape(ShapeKind::Ellipse { radius_x: 5.0, radius_y: 5.0 }, Affine::IDENTITY, 0);
        saver.tick(&doc, 2, false, None);
        assert!(saver.pending().is_none(), "saved designs aren't backed up");
        // Unsaved changes are.
        saver.tick(&doc, 2, true, Some(Path::new("C:/designs/coaster.bcproj")));
        saver.wait();
        let (file, info) = saver.pending().expect("a recovery file");
        assert_eq!(info.file.as_deref(), Some(Path::new("C:/designs/coaster.bcproj")));
        assert_eq!(Document::load(&file).unwrap().shapes.len(), 1);
        assert_eq!(info.age(), "less than a minute ago");
        // A clean exit removes it.
        saver.clear();
        assert!(saver.pending().is_none());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn recent_files_are_unique_newest_first_and_capped() {
        let mut recent = Vec::new();
        for i in 0..15 {
            remember(&mut recent, Path::new(&format!("{i}.bcproj")));
        }
        remember(&mut recent, Path::new("10.bcproj"));
        assert_eq!(recent.len(), RECENT_LIMIT);
        assert_eq!(recent[0], Path::new("10.bcproj"));
        assert_eq!(recent.iter().filter(|p| *p == Path::new("10.bcproj")).count(), 1);
    }
}

#[cfg(test)]
mod where_tests {
    #[test]
    #[ignore = "diagnostic"]
    fn print_recovery_dir() {
        println!("RECOVERY_DIR={}", eframe::storage_dir("BurnCraft").unwrap().join("recovery").display());
    }
}
#[cfg(test)]
mod plant_tests {
    use super::*;

    /// Leaves a backup in the real recovery folder, as a crash would (for checking the restore prompt).
    #[test]
    #[ignore = "diagnostic"]
    fn plant_backup() {
        let mut saver = AutoSaver::new();
        let mut doc = Document::default();
        doc.add_shape(bc_doc::ShapeKind::Ellipse { radius_x: 45.0, radius_y: 45.0 }, bc_geom::Affine::translate((150.0, 150.0)), 0);
        saver.save_now(&doc, 1, Some(Path::new("C:/designs/coaster.bcproj")));
        saver.wait();
        println!("planted: {:?}", saver.pending().map(|(p, i)| (p, i.age())));
    }
}