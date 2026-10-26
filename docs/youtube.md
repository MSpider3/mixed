# YouTube Music Setup

`mixed` plays YouTube Music (`Ctrl+3` / `Alt+3`) with the help of three other programs:

| Piece | What it does | Who provides it |
|---|---|---|
| YouTube Music API (`ytmapi-rs`) | Search, browsing albums and playlists, your library | Built into `mixed` |
| [`yt-dlp`](https://github.com/yt-dlp/yt-dlp) | Downloads the audio that is played | You install it |
| A JavaScript runtime ([Deno](https://deno.com) or Node.js) | Lets `yt-dlp` run the code YouTube uses to hide the audio address | You install it |

What happens when you play a YouTube track:

1. `mixed` starts `yt-dlp` for the track.
2. `yt-dlp` asks YouTube for the track. If YouTube answers that it wants a signed-in user, `mixed` starts `yt-dlp` again with your cookie (see [What the cookie is used for](#what-the-cookie-is-used-for)).
3. `yt-dlp` runs YouTube's player code in the JavaScript runtime to work out the audio address, then downloads the audio into the cache.
4. `mixed` starts playing as soon as the first part of the file has arrived.

## Requirements

- `yt-dlp` must be installed. `mixed` looks for it on your `PATH`; to use a binary elsewhere, set `yt_dlp_path` in `config.json` (see [Configuration](#configuration)).
- A JavaScript runtime must be installed, and `yt-dlp` told how to use it. See [JavaScript runtime](#javascript-runtime).
- Keep `yt-dlp` up to date. When YouTube changes something, the fix arrives as a `yt-dlp` update, not a `mixed` update.

If `yt-dlp` is missing, playing a YouTube track shows `yt-dlp not found. Install it to play YouTube Music.`

## What works without logging in

- **Search** (`F5` or `/`): up to 20 songs, 20 albums and 20 playlists, in that order.
- **Opening** an album or playlist from the search results (`Enter` opens it, `Enter` again closes it).
- **Playback** of any track you can enqueue, unless YouTube asks downloads from your network to sign in. In that case playback needs the cookie too (see [What the cookie is used for](#what-the-cookie-is-used-for)).

Logging in adds your own library in the Library tab (`F3`): **Liked Songs**, **Playlists** and **Albums**.

## Signing in

1. Switch to YouTube Music (`Ctrl+3` / `Alt+3`) and open the Library tab (`F3`).
2. Press `Enter`. A browser window opens at Google's sign-in page.
3. Sign in to YouTube Music there (2-step verification and passkeys work as usual), then close the window.

Your library appears a moment later. `mixed` never sees your password: it keeps the cookies of the signed-in session, the same way a cookie copied by hand would, only without the copying.

### Why this sign-in lasts

The window belongs to a browser profile that only `mixed` uses, in its data directory (`~/.local/share/mixed/youtube-browser` on Linux). A cookie copied from the browser you use every day stops working within days, because that browser keeps receiving new cookies from Google, which replace the copied one. Nothing else uses `mixed`'s profile, so its cookies stay valid much longer.

If Google does end the session (after a password change, for example), `mixed` notices at start and the YouTube tab says the sign-in has expired. Press `Enter` again: the same window opens, usually still signed in, so closing it is enough.

The browser is the first of Brave, Chrome, Chromium, Vivaldi and Firefox found on your `PATH`. Set `youtube_browser` in `config.json` to choose another (a name on the `PATH` or the full path). Chromium-based browsers are started with `--password-store=basic`, so that `yt-dlp` can read the cookies of the profile without your desktop's keyring.

## Signing in with a copied cookie

Without a supported browser, or if you prefer it, `mixed` also signs in with the cookie of a browser session copied by hand. Copy it from a private window that you close afterwards: a cookie of a window that stays in use is replaced by the browser within days.

### 1. Copy the cookie

1. Open [music.youtube.com](https://music.youtube.com) in your browser and sign in.
2. Open the developer tools (`F12`, or `Ctrl+Shift+I` / `Cmd+Option+I`) and select the **Network** tab.
3. Reload the page.
4. Click a request to `music.youtube.com` (for example one named `browse`).
5. Under **Request Headers**, find `cookie:` and copy its whole value.

### 2. Give it to `mixed`

**In the app (recommended):**

1. Switch to YouTube Music (`Ctrl+3` / `Alt+3`) and open the Library tab (`F3`).
2. Press `c`. A `Credentials:` prompt appears.
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

### What the cookie is used for

The cookie is used for the YouTube Music API (search and your library).

Downloads normally run without it. If YouTube answers a download with `Sign in to confirm you're not a bot`, which it does for some networks, `mixed` runs `yt-dlp` again with your cookie and keeps using it for the downloads of that session. For this the cookie is written to `yt_cookies_….txt` next to `credentials.json` (owner-only permissions), a file `yt-dlp` reads and keeps up to date.

Downloading with your account's cookie is something YouTube can act on: the `yt-dlp` project warns that an account used this way may be restricted. A separate account for this is the safe choice.

## How playback works

1. Pressing `Enter` on a track adds it to the queue. When it becomes the current track, `mixed` asks `yt-dlp` for the URL of its audio (a few seconds) and shows `downloading…` in place of the progress bar.
2. `mixed` then plays that URL directly, the way YouTube Music apps do: it fetches the audio in pieces of 512 KB, starting where playback is, so the first sound comes after the first piece.
3. Once every piece is in, the file is saved to the cache and later plays start immediately.

Whenever a track starts, `mixed` also finds the audio URLs of the next three YouTube tracks in the queue. YouTube's URLs stay valid for hours, so skipping to one of those tracks starts without waiting for `yt-dlp`.

Details:

- **Album and cover art:** the Now Playing view shows a track's album name and its cover in full size (544 pixels). Videos in a playlist have no album. Tracks queued by a version of `mixed` from before album names were added show `Unknown Album` until they are removed and added again.
- **Formats:** only AAC (`.m4a`), MP3 and raw AAC audio is requested. WebM/Opus is not requested because the built-in decoder cannot play it.
- **Seeking:** seeking anywhere is immediate: the fetching moves to the new position and fills in the rest afterwards.
- **Prefetch:** when the current track passes 50%, the next track is downloaded in advance if it is a YouTube track.
- **Fallback:** if `yt-dlp` cannot give a URL, `mixed` downloads the track with `yt-dlp` instead and plays the partial file while it arrives (there, seeking is limited to the part that has arrived).
- **Failures:** a track that fails to download is skipped. After three failures in a row, playback stops.
- **Lyrics:** looked up online when the track starts. See [Lyrics](lyrics.md).

## Radio and Made for you

- **Radio:** `R` (Shift+R) on a YouTube track, in the queue, a library or search, or in the Now Playing view, queues YouTube Music's radio of that song (about 50 similar songs) right after the current track. It works without signing in.
- **Made for you:** a folder in the YouTube Library tab with **My Supermix** (made from your listening), **Moods & moments** (Chill, Focus, Workout, …) and **Genres**, each a list of YouTube Music's own playlists.

## Configuration

The YouTube settings are keys in `config.json`, which is in the same directory as `credentials.json` (`~/.config/mixed/config.json` on Linux):

| Key | Default | Meaning |
|---|---|---|
| `yt_dlp_path` | not set | Full path to the `yt-dlp` binary, if it is not on your `PATH` |
| `yt_dlp_args` | `[]` | Extra arguments passed to `yt-dlp` on every download, each word as its own string |
| `yt_cache_mb` | `512` | Maximum size of the audio cache in megabytes |
| `youtube_browser` | not set | Browser for the sign-in window (see [Signing in](#signing-in)) |

`mixed` creates the file with all its settings on first run. To change a YouTube setting, edit that line and leave the others as they are. With Deno installed, the file looks like this:

```json
{
  "music_dir": "/home/you/Music",
  "volume": 80,
  "visualizer_enabled": true,
  "visualizer_height": 5,
  "cover_enabled": true,
  "color_scheme": 0,
  "strip_track_numbers": true,
  "desktop_notifications": true,
  "spotify_client_id": null,
  "yt_dlp_path": null,
  "yt_dlp_args": ["--js-runtimes", "deno:/home/you/.deno/bin/deno", "--remote-components", "ejs:github"],
  "yt_cache_mb": 512,
  "yt_audio_quality": "bestaudio",
  "save_lyrics_next_to_songs": true,
  "youtube_browser": null
}
```

Edit the file while `mixed` is not running, because `mixed` writes its settings back to it when it exits. A version of `mixed` from before `yt_dlp_args` existed removes that line when it exits, so update `mixed` first.

### Cache

Downloaded audio is kept in the `yt` folder of the `mixed` cache directory (`~/.cache/mixed/yt/` on Linux). When the cache exceeds `yt_cache_mb`, the oldest files are deleted. This runs at startup and after each download. Deleting the folder by hand is safe.

Cached files are kept exactly as downloaded (`yt-dlp` runs with `--fixup never`, so that a file is never rewritten while it is playing). They play and seek correctly in `mixed`; some other players may report a wrong duration for them.

### JavaScript runtime

YouTube does not hand out the address of a track's audio directly. It sends player code that has to be run to work the address out, and `yt-dlp` needs a JavaScript runtime and a solver script for that. Without them a download fails and `mixed` shows `yt-dlp needs a JavaScript runtime for YouTube`.

Two runtimes are common. Either one works.

| | Deno | Node.js |
|---|---|---|
| What the code it runs may do | Nothing unless allowed: no files, no network | Everything your user account may do |
| Used by `yt-dlp` | Automatically, once installed | Only when enabled with `--js-runtimes node` |

**With Deno (recommended).** Install it with Deno's installer and make sure its folder is on your `PATH`:

```
curl -fsSL https://deno.land/install.sh | sh
deno --version
```

Then set in `config.json`, with the path the installer printed (`~/.deno/bin/deno` written out in full):

```json
"yt_dlp_args": ["--js-runtimes", "deno:/home/you/.deno/bin/deno", "--remote-components", "ejs:github"]
```

Naming the Deno binary makes this work wherever `mixed` is started from. Without the first two words `yt-dlp` only finds Deno if its folder is on the `PATH` of the program that started `mixed`, which is often not the case for a terminal opened before Deno was installed or for a desktop launcher.

**With Node.js.** If Node.js is already installed and you do not want Deno, set:

```json
"yt_dlp_args": ["--js-runtimes", "node", "--remote-components", "ejs:github"]
```

What the arguments mean:

- `--remote-components ejs:github` lets `yt-dlp` download its solver script from the `yt-dlp` project on GitHub. `yt-dlp` keeps a copy and does not download it for every track.
- `--js-runtimes deno:/path/to/deno` tells `yt-dlp` where the Deno binary is.
- `--js-runtimes node` lets `yt-dlp` use Node.js. It is off by default because Node.js runs YouTube's code without the restrictions Deno applies.

The [yt-dlp EJS guide](https://github.com/yt-dlp/yt-dlp/wiki/EJS) describes all options, including installing the solver script as a package so that nothing is downloaded at run time.

## Troubleshooting

- **Nothing plays, or downloads fail:** update `yt-dlp` first. The status line says what went wrong, and `mixed.log` has everything `yt-dlp` printed.
- **`yt-dlp needs a JavaScript runtime for YouTube`:** the runtime or the `yt_dlp_args` setting is missing, or `yt-dlp` cannot find the runtime. If Deno is installed, name its binary in `yt_dlp_args` as shown under [JavaScript runtime](#javascript-runtime).
- **A track takes 10 to 15 seconds to start:** that is the time `yt-dlp` needs to sign in and solve YouTube's challenge. A track that is already in the cache starts at once.
- **`YouTube asks to sign in: add your cookie in the YouTube Library tab`:** YouTube wants a signed-in session for downloads from your network. Sign in as described under [Signing in](#signing-in).
- **`YouTube rejected the saved cookie`** or **"Your YouTube Music sign-in has expired":** press `Enter` in the YouTube Library tab and sign in again.
- **Library is empty or shows "Not connected":** you are not signed in. Press `Enter` in the YouTube Library tab.
- **"The sign-in window is already open":** a window of `mixed`'s profile is still open. Finish there and close it, then press `Enter` again.
- **"Could not read the cookies of the sign-in window":** `yt-dlp` could not read the profile. `mixed.log` has `yt-dlp`'s explanation.
- **Details of an error:** `mixed` writes diagnostics to `mixed.log` in its cache directory (`~/.cache/mixed/mixed.log` on Linux).

## Privacy

The cookie gives full access to your YouTube session. It is stored only on your machine and sent only to YouTube: to YouTube Music by `mixed`, and to YouTube by `yt-dlp` when a download needs it. Do not share `credentials.json` or the `yt_cookies_….txt` file.
