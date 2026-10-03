use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use crate::app::SourceTab;

/// Source-neutral reference identifying a playable track.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TrackRef {
    Local(PathBuf),
    Spotify(String),
    YouTube(String),
}

impl TrackRef {
    pub fn is_local(&self) -> bool {
        matches!(self, TrackRef::Local(_))
    }

    pub fn local_path(&self) -> Option<&Path> {
        match self {
            TrackRef::Local(p) => Some(p.as_path()),
            _ => None,
        }
    }

    pub fn source_kind(&self) -> SourceTab {
        match self {
            TrackRef::Local(_) => SourceTab::Local,
            TrackRef::Spotify(_) => SourceTab::Spotify,
            TrackRef::YouTube(_) => SourceTab::YouTube,
        }
    }
}

impl From<PathBuf> for TrackRef {
    fn from(p: PathBuf) -> Self {
        TrackRef::Local(p)
    }
}

impl From<&Path> for TrackRef {
    fn from(p: &Path) -> Self {
        TrackRef::Local(p.to_path_buf())
    }
}

impl From<&PathBuf> for TrackRef {
    fn from(p: &PathBuf) -> Self {
        TrackRef::Local(p.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_track_ref_helpers() {
        let local = TrackRef::Local(PathBuf::from("/music/song.mp3"));
        assert!(local.is_local());
        assert_eq!(local.local_path(), Some(Path::new("/music/song.mp3")));
        assert_eq!(local.source_kind(), SourceTab::Local);

        let spotify = TrackRef::Spotify("spotify:track:123".to_string());
        assert!(!spotify.is_local());
        assert_eq!(spotify.local_path(), None);
        assert_eq!(spotify.source_kind(), SourceTab::Spotify);

        let yt = TrackRef::YouTube("dQw4w9WgXcQ".to_string());
        assert!(!yt.is_local());
        assert_eq!(yt.local_path(), None);
        assert_eq!(yt.source_kind(), SourceTab::YouTube);
    }
}
