use std::path::{Path, PathBuf};

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
}

/// Scans a root directory recursively using WalkDir and builds a tree of `LibraryEntry` items.
/// Empty directories and hidden files/directories are pruned.
pub fn scan_library(root: &Path, strip_track_numbers: bool) -> Vec<LibraryEntry> {
    if !root.is_dir() {
        return Vec::new();
    }

    let mut dir_stack: Vec<(PathBuf, String, Vec<LibraryEntry>)> = Vec::new();
    let mut root_entries: Vec<LibraryEntry> = Vec::new();

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

        while let Some(top) = dir_stack.last() {
            if !path.starts_with(&top.0) {
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

        if entry.file_type().is_dir() {
            dir_stack.push((path, name, Vec::new()));
        } else if is_audio_file(&path) {
            let meta = read_metadata(&path);
            let display_name = format!(
                "{} - {}",
                meta.display_title(strip_track_numbers),
                meta.display_artist()
            );
            let track_entry = LibraryEntry::Track {
                name: display_name,
                path,
                metadata: meta,
            };
            if let Some(parent) = dir_stack.last_mut() {
                parent.2.push(track_entry);
            } else {
                root_entries.push(track_entry);
            }
        }
    }

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

/// Flattens the library tree recursively. Skipped/collapsed directories are not expanded.
pub fn flatten_library(
    entries: &[LibraryEntry],
    depth: usize,
    ancestor_last: Vec<bool>,
    collapsed_dirs: &std::collections::HashSet<PathBuf>,
) -> Vec<FlatLibraryItem> {
    let mut result = Vec::new();
    let len = entries.len();
    for (i, entry) in entries.iter().enumerate() {
        let is_last = i == len - 1;
        let mut current_ancestors = ancestor_last.clone();

        result.push(FlatLibraryItem {
            entry: entry.clone(),
            depth,
            is_last,
            ancestor_last: current_ancestors.clone(),
        });

        if let LibraryEntry::Directory { path, children, .. } = entry {
            if !collapsed_dirs.contains(path) {
                current_ancestors.push(is_last);
                result.extend(flatten_library(
                    children,
                    depth + 1,
                    current_ancestors,
                    collapsed_dirs,
                ));
            }
        }
    }
    result
}
