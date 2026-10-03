pub mod local;
pub mod runtime;
pub mod spotify;
pub mod youtube;
pub mod ytdlp;

use crate::data::track::TrackRef;
use std::path::PathBuf;

/// Available music source tabs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SourceTab {
    #[default]
    Local,
    Spotify,
    YouTube,
    Unified,
}

impl SourceTab {
    pub const ALL: [SourceTab; 4] = [
        SourceTab::Local,
        SourceTab::Spotify,
        SourceTab::YouTube,
        SourceTab::Unified,
    ];

    pub fn index(self) -> usize {
        match self {
            SourceTab::Local => 0,
            SourceTab::Spotify => 1,
            SourceTab::YouTube => 2,
            SourceTab::Unified => 3,
        }
    }

    pub fn from_index(idx: usize) -> Option<Self> {
        match idx {
            0 => Some(SourceTab::Local),
            1 => Some(SourceTab::Spotify),
            2 => Some(SourceTab::YouTube),
            3 => Some(SourceTab::Unified),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            SourceTab::Local => "local",
            SourceTab::Spotify => "spotify",
            SourceTab::YouTube => "youtube",
            SourceTab::Unified => "queue",
        }
    }
}

/// Category of browse item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BrowseItemKind {
    Track,
    Album,
    Artist,
    Playlist,
}

/// A generic browse item shared across local, Spotify, and YouTube sources.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrowseItem {
    pub id: String,
    pub title: String,
    pub subtitle: Option<String>,
    pub kind: BrowseItemKind,
    pub is_container: bool,
    pub track_ref: Option<TrackRef>,
    pub duration_secs: Option<u64>,
    pub artwork_url: Option<String>,
    /// Nesting level in a browse tree: 0 for roots, parent depth + 1 for children.
    pub depth: usize,
}

/// State representation for remote browse and search views.
#[derive(Debug, Clone, Default)]
pub struct SourceView {
    pub connected: bool,
    pub user_name: Option<String>,
    pub roots: Vec<BrowseItem>,
    pub flat: Vec<BrowseItem>,
    /// Ids of containers whose children are already spliced into `flat`.
    pub expanded: std::collections::HashSet<String>,
    pub cursor: usize,
    pub search_cursor: usize,
    pub loading: bool,
    pub error: Option<String>,
    pub search_query: String,
    pub search_results: Vec<BrowseItem>,
    pub search_generation: u64,
    pub next_page: usize,
    pub login_input: String,
    pub awaiting_login_input: bool,
}

impl SourceView {
    /// Insert a container's children directly below it, one level deeper, in
    /// every list that shows the container (library tree and search results).
    pub fn expand(&mut self, parent_id: &str, children: Vec<BrowseItem>) {
        if self.expanded.contains(parent_id) {
            return;
        }
        let mut found = false;
        for list in [&mut self.flat, &mut self.search_results] {
            if let Some(idx) = list.iter().position(|it| it.id == parent_id) {
                let depth = list[idx].depth + 1;
                let nested = children.iter().cloned().map(|mut child| {
                    child.depth = depth;
                    child
                });
                list.splice(idx + 1..idx + 1, nested);
                found = true;
            }
        }
        if found {
            self.expanded.insert(parent_id.to_string());
        }
    }

    /// Remove everything nested below a container. Returns false if it was not expanded.
    pub fn collapse(&mut self, parent_id: &str) -> bool {
        if !self.expanded.remove(parent_id) {
            return false;
        }
        for list in [&mut self.flat, &mut self.search_results] {
            if let Some(idx) = list.iter().position(|it| it.id == parent_id) {
                let depth = list[idx].depth;
                let end = list[idx + 1..]
                    .iter()
                    .position(|it| it.depth <= depth)
                    .map_or(list.len(), |n| idx + 1 + n);
                for removed in list.drain(idx + 1..end) {
                    // Nested containers collapse along with their parent
                    self.expanded.remove(&removed.id);
                }
            }
        }
        self.cursor = self.cursor.min(self.flat.len().saturating_sub(1));
        self.search_cursor = self
            .search_cursor
            .min(self.search_results.len().saturating_sub(1));
        true
    }
}

