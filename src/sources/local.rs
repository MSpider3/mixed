use super::{BrowseItem, BrowseItemKind};
use crate::data::library::{FlatLibraryItem, LibraryEntry};
use crate::data::track::TrackRef;

/// Convert a single LibraryEntry into a generic BrowseItem.
pub fn entry_to_browse(entry: &LibraryEntry) -> BrowseItem {
    match entry {
        LibraryEntry::Track {
            name,
            path,
            metadata,
        } => BrowseItem {
            id: path.to_string_lossy().to_string(),
            title: metadata.title.clone().unwrap_or_else(|| name.clone()),
            subtitle: metadata.artist.clone(),
            kind: BrowseItemKind::Track,
            is_container: false,
            track_ref: Some(TrackRef::Local(path.clone())),
            duration_secs: metadata.duration.map(|d| d.as_secs()),
            artwork_url: None,
            album: None,
            depth: 0,
        },
        LibraryEntry::Directory { name, path, .. } => BrowseItem {
            id: path.to_string_lossy().to_string(),
            title: name.clone(),
            subtitle: None,
            kind: BrowseItemKind::Album,
            is_container: true,
            track_ref: None,
            duration_secs: None,
            artwork_url: None,
            album: None,
            depth: 0,
        },
    }
}

/// Convert flat library items into generic BrowseItems.
pub fn flat_library_to_browse(items: &[FlatLibraryItem]) -> Vec<BrowseItem> {
    items
        .iter()
        .map(|item| entry_to_browse(&item.entry))
        .collect()
}

/// Local filesystem music source provider.
#[derive(Debug, Default, Clone)]
pub struct LocalSource;

impl crate::sources::MusicSource for LocalSource {
    fn name(&self) -> &'static str {
        "Local"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::metadata::TrackMetadata;
    use std::path::PathBuf;

    #[test]
    fn test_local_library_items_to_browse() {
        let entry = LibraryEntry::Track {
            name: "song.mp3".into(),
            path: PathBuf::from("/music/song.mp3"),
            metadata: TrackMetadata {
                title: Some("Song".into()),
                artist: Some("Artist".into()),
                ..Default::default()
            },
        };

        let browse = entry_to_browse(&entry);
        assert_eq!(browse.title, "Song");
        assert!(!browse.is_container);
        assert_eq!(
            browse.track_ref,
            Some(TrackRef::Local(PathBuf::from("/music/song.mp3")))
        );
        assert_eq!(browse.subtitle.as_deref(), Some("Artist"));
    }
}
