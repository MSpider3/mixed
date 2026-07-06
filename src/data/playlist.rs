#![allow(dead_code)]
use std::path::PathBuf;

use crate::data::metadata::TrackMetadata;

/// Repeat mode for the player.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, Default)]
pub enum RepeatMode {
    #[default]
    Off,
    Track,
    Queue,
}

impl RepeatMode {
    pub fn next(self) -> Self {
        match self {
            RepeatMode::Off => RepeatMode::Track,
            RepeatMode::Track => RepeatMode::Queue,
            RepeatMode::Queue => RepeatMode::Off,
        }
    }

    #[allow(dead_code)]
    pub fn label(self) -> &'static str {
        match self {
            RepeatMode::Off => "Repeat: Off",
            RepeatMode::Track => "Repeat: Track",
            RepeatMode::Queue => "Repeat: Queue",
        }
    }

    pub fn symbol(self) -> &'static str {
        match self {
            RepeatMode::Off => "",
            RepeatMode::Track => "🔂",
            RepeatMode::Queue => "🔁",
        }
    }
}

/// A visual item for folder-grouped queue rendering.
#[derive(Debug, Clone)]
pub enum QueueVisualItem {
    Header {
        name: String,
    },
    Separator,
    Track {
        entry_idx: usize,
        title: String,
        duration: String,
    },
}

/// A single entry in the playlist / queue.
#[derive(Debug, Clone)]
pub struct PlaylistEntry {
    pub path: PathBuf,
    pub metadata: TrackMetadata,
}

/// The playlist / queue manager.
#[derive(Debug, Clone)]
pub struct Playlist {
    pub entries: Vec<PlaylistEntry>,
    pub current: usize, // Index in play_order
    pub repeat: RepeatMode,
    pub shuffle: bool,
    pub play_order: Vec<usize>,
    pub cached_visual_items: std::cell::RefCell<Option<(bool, bool, Vec<QueueVisualItem>)>>,
    pub entry_paths: std::collections::HashSet<PathBuf>,
}

impl Default for Playlist {
    fn default() -> Self {
        Self {
            entries: Vec::new(),
            current: 0,
            repeat: RepeatMode::Off,
            shuffle: false,
            play_order: Vec::new(),
            cached_visual_items: std::cell::RefCell::new(None),
            entry_paths: std::collections::HashSet::new(),
        }
    }
}

struct Xorshift {
    state: u32,
}

impl Xorshift {
    fn new(seed: u32) -> Self {
        Self {
            state: if seed == 0 { 1 } else { seed },
        }
    }

    fn next(&mut self) -> u32 {
        let mut x = self.state;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.state = x;
        x
    }

    fn gen_range(&mut self, limit: usize) -> usize {
        if limit == 0 {
            return 0;
        }
        (self.next() as usize) % limit
    }
}

impl Playlist {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn len(&self) -> usize {
        self.entries.len()
    }
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn current_real_index(&self) -> Option<usize> {
        if self.play_order.is_empty() || self.current >= self.play_order.len() {
            None
        } else {
            Some(self.play_order[self.current])
        }
    }