/// Requests dispatched to the background async network runtime.
#[derive(Debug, Clone)]
pub enum SourceRequest {
    CheckAuth(SourceTab),
    Login {
        source: SourceTab,
        payload: String,
    },
    FetchRoots(SourceTab),
    FetchChildren {
        source: SourceTab,
        parent_id: String,
        page: usize,
    },
    Search {
        source: SourceTab,
        query: String,
        generation: u64,
        page: usize,
    },
    FetchCover {
        url: String,
    },
    DownloadYouTubeTrack {
        video_id: String,
        generation: u64,
    },
}

/// Events emitted by background sources back to the main UI loop.
#[derive(Debug, Clone)]
pub enum SourceEvent {
    AuthState {
        source: SourceTab,
        connected: bool,
        user_name: Option<String>,
        error: Option<String>,
    },
    Roots {
        source: SourceTab,
        items: Vec<BrowseItem>,
    },
    Children {
        source: SourceTab,
        parent_id: String,
        items: Vec<BrowseItem>,
        page: usize,
        has_more: bool,
    },
    SearchResults {
        source: SourceTab,
        query: String,
        generation: u64,
        items: Vec<BrowseItem>,
        page: usize,
        has_more: bool,
    },
    CoverFetched {
        url: String,
        path: PathBuf,
    },
    /// Enough of a YouTube track has been downloaded to start playing `path`
    /// (the partial file) while the rest arrives.
    YouTubeTrackStreamable {
        video_id: String,
        path: PathBuf,
        progress: std::sync::Arc<crate::audio::growing_file::DownloadProgress>,
    },
    YouTubeTrackDownloaded {
        video_id: String,
        path: PathBuf,
        generation: u64,
    },
    YouTubeDownloadFailed {
        video_id: String,
        error: String,
        generation: u64,
    },
    Error {
        source: SourceTab,
        error: String,
    },
}

/// Abstract contract for music source providers.
pub trait MusicSource: Send + Sync {
    fn name(&self) -> &'static str;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(id: &str, is_container: bool) -> BrowseItem {
        BrowseItem {
            id: id.to_string(),
            title: id.to_string(),
            subtitle: None,
            kind: if is_container {
                BrowseItemKind::Playlist
            } else {
                BrowseItemKind::Track
            },
            is_container,
            track_ref: None,
            duration_secs: None,
            artwork_url: None,
            depth: 0,
        }
    }

    fn ids(view: &SourceView) -> Vec<(&str, usize)> {
        view.flat
            .iter()
            .map(|it| (it.id.as_str(), it.depth))
            .collect()
    }

    #[test]
    fn expand_nests_children_and_collapse_removes_the_whole_subtree() {
        let mut view = SourceView {
            flat: vec![item("playlists", true), item("albums", true)],
            ..Default::default()
        };

        view.expand("playlists", vec![item("p1", true), item("p2", true)]);
        view.expand("p1", vec![item("t1", false), item("t2", false)]);
        // The same track may appear under a second playlist
        view.expand("p2", vec![item("t1", false)]);
        assert_eq!(
            ids(&view),
            vec![
                ("playlists", 0),
                ("p1", 1),
                ("t1", 2),
                ("t2", 2),
                ("p2", 1),
                ("t1", 2),
                ("albums", 0)
            ]
        );

        // A repeated reply for an expanded container changes nothing
        view.expand("p1", vec![item("t1", false)]);
        assert_eq!(view.flat.len(), 7);

        view.cursor = 5;
        assert!(view.collapse("playlists"));
        assert_eq!(ids(&view), vec![("playlists", 0), ("albums", 0)]);
        assert!(view.expanded.is_empty(), "nested containers are forgotten");
        assert_eq!(view.cursor, 1, "cursor is clamped into the shorter list");
        assert!(!view.collapse("playlists"));
    }

    #[test]
    fn expand_ignores_a_parent_that_is_no_longer_listed() {
        let mut view = SourceView::default();
        view.expand("gone", vec![item("t1", false)]);
        assert!(view.flat.is_empty());
        assert!(view.expanded.is_empty());
    }
}
