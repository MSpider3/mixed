# Lyrics

`mixed` shows lyrics in the Now Playing view (`F4`). `m` switches between the three-line view and the full view.

## Where lyrics come from

For every track that starts playing, the first of these that has lyrics is used:

1. Lyrics `mixed` saved earlier for the track.
2. Local files only: a `.lrc` file with the same name next to the song (`Song.mp3` and `Song.lrc`), then the lyrics embedded in the song's tags.
3. Spotify tracks only: Spotify's own lyrics.
4. [Karalyr](https://www.karalyr.com), which has word-by-word synced lyrics for a small number of songs.
5. [LRCLIB](https://lrclib.net), which has line-synced or plain lyrics for most songs.

Steps 3 to 5 send the track's title, artist, album and length to that service. Synced lyrics are preferred; plain text is shown when nothing synced exists. Lyrics found online are saved, so each track is looked up once.

## Where lyrics are saved

| Track | Saved as |
|---|---|
| Local file | `Song.lrc` next to the song |
| Spotify or YouTube | A file in the `lyrics` folder of the cache directory (`~/.cache/mixed/lyrics/` on Linux) |

Set `"save_lyrics_next_to_songs": false` in `config.json` to keep the lyrics of local files in the cache folder too. They also go there when the music folder cannot be written.

Every lyrics file `mixed` writes starts with the line `[re:mixed]`. A `.lrc` file without that line is treated as your own: it is used as it is and never replaced without asking. To edit lyrics, open the `.lrc` file in a text editor.

When no lyrics are found for a track, that is remembered and the track is not searched again on later plays.

## Choosing other lyrics

When a track shows the wrong lyrics, or none, press `y` in the Now Playing view. A list opens:

| Row | What it does |
|---|---|
| Automatic: search again | Removes what `mixed` saved for the track and looks the lyrics up again as described above |
| No lyrics for this track | Removes what `mixed` saved and stops searching for this track. Your own `.lrc` file and embedded lyrics are kept and still shown |
| A match | Uses those lyrics and saves them in place of the previous ones |

Each match shows its title, artist, album, length, whether it is word-synced, synced or plain text, and the service it comes from. Matches of the same length as the track come first.

| Key | Action |
|---|---|
| `Up` / `Down` or `k` / `j` | Move in the list |
| `Enter` | Use the selected row |
| `/` | Search with your own words, for example when the track's title is wrong |
| `Esc` | Close the list (or cancel the search text) |

If the chosen lyrics would replace a `.lrc` file of your own, `mixed` asks first: `y` replaces it, any other key keeps it.