    pub fn add(&mut self, path: PathBuf, metadata: TrackMetadata) {
        *self.cached_visual_items.borrow_mut() = None;
        let new_idx = self.entries.len();
        self.entry_paths.insert(path.clone());
        self.entries.push(PlaylistEntry { path, metadata });
        self.play_order.push(new_idx);

        // If shuffle is active, keep current first and shuffle the rest
        if self.shuffle && self.play_order.len() > 1 {
            let current_real = self.current_real_index().unwrap_or(0);
            let mut others: Vec<usize> = (0..self.entries.len())
                .filter(|&i| i != current_real)
                .collect();
            let seed = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs() as u32)
                .unwrap_or(12345)
                + new_idx as u32;
            let mut rng = Xorshift::new(seed);
            if !others.is_empty() {
                for i in (1..others.len()).rev() {
                    let j = rng.gen_range(i + 1);
                    others.swap(i, j);
                }
            }
            let mut new_order = vec![current_real];
            new_order.extend(others);
            self.play_order = new_order;
            self.current = 0;
        }
    }

    pub fn remove(&mut self, idx: usize) {
        *self.cached_visual_items.borrow_mut() = None;
        if idx < self.entries.len() {
            let playing_real = self.current_real_index();
            self.entries.remove(idx);
            self.entry_paths = self.entries.iter().map(|e| e.path.clone()).collect();

            // Rebuild play_order
            let mut new_order = Vec::new();
            for &o in &self.play_order {
                if o == idx {
                    continue;
                } else if o > idx {
                    new_order.push(o - 1);
                } else {
                    new_order.push(o);
                }
            }
            self.play_order = new_order;

            // Restore self.current matching the playing_real
            if let Some(real_idx) = playing_real {
                let target_real = if real_idx >= idx {
                    real_idx.saturating_sub(1)
                } else {
                    real_idx
                };
                if let Some(pos) = self.play_order.iter().position(|&o| o == target_real) {
                    self.current = pos;
                } else {
                    self.current = 0;
                }
            } else {
                self.current = 0;
            }

            if self.current >= self.play_order.len() && !self.play_order.is_empty() {
                self.current = self.play_order.len() - 1;
            }
        }
    }

    pub fn clear(&mut self) {
        *self.cached_visual_items.borrow_mut() = None;
        self.entries.clear();
        self.play_order.clear();
        self.current = 0;
        self.entry_paths.clear();
    }

    pub fn current_entry(&self) -> Option<&PlaylistEntry> {
        self.current_real_index()
            .and_then(|idx| self.entries.get(idx))
    }

    pub fn can_go_next(&self) -> bool {
        if self.play_order.is_empty() {
            return false;
        }

        match self.repeat {
            RepeatMode::Track | RepeatMode::Queue => true,
            RepeatMode::Off => self.current + 1 < self.play_order.len(),
        }
    }

    pub fn can_go_previous(&self) -> bool {
        if self.play_order.is_empty() {
            return false;
        }

        match self.repeat {
            RepeatMode::Track | RepeatMode::Queue => true,
            RepeatMode::Off => self.current > 0,
        }
    }

    pub fn advance_track(&mut self) -> bool {
        if self.play_order.is_empty() {
            return false;
        }
        match self.repeat {
            RepeatMode::Track => true,
            RepeatMode::Queue => {
                self.current = (self.current + 1) % self.play_order.len();
                true
            }
            RepeatMode::Off => {
                if self.current + 1 < self.play_order.len() {
                    self.current += 1;
                    true
                } else {
                    false
                }
            }
        }
    }

    pub fn prev(&mut self) -> bool {
        if self.play_order.is_empty() {
            return false;
        }
        match self.repeat {
            RepeatMode::Track => true,
            RepeatMode::Queue => {
                if self.current == 0 {
                    self.current = self.play_order.len() - 1;
                } else {
                    self.current -= 1;
                }
                true
            }
            RepeatMode::Off => {
                if self.current > 0 {
                    self.current -= 1;
                    true
                } else {
                    false
                }
            }
        }
    }

    pub fn move_up(&mut self, idx: usize) {
        *self.cached_visual_items.borrow_mut() = None;
        if idx > 0 && idx < self.entries.len() {
            self.entries.swap(idx, idx - 1);
            for o in &mut self.play_order {
                if *o == idx {
                    *o = idx - 1;
                } else if *o == idx - 1 {
                    *o = idx;
                }
            }
        }
    }

    pub fn move_down(&mut self, idx: usize) {
        *self.cached_visual_items.borrow_mut() = None;
        if idx + 1 < self.entries.len() {
            self.entries.swap(idx, idx + 1);
            for o in &mut self.play_order {
                if *o == idx {
                    *o = idx + 1;
                } else if *o == idx + 1 {
                    *o = idx;
                }
            }
        }
    }

    pub fn set_shuffle(&mut self, shuffle: bool) {
        self.shuffle = shuffle;
        if self.entries.is_empty() {
            self.play_order.clear();
            return;
        }

        let current_real = self.current_real_index().unwrap_or(0);

        if shuffle {
            let seed = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs() as u32)
                .unwrap_or(12345);
            let mut rng = Xorshift::new(seed);

            let mut others: Vec<usize> = (0..self.entries.len())
                .filter(|&i| i != current_real)
                .collect();

            if !others.is_empty() {
                for i in (1..others.len()).rev() {
                    let j = rng.gen_range(i + 1);
                    others.swap(i, j);
                }
            }

            let mut new_order = vec![current_real];
            new_order.extend(others);
            self.play_order = new_order;
            self.current = 0;
        } else {
            self.play_order = (0..self.entries.len()).collect();
            self.current = current_real;
        }
    }

    pub fn get_visual_items(
        &self,
        show_folders: bool,
        strip_track_numbers: bool,
    ) -> std::cell::Ref<'_, [QueueVisualItem]> {
        let is_cached =
            if let Some((cached_show, cached_strip, _)) = &*self.cached_visual_items.borrow() {
                *cached_show == show_folders && *cached_strip == strip_track_numbers
            } else {
                false
            };

        if !is_cached {
            let mut cache = self.cached_visual_items.borrow_mut();
            let mut items = Vec::new();
            let mut prev_parent: Option<PathBuf> = None;
            for (idx, entry) in self.entries.iter().enumerate() {
                if show_folders {
                    let parent = entry.path.parent().map(|p| p.to_path_buf());
                    let same = match (&prev_parent, &parent) {
                        (Some(a), Some(b)) => a == b,
                        _ => false,
                    };
                    if !same {
                        if prev_parent.is_some() {
                            items.push(QueueVisualItem::Separator);
                        }
                        let name = parent
                            .as_deref()
                            .and_then(|p| p.file_name())
                            .and_then(|n| n.to_str())
                            .unwrap_or("Unknown")
                            .to_string();
                        items.push(QueueVisualItem::Header { name });
                        prev_parent = parent;
                    }
                }
                items.push(QueueVisualItem::Track {
                    entry_idx: idx,
                    title: format!(
                        "{} - {}",
                        entry.metadata.display_title(strip_track_numbers),
                        entry.metadata.display_artist()
                    ),
                    duration: "".to_string(),
                });
            }
            *cache = Some((show_folders, strip_track_numbers, items));
        }

        std::cell::Ref::map(self.cached_visual_items.borrow(), |opt| {
            &opt.as_ref().unwrap().2[..]
        })
    }

    pub fn visual_to_real(
        &self,
        visual_idx: usize,
        show_folders: bool,
        strip_track_numbers: bool,
    ) -> Option<usize> {
        let items = self.get_visual_items(show_folders, strip_track_numbers);
        items.get(visual_idx).and_then(|item| match item {
            QueueVisualItem::Track { entry_idx, .. } => Some(*entry_idx),
            _ => None,
        })
    }

    pub fn paths(&self) -> Vec<PathBuf> {
        self.entries.iter().map(|e| e.path.clone()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn add_track(playlist: &mut Playlist, name: &str) {
        playlist.add(PathBuf::from(name), TrackMetadata::default());
    }

    #[test]
    fn navigation_capabilities_single_track_no_repeat() {
        let mut playlist = Playlist::new();
        add_track(&mut playlist, "one.mp3");
        playlist.current = 0;
        playlist.repeat = RepeatMode::Off;
        assert!(!playlist.can_go_next());
        assert!(!playlist.can_go_previous());
    }

    #[test]
    fn navigation_capabilities_follow_play_order_and_repeat() {
        let mut playlist = Playlist::new();
        assert!(!playlist.can_go_next());
        assert!(!playlist.can_go_previous());

        add_track(&mut playlist, "one.mp3");
        add_track(&mut playlist, "two.mp3");

        playlist.current = 0;
        assert!(playlist.can_go_next());
        assert!(!playlist.can_go_previous());

        playlist.current = 1;
        assert!(!playlist.can_go_next());
        assert!(playlist.can_go_previous());

        playlist.repeat = RepeatMode::Queue;
        assert!(playlist.can_go_next());
        assert!(playlist.can_go_previous());

        playlist.repeat = RepeatMode::Track;
        assert!(playlist.can_go_next());
        assert!(playlist.can_go_previous());
    }
}
