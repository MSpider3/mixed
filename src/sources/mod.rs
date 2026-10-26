pub mod local;
#[cfg(any(feature = "spotify", feature = "youtube"))]
pub mod lyrics;
pub mod runtime;
pub mod spotify;
#[cfg(feature = "spotify")]
pub mod spotify_catalog;
#[cfg(feature = "spotify")]
pub mod spotify_oauth;
pub mod youtube;
#[cfg(feature = "youtube")]
pub mod youtube_login;
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

/// The sections of a search result list, in the order they are shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ResultGroup {
    Songs,
    Albums,
    Artists,
    Playlists,
    Folders,
}

impl ResultGroup {
    pub fn label(self) -> &'static str {
        match self {
            ResultGroup::Songs => "Songs",
            ResultGroup::Albums => "Albums",
            ResultGroup::Artists => "Artists",
            ResultGroup::Playlists => "Playlists",
            ResultGroup::Folders => "Folders",
        }
    }

    pub fn of_browse(item: &BrowseItem) -> Self {
        match item.kind {
            BrowseItemKind::Track => ResultGroup::Songs,
            BrowseItemKind::Album => ResultGroup::Albums,
            BrowseItemKind::Artist => ResultGroup::Artists,
            BrowseItemKind::Playlist => ResultGroup::Playlists,
        }
    }

    pub fn of_library(entry: &crate::data::library::LibraryEntry) -> Self {
        if entry.is_dir() {
            ResultGroup::Folders
        } else {
            ResultGroup::Songs
        }
    }
}

/// The results `scope` lets through (all of them for `None`), grouped in
/// `ResultGroup` order. Each group keeps the order the search ranked them in.
pub fn arrange_results<T: Clone>(
    all: &[T],
    scope: Option<ResultGroup>,
    group: impl Fn(&T) -> ResultGroup,
) -> Vec<T> {
    let mut shown: Vec<T> = all
        .iter()
        .filter(|item| scope.is_none_or(|s| group(item) == s))
        .cloned()
        .collect();
    shown.sort_by_key(|item| group(item));
    shown
}

/// The groups among `all`, in display order, with how many results each holds.
pub fn result_groups<T>(all: &[T], group: impl Fn(&T) -> ResultGroup) -> Vec<(ResultGroup, usize)> {
    let mut counts: Vec<(ResultGroup, usize)> = Vec::new();
    for g in all.iter().map(group) {
        match counts.iter_mut().find(|(seen, _)| *seen == g) {
            Some((_, n)) => *n += 1,
            None => counts.push((g, 1)),
        }
    }
    counts.sort_by_key(|&(g, _)| g);
    counts
}

/// The scope after `scope` when cycling: all results, then each group present.
pub fn next_scope(
    scope: Option<ResultGroup>,
    present: &[(ResultGroup, usize)],
) -> Option<ResultGroup> {
    let order: Vec<Option<ResultGroup>> = std::iter::once(None)
        .chain(present.iter().map(|&(g, _)| Some(g)))
        .collect();
    let at = order.iter().position(|&s| s == scope).unwrap_or(0);
    order[(at + 1) % order.len()]
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
    /// Album a track belongs to, where the source reports it.
    pub album: Option<String>,
    /// Nesting level in a browse tree: 0 for roots, parent depth + 1 for children.
    pub depth: usize,
}

impl BrowseItem {
    /// The queue entry for a track row; `None` for folders.
    pub fn queue_entry(self) -> Option<(TrackRef, crate::data::metadata::TrackMetadata)> {
        let track = self.track_ref?;
        Some((
            track,
            crate::data::metadata::TrackMetadata {
                title: Some(self.title),
                artist: self.subtitle,
                album: self.album,
                duration: self.duration_secs.map(std::time::Duration::from_secs),
                cover_url: self.artwork_url,
                ..Default::default()
            },
        ))
    }
}

