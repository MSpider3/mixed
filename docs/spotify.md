# Spotify Setup & Authentication

`mixed` plays Spotify (`Ctrl+2` / `Alt+2`) by acting as its own Spotify device through `librespot`, and uses the Spotify Web API for search and your library.

## Requirements

- A **Spotify Premium** account. Without Premium, sign-in ends with `Spotify Premium is required.` and the source stays disconnected.
- Your **own Client ID** from the Spotify Developer Dashboard. `mixed` does not ship a shared one: Spotify limits a development-mode app to a handful of users.

No client secret is needed. `mixed` uses the [OAuth 2.0 authorization code flow with PKCE](https://developer.spotify.com/documentation/web-api/tutorials/code-pkce-flow).

## 1. Create a Client ID

1. Go to the [Spotify Developer Dashboard](https://developer.spotify.com/dashboard) and log in.
2. Click **Create App** and fill in a name and description.
3. Under **Redirect URIs**, add exactly:
   ```
   http://127.0.0.1:8898/login
   ```
4. Save, open the app's settings and copy the **Client ID**.

## 2. Sign in from `mixed`

1. Switch to Spotify (`Ctrl+2` / `Alt+2`) and open the Library tab (`F3`).
2. Press `Enter`. A `Credentials:` prompt appears.
3. Paste your Client ID and press `Enter`. (`Esc` cancels, `Ctrl+U` clears the input.)
4. Your browser opens Spotify's authorization page. Approve access.
5. Spotify redirects to `http://127.0.0.1:8898/login`, where `mixed` is listening. It exchanges the code for tokens and checks that the account has Premium.

When this succeeds, the Library tab shows **Liked Songs**, **Playlists** and **Albums**. This is a one-time step per machine; there is only one approval.

The permissions requested are: reading and controlling playback, streaming, reading your private and collaborative playlists, and reading your library.

## What is stored

Credentials are kept in `credentials.json` in the `mixed` configuration directory:

| Platform | Location |
|---|---|
| Linux | `~/.config/mixed/credentials.json` |
| macOS | `~/Library/Application Support/mixed/credentials.json` |
| Windows | `%APPDATA%\mixed\config\credentials.json` |

```json
{
    "spotify_client_id": "YOUR_CLIENT_ID",
    "spotify_token_cache": "{\"access_token\":\"…\",\"refresh_token\":\"…\"}"
}
```

On Linux and macOS the file is written with owner-only permissions (`0600`). You do not need to edit it by hand.

`librespot` also keeps its own login and audio cache in the `spotify` folder of the `mixed` cache directory (`~/.cache/mixed/spotify/` on Linux).

## How authentication works

Two things are authenticated from the single sign-in:

1. **Web API token** — used for search and for browsing your library. Access tokens last one hour. `mixed` refreshes the token automatically (at startup and before a request when the token is older than 45 minutes) and saves the new tokens.
2. **`librespot` session** — used for audio. It connects the first time you play a Spotify track, using its own saved login if there is one and otherwise the Web API access token. The first track therefore takes a little longer to start; if the connection cannot be made within 15 seconds the track fails with `Spotify connection failed`.

## Using Spotify in `mixed`

- **Library (`F3`):** `Enter` on a folder, playlist or album opens it; `Enter` again closes it. `Enter` on a track adds it to the queue (or removes it if it is already queued), `Alt+Enter` plays it now, `Shift+Enter` queues it next.
- **Search (`F5` or `/`):** results arrive as you type and contain tracks, albums and playlists. Spotify returns at most 10 results of each kind per request.
- **Queue:** Spotify tracks sit in the same queue as local and YouTube tracks, and are restored with the queue on the next launch.
- **Progress and seeking:** elapsed time follows the position reported by Spotify, so it stays at zero while a track is still buffering.
- **Unavailable tracks:** a track Spotify cannot play is skipped. After three failures in a row, playback stops.
- **Not signed in:** playing a queued Spotify track while disconnected stops playback and shows `Spotify not connected. Switch to Spotify (Ctrl+2/Alt+2) to log in.` The track stays in the queue.
- **Lyrics:** not available for Spotify tracks (local files only).

Podcast episodes and local files inside a Spotify playlist are not listed.

## Troubleshooting

- **The browser shows "INVALID_CLIENT: Invalid redirect URI":** the Redirect URI in your Spotify app does not match `http://127.0.0.1:8898/login` exactly.
- **Sign-in never completes:** make sure nothing else is using port `8898`, then press `Enter` in the Library tab to try again.
- **`Search failed: HTTP 429`:** Spotify is rate-limiting your Client ID. A rate-limited search is retried once automatically; if it still fails, wait a moment and search again.
- **Signed in before but now "Not connected":** the saved tokens were rejected. Sign in again from the Library tab.
- **Changing the Client ID:** sign in again with the new one; the old tokens no longer apply.
- **Details of an error:** `mixed` writes diagnostics to `mixed.log` in its cache directory (`~/.cache/mixed/mixed.log` on Linux).

## Building without Spotify

Spotify support is a Cargo feature that is on by default. `cargo build --release --no-default-features --features youtube` builds `mixed` without it.

## A note on `librespot`

`librespot` is an open-source, reverse-engineered Spotify client library, not an official Spotify SDK. Search and library browsing use the official Web API.
