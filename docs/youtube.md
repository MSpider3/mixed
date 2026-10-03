# YouTube Music Setup

`mixed` uses two separate pieces for YouTube Music (`Ctrl+3` / `Alt+3`):

| Piece | Used for | Needs login? |
|---|---|---|
| YouTube Music API (`ytmapi-rs`) | Search, browsing albums and playlists, your library | Only for your library |
| [`yt-dlp`](https://github.com/yt-dlp/yt-dlp) | Downloading the audio that is played | No |

## Requirements

- `yt-dlp` must be installed. `mixed` looks for it on your `PATH`; to use a binary elsewhere, set `yt_dlp_path` in `config.json` (see [Configuration](#configuration)).
- Keep `yt-dlp` up to date. When YouTube changes something, the fix arrives as a `yt-dlp` update, not a `mixed` update.
- Recent `yt-dlp` releases warn that YouTube extraction without a JavaScript runtime (such as `deno`) is deprecated and that some formats may be missing. Installing one avoids that.

If `yt-dlp` is missing, playing a YouTube track shows `yt-dlp not found. Install it to play YouTube Music.`

## What works without logging in

- **Search** (`F5` or `/`): songs, albums and playlists.
- **Opening** an album or playlist from the search results (`Enter` opens it, `Enter` again closes it).
- **Playback** of any track you can enqueue.

Logging in adds your own library in the Library tab (`F3`): **Liked Songs**, **Playlists** and **Albums**.

## Logging in with your browser cookie

`mixed` signs in to YouTube Music with the cookie from a browser session.

### 1. Copy the cookie

1. Open [music.youtube.com](https://music.youtube.com) in your browser and sign in.
2. Open the developer tools (`F12`, or `Ctrl+Shift+I` / `Cmd+Option+I`) and select the **Network** tab.
3. Reload the page.
4. Click a request to `music.youtube.com` (for example one named `browse`).
5. Under **Request Headers**, find `cookie:` and copy its whole value.

### 2. Give it to `mixed`

**In the app (recommended):**

1. Switch to YouTube Music (`Ctrl+3` / `Alt+3`) and open the Library tab (`F3`).
2. Press `Enter`. A `Credentials:` prompt appears.
3. Paste the cookie and press `Enter`.

While the prompt is open, `Esc` cancels and `Ctrl+U` clears the input. On success the cookie is saved and your library appears.

**Or by editing the file:** add the cookie to `credentials.json` and restart `mixed`.

```json
{
    "youtube_cookie": "YOUR_COPIED_COOKIE_STRING_HERE"
}
```

`credentials.json` lives in the `mixed` configuration directory:

| Platform | Location |
|---|---|
| Linux | `~/.config/mixed/credentials.json` |
| macOS | `~/Library/Application Support/mixed/credentials.json` |
| Windows | `%APPDATA%\mixed\config\credentials.json` |

On Linux and macOS the file is written with owner-only permissions (`0600`).

### What the cookie is and is not used for

The cookie is used only for the YouTube Music API (search and your library). It is **not** passed to `yt-dlp`, so it does not change which tracks can be downloaded, the audio quality, or how YouTube rate-limits downloads.

## How playback works

1. Pressing `Enter` on a track adds it to the queue. When it becomes the current track, `mixed` starts `yt-dlp` in the background and shows `downloading…` in place of the progress bar.
2. Once the first ~190 KB have arrived (after `yt-dlp` has resolved the video, which takes a few seconds), playback starts from the partial file while the rest downloads.
3. When the download finishes, the file stays in the cache and later plays start immediately.

Details:

- **Formats:** only AAC (`.m4a`), MP3 and raw AAC audio is requested. WebM/Opus is not requested because the built-in decoder cannot play it.
- **Seeking while downloading:** you can seek within the part that has already arrived. Seeking further ahead moves to the furthest point available. After the download completes, seeking is unrestricted.
- **Prefetch:** when the current track passes 50%, the next track is downloaded in advance if it is a YouTube track.
- **Fallback:** if a partial file cannot be played as it arrives, `mixed` waits for the complete download instead.
- **Failures:** a track that fails to download is skipped. After three failures in a row, playback stops.
- **Lyrics:** not available for YouTube tracks (local files only).

## Configuration

Optional keys in `config.json` (same directory as `credentials.json`):

| Key | Default | Meaning |
|---|---|---|
| `yt_dlp_path` | not set | Full path to the `yt-dlp` binary, if it is not on your `PATH` |
| `yt_cache_mb` | `512` | Maximum size of the audio cache in megabytes |

### Cache

Downloaded audio is kept in the `yt` folder of the `mixed` cache directory (`~/.cache/mixed/yt/` on Linux). When the cache exceeds `yt_cache_mb`, the oldest files are deleted. This runs at startup and after each download. Deleting the folder by hand is safe.

Cached files are kept exactly as downloaded (`yt-dlp` runs with `--fixup never`, so that a file is never rewritten while it is playing). They play and seek correctly in `mixed`; some other players may report a wrong duration for them.

## Troubleshooting

- **Nothing plays, or downloads fail:** update `yt-dlp` first. The exact error from `yt-dlp` is shown in the status line.
- **Library is empty or shows "Not connected":** the cookie is missing or has expired. Copy a fresh one and log in again.
- **Details of an error:** `mixed` writes diagnostics to `mixed.log` in its cache directory (`~/.cache/mixed/mixed.log` on Linux).

## Privacy

The cookie gives full access to your YouTube session. It is stored only on your machine and sent only to YouTube Music. Do not share `credentials.json`.