/// State for a directory level in the navigation stack ("cd" / "cd ..").
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrowseFolderState {
    pub id: String,
    pub title: String,
    pub saved_cursor: usize,
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
    /// Directory navigation stack for hierarchical browsing ("cd" / "cd ..").
    pub nav_stack: Vec<BrowseFolderState>,
    /// In-memory cache of container children: container_id -> Vec<BrowseItem>.
    pub cache: std::collections::HashMap<String, Vec<BrowseItem>>,
    pub cursor: usize,
    pub search_cursor: usize,
    pub loading: bool,
    pub error: Option<String>,
    pub search_query: String,
    /// What is shown of `search_all`: the current scope's results, grouped.
    pub search_results: Vec<BrowseItem>,
    /// Every result of the current search, as the source ranked them.
    pub search_all: Vec<BrowseItem>,
    /// Which group of results is shown; `None` shows them all.
    pub search_scope: Option<ResultGroup>,
    pub search_generation: u64,
    pub next_page: usize,
    pub login_input: String,
    pub awaiting_login_input: bool,
    /// Progress of a sign-in that is still running.
    pub login_status: Option<String>,
}

/// Id prefix of the "add all tracks" row at the top of an open folder; the rest is the
/// folder's id.
pub const ADD_ALL_PREFIX: &str = "add_all:";

/// The rows of an open folder: its children, after an "add all tracks" row when it
/// holds any track.
fn folder_rows(folder_id: &str, children: &[BrowseItem]) -> Vec<BrowseItem> {
    let tracks = children.iter().filter(|c| c.track_ref.is_some()).count();
    let mut rows = Vec::with_capacity(children.len() + 1);
    if tracks > 0 {
        rows.push(BrowseItem {
            id: format!("{}{}", ADD_ALL_PREFIX, folder_id),
            title: format!("Add all {} tracks", tracks),
            subtitle: None,
            kind: BrowseItemKind::Playlist,
            is_container: false,
            track_ref: None,
            duration_secs: None,
            artwork_url: None,
            album: None,
            depth: 0,
        });
    }
    rows.extend_from_slice(children);
    rows
}

impl SourceView {
    /// Show what the scope lets through of `search_all`, keeping the cursor on its item.
    pub fn arrange_search(&mut self) {
        let selected = self
            .search_results
            .get(self.search_cursor)
            .map(|item| item.id.clone());
        self.search_results =
            arrange_results(&self.search_all, self.search_scope, ResultGroup::of_browse);
        self.search_cursor = selected
            .and_then(|id| self.search_results.iter().position(|item| item.id == id))
            .unwrap_or(0);
    }

    /// Returns the breadcrumb string for the current navigation state.
    pub fn current_path_display(&self, root_name: &str) -> String {
        if self.nav_stack.is_empty() {
            format!("📁 {}", root_name)
        } else {
            let trail = self
                .nav_stack
                .iter()
                .map(|s| s.title.as_str())
                .collect::<Vec<_>>()
                .join(" / ");
            format!("📁 {} / {}", root_name, trail)
        }
    }

    /// Drill down into a folder/container ("cd"). Returns true if children were in cache.
    pub fn drill_down(&mut self, container: &BrowseItem) -> bool {
        self.nav_stack.push(BrowseFolderState {
            id: container.id.clone(),
            title: container.title.clone(),
            saved_cursor: self.cursor,
        });
        if let Some(cached) = self.cache.get(&container.id) {
            self.flat = folder_rows(&container.id, cached);
            self.cursor = 0;
            self.loading = false;
            true
        } else {
            self.flat.clear();
            self.cursor = 0;
            self.loading = true;
            false
        }
    }

    /// Navigate back up to the parent directory ("cd .."). Returns true if popped.
    pub fn navigate_up(&mut self) -> bool {
        let Some(popped) = self.nav_stack.pop() else {
            return false;
        };
        if let Some(parent) = self.nav_stack.last() {
            let children = self.cache.get(&parent.id).map(Vec::as_slice);
            self.flat = folder_rows(&parent.id, children.unwrap_or_default());
        } else {
            self.flat = self.roots.clone();
        }
        self.cursor = popped.saved_cursor.min(self.flat.len().saturating_sub(1));
        self.loading = false;
        true
    }

