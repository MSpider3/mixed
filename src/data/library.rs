use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use crate::data::metadata::read_metadata;

/// Supported audio file extensions.
const AUDIO_EXTENSIONS: &[&str] = &[
    "mp3", "flac", "ogg", "opus", "wav", "m4a", "aac", "wma", "webm",
];

/// A single entry in the music library tree.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum LibraryEntry {
    Directory {
        name: String,
        path: PathBuf,
        children: Vec<LibraryEntry>,
    },
    Track {
        name: String,
        path: PathBuf,
        metadata: crate::data::metadata::TrackMetadata,
    },
}

impl LibraryEntry {
    /// Returns the display name.
    pub fn name(&self) -> &str {
        match self {
            LibraryEntry::Directory { name, .. } => name,
            LibraryEntry::Track { name, .. } => name,
        }
    }

    /// Returns the path.
    #[allow(dead_code)]
    pub fn path(&self) -> &Path {
        match self {
            LibraryEntry::Directory { path, .. } => path,
            LibraryEntry::Track { path, .. } => path,
        }
    }

    /// Returns true if this is a directory.
    pub fn is_dir(&self) -> bool {
        matches!(self, LibraryEntry::Directory { .. })
    }

    /// Returns all audio file paths under this entry recursively.
    pub fn get_all_files(&self) -> Vec<PathBuf> {
        let mut paths = Vec::new();
        self.collect_files_recursive(&mut paths);
        paths
    }

    fn collect_files_recursive(&self, paths: &mut Vec<PathBuf>) {
        match self {
            LibraryEntry::Track { path, .. } => {
                paths.push(path.clone());
            }
            LibraryEntry::Directory { children, .. } => {
                for child in children {
                    child.collect_files_recursive(paths);
                }
            }
        }
    }

    /// Returns all tracks (path and metadata) under this entry recursively.
    pub fn get_all_tracks(&self) -> Vec<(PathBuf, crate::data::metadata::TrackMetadata)> {
        let mut tracks = Vec::new();
        self.collect_tracks_recursive(&mut tracks);
        tracks
    }

    fn collect_tracks_recursive(
        &self,
        tracks: &mut Vec<(PathBuf, crate::data::metadata::TrackMetadata)>,
    ) {
        match self {
            LibraryEntry::Track { path, metadata, .. } => {
                tracks.push((path.clone(), metadata.clone()));
            }
            LibraryEntry::Directory { children, .. } => {
                for child in children {
                    child.collect_tracks_recursive(tracks);
                }
            }
        }
    }

    /// True if ALL audio tracks under this entry are present in `entry_paths`.
    /// Used by the flat-library builder to pre-cache enqueue status, avoiding
    /// per-frame recursive tree walks in the render loop.
    pub fn all_tracks_enqueued(&self, entry_paths: &HashSet<PathBuf>) -> bool {
        match self {
            LibraryEntry::Track { path, .. } => entry_paths.contains(path),
            LibraryEntry::Directory { children, .. } => {
                let mut has_track = false;
                for child in children {
                    match child {
                        LibraryEntry::Track { path, .. } => {
                            has_track = true;
                            if !entry_paths.contains(path) {
                                return false;
                            }
                        }
                        LibraryEntry::Directory { .. } => {
                            has_track = true;
                            if !child.all_tracks_enqueued(entry_paths) {
                                return false;
                            }
                        }
                    }
                }
                has_track // empty dirs return false (not "all enqueued")
            }
        }
    }
}

/// Returns true if the file extension is a supported audio format.
fn is_audio_file(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| AUDIO_EXTENSIONS.iter().any(|&ext| e.eq_ignore_ascii_case(ext)))
        .unwrap_or(false)
}

/// A flattened representation of a LibraryEntry for UI rendering.
#[derive(Debug, Clone)]
pub struct FlatLibraryItem {
    pub entry: LibraryEntry,
    #[allow(dead_code)]
    pub depth: usize,
    pub is_last: bool,
    pub ancestor_last: Vec<bool>,
    /// Pre-computed enqueue status — true when ALL tracks under this item are in
    /// the playlist. Cached here so `draw_library` never calls `get_all_files()`
    /// in the render hot-path (was O(N×depth) per visible row per frame).
    pub enqueued: bool,
}

/// Intermediate entry collected during the WalkDir phase (Phase 1).
/// Keeps filesystem traversal and metadata I/O fully separate so rayon
/// can parallelize metadata reads across all CPU cores in Phase 2.
struct WalkRecord {
    is_dir: bool,
    path: PathBuf,
    name: String,
}