    /// Insert or cache a container's children.
    pub fn expand(&mut self, parent_id: &str, children: Vec<BrowseItem>) {
        self.cache.insert(parent_id.to_string(), children.clone());

        // If we navigated into this folder ("cd"), populate the active view
        if self.nav_stack.last().map(|s| s.id.as_str()) == Some(parent_id) {
            self.flat = folder_rows(parent_id, &children);
            self.cursor = 0;
            self.loading = false;
            self.expanded.insert(parent_id.to_string());
            return;
        }

        if !self.expanded.insert(parent_id.to_string()) {
            return;
        }

        let mut found = false;
        for list in [&mut self.flat, &mut self.search_results] {
            if let Some(idx) = list.iter().position(|it| it.id == parent_id) {
                found = true;
                let depth = list[idx].depth + 1;
                let nested = children.iter().cloned().map(|mut child| {
                    child.depth = depth;
                    child
                });
                list.splice(idx + 1..idx + 1, nested);
            }
        }
        if !found {
            self.expanded.remove(parent_id);
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

    /// Append an incremental page of items to the container's cache and active flat list.
    pub fn append_page(&mut self, parent_id: &str, items: Vec<BrowseItem>) {
        if let Some(cached) = self.cache.get_mut(parent_id) {
            cached.extend(items.clone());
        } else {
            self.cache.insert(parent_id.to_string(), items.clone());
        }
        if self.nav_stack.last().map(|s| s.id.as_str()) == Some(parent_id) {
            let cursor = self.cursor;
            self.flat = folder_rows(parent_id, &self.cache[parent_id]);
            self.cursor = cursor;
        }
    }
}

/// What lyrics are looked up by.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LyricsQuery {
    pub title: String,
    pub artist: String,
    pub album: Option<String>,
    pub duration_secs: Option<u64>,
}

/// How closely lyrics follow the music, best last.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum LyricsSync {
    Plain,
    Line,
    Word,
}

/// Lyrics a provider offers for a search.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LyricsCandidate {
    pub provider: &'static str,
    pub title: String,
    pub artist: String,
    pub album: Option<String>,
    pub duration_secs: Option<u64>,
    pub sync: LyricsSync,
    /// LRC text, or plain text for `LyricsSync::Plain`.
    pub text: String,
}

/// Requests dispatched to the background async network runtime.
#[derive(Debug, Clone)]
pub enum SourceRequest {
    CheckAuth(SourceTab),
    /// Sign in through a browser window that only mixed uses (YouTube Music).
    BrowserLogin(SourceTab),
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
    /// Find lyrics for a track: the best match of the first provider that has any.
    FetchLyrics {
        track: TrackRef,
        query: Box<LyricsQuery>,
    },
    /// List every match, for choosing by hand. `text` replaces the query's title
    /// and artist with the user's own words.
    SearchLyrics {
        track: TrackRef,
        query: Box<LyricsQuery>,
        text: Option<String>,
    },
    DownloadYouTubeTrack {
        video_id: String,
        generation: u64,
    },
    /// Play a YouTube track by streaming its audio URL; falls back to downloading.
    StreamYouTubeTrack {
        video_id: String,
        generation: u64,
    },
    /// Find the audio URLs of tracks that may play soon, so they start at once.
    PrepareYouTubeTracks {
        video_ids: Vec<String>,
    },
    /// The radio of a Spotify or YouTube track: similar tracks, starting with it.
    Radio {
        seed: TrackRef,
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
    /// What a sign-in that is still running is waiting for.
    LoginProgress {
        source: SourceTab,
        message: String,
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
    /// Answer to `StreamYouTubeTrack`: where the track's audio is fetched from.
    YouTubeTrackUrl {
        video_id: String,
        url: String,
        /// File extension of the audio (e.g. `m4a`), for the cache file and decoder.
        ext: String,
    },
    /// Answer to `Radio`.
    Radio {
        seed: TrackRef,
        result: Result<Vec<BrowseItem>, String>,
    },
    /// Answer to `FetchLyrics`: `Ok(None)` if no provider has lyrics for the track.
    LyricsFetched {
        track: TrackRef,
        result: Result<Option<LyricsCandidate>, String>,
    },
    /// Answer to `SearchLyrics`, best matches first.
    LyricsMatches {
        track: TrackRef,
        result: Result<Vec<LyricsCandidate>, String>,
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
            album: None,
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
    fn search_results_are_grouped_and_scoped() {
        // As a source ranks them: playlists and songs mixed
        let mut view = SourceView {
            search_all: vec![
                item("p1", true),
                item("s1", false),
                item("p2", true),
                item("s2", false),
            ],
            ..Default::default()
        };
        view.arrange_search();
        let shown = |view: &SourceView| {
            view.search_results
                .iter()
                .map(|it| it.id.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(
            shown(&view),
            ["s1", "s2", "p1", "p2"],
            "songs first, ranks kept"
        );

        let groups = result_groups(&view.search_all, ResultGroup::of_browse);
        assert_eq!(
            groups,
            [(ResultGroup::Songs, 2), (ResultGroup::Playlists, 2)]
        );

        // The cursor stays on its item while scopes change
        view.search_cursor = 3;
        view.search_scope = next_scope(view.search_scope, &groups);
        assert_eq!(view.search_scope, Some(ResultGroup::Songs));
        view.arrange_search();
        assert_eq!(shown(&view), ["s1", "s2"]);
        assert_eq!(view.search_cursor, 0, "p2 is not shown: back to the top");

        view.search_scope = next_scope(view.search_scope, &groups);
        view.arrange_search();
        assert_eq!(shown(&view), ["p1", "p2"]);
        view.search_cursor = 1;
        view.search_scope = next_scope(view.search_scope, &groups);
        assert_eq!(view.search_scope, None, "after the last group: all results");
        view.arrange_search();
        assert_eq!(view.search_results[view.search_cursor].id, "p2");
    }

    #[test]
    fn an_open_folder_with_tracks_starts_with_an_add_all_row() {
        let mut view = SourceView {
            flat: vec![item("playlist", true), item("shelf", true)],
            ..Default::default()
        };
        let track = |id: &str| BrowseItem {
            track_ref: Some(TrackRef::Spotify(id.into())),
            ..item(id, false)
        };

        // Pages arrive while the folder is open: the count follows them
        view.drill_down(&item("playlist", true));
        view.expand("playlist", vec![track("t1"), track("t2")]);
        assert_eq!(view.flat[0].id, "add_all:playlist");
        assert_eq!(view.flat[0].title, "Add all 2 tracks");
        view.cursor = 2;
        view.append_page("playlist", vec![track("t3")]);
        assert_eq!(view.flat[0].title, "Add all 3 tracks");
        assert_eq!(view.flat.len(), 4);
        assert_eq!(view.cursor, 2, "the cursor stays on its row");

        // Opened again from the cache
        view.navigate_up();
        view.drill_down(&item("playlist", true));
        assert_eq!(view.flat[0].title, "Add all 3 tracks");

        // A folder of folders has nothing to add
        view.navigate_up();
        view.drill_down(&item("shelf", true));
        view.expand("shelf", vec![item("p1", true)]);
        assert_eq!(ids(&view), vec![("p1", 0)]);
    }

    #[test]
    fn expand_ignores_a_parent_that_is_no_longer_listed() {
        let mut view = SourceView::default();
        view.expand("gone", vec![item("t1", false)]);
        assert!(view.flat.is_empty());
        assert!(view.expanded.is_empty());
    }
}