/// Scans a root directory recursively, reading metadata in parallel via rayon.
///
/// # Two-phase design
/// **Phase 1** — fast `WalkDir` traversal that only performs `stat(2)` calls and
/// collects (path, name, is_dir) records in filesystem order.
/// **Phase 2** — rayon parallel iterator reads audio metadata for every track
/// simultaneously across all CPU cores, giving 4–8× speedup over a sequential
/// scan on a multi-core machine.
pub fn scan_library(root: &Path, strip_track_numbers: bool) -> Vec<LibraryEntry> {
    if !root.is_dir() {
        return Vec::new();
    }

    // ── Phase 1: WalkDir traversal ───────────────────────────────────────────
    // Collect directory/file records in sorted order. No metadata I/O here.
    let mut walk_records: Vec<WalkRecord> = Vec::new();
    let mut audio_paths: Vec<PathBuf> = Vec::new();

    let walker = walkdir::WalkDir::new(root)
        .min_depth(1)
        .sort_by(|a, b| {
            let a_name = a.file_name().to_string_lossy().to_lowercase();
            let b_name = b.file_name().to_string_lossy().to_lowercase();
            a_name.cmp(&b_name)
        })
        .into_iter()
        .filter_entry(|e| {
            let name = e.file_name().to_string_lossy();
            !name.starts_with('.')
        });

    for entry in walker {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };

        let path = entry.path().to_path_buf();
        let name = entry.file_name().to_string_lossy().to_string();
        let is_dir = entry.file_type().is_dir();

        if !is_dir {
            if !is_audio_file(&path) {
                continue;
            }
            audio_paths.push(path.clone());
        }

        walk_records.push(WalkRecord { is_dir, path, name });
    }

    // ── Phase 2: Parallel metadata reads (rayon) ─────────────────────────────
    // Each track path is processed independently — no shared state, no locks.
    let metadata_vec: Vec<crate::data::metadata::TrackMetadata> = audio_paths
        .par_iter()
        .map(|p| read_metadata(p))
        .collect();

    // Build O(1) path→metadata lookup. `remove` later avoids cloning metadata.
    let mut path_to_meta: HashMap<PathBuf, crate::data::metadata::TrackMetadata> =
        audio_paths.into_iter().zip(metadata_vec).collect();

    // ── Phase 3: Rebuild tree using the pre-computed metadata ─────────────────
    let mut dir_stack: Vec<(PathBuf, String, Vec<LibraryEntry>)> = Vec::new();
    let mut root_entries: Vec<LibraryEntry> = Vec::new();

    for rec in walk_records {
        // Pop directories that no longer contain the current path
        while let Some(top) = dir_stack.last() {
            if !rec.path.starts_with(&top.0) {
                let (d_path, d_name, d_children) = dir_stack.pop().unwrap();
                if !d_children.is_empty() {
                    let dir_entry = LibraryEntry::Directory {
                        name: d_name,
                        path: d_path,
                        children: d_children,
                    };
                    if let Some(parent) = dir_stack.last_mut() {
                        parent.2.push(dir_entry);
                    } else {
                        root_entries.push(dir_entry);
                    }
                }
            } else {
                break;
            }
        }

        if rec.is_dir {
            dir_stack.push((rec.path, rec.name, Vec::new()));
        } else {
            // `remove` takes ownership of the metadata avoiding a clone.
            let meta = path_to_meta.remove(&rec.path).unwrap_or_default();
            let display_name = format!(
                "{} - {}",
                meta.display_title(strip_track_numbers),
                meta.display_artist()
            );
            let track_entry = LibraryEntry::Track {
                name: display_name,
                path: rec.path,
                metadata: meta,
            };
            if let Some(parent) = dir_stack.last_mut() {
                parent.2.push(track_entry);
            } else {
                root_entries.push(track_entry);
            }
        }
    }

    // Drain remaining open directories
    while let Some((d_path, d_name, d_children)) = dir_stack.pop() {
        if !d_children.is_empty() {
            let dir_entry = LibraryEntry::Directory {
                name: d_name,
                path: d_path,
                children: d_children,
            };
            if let Some(parent) = dir_stack.last_mut() {
                parent.2.push(dir_entry);
            } else {
                root_entries.push(dir_entry);
            }
        }
    }

    root_entries
}

/// Flattens the library tree for UI rendering. Collapsed directories are not
/// expanded. Each `FlatLibraryItem` carries a pre-computed `enqueued` bool so
/// `draw_library` can check enqueue status in O(1) without recursive tree walks.
pub fn flatten_library(
    entries: &[LibraryEntry],
    depth: usize,
    ancestor_last: Vec<bool>,
    collapsed_dirs: &HashSet<PathBuf>,
    entry_paths: &HashSet<PathBuf>,
) -> Vec<FlatLibraryItem> {
    let mut result = Vec::new();
    let len = entries.len();
    for (i, entry) in entries.iter().enumerate() {
        let is_last = i == len - 1;
        let mut current_ancestors = ancestor_last.clone();

        // Pre-compute enqueue status — O(N_tracks_in_subtree) but only during
        // rebuild (on library/playlist change), not on every render frame.
        let enqueued = entry.all_tracks_enqueued(entry_paths);

        result.push(FlatLibraryItem {
            entry: entry.clone(),
            depth,
            is_last,
            ancestor_last: current_ancestors.clone(),
            enqueued,
        });

        if let LibraryEntry::Directory { path, children, .. } = entry {
            if !collapsed_dirs.contains(path) {
                current_ancestors.push(is_last);
                result.extend(flatten_library(
                    children,
                    depth + 1,
                    current_ancestors,
                    collapsed_dirs,
                    entry_paths,
                ));
            }
        }
    }
    result
}
