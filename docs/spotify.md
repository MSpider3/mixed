# Spotify Setup & Authentication

`mixed` plays Spotify (`Ctrl+2` / `Alt+2`) by acting as its own Spotify device through `librespot`, and uses the Spotify Web API for search and your library.

## Requirements

- A **Spotify Premium** account. Without Premium, sign-in ends with `Spotify Premium is required.` and the source stays disconnected.

No client secret or developer account is required. `mixed` uses the [OAuth 2.0 authorization code flow with PKCE](https://developer.spotify.com/documentation/web-api/tutorials/code-pkce-flow).

## Quick Start: One-Click Sign In

1. Switch to Spotify (`Ctrl+2` / `Alt+2`) and open the Library tab (`F3`).
2. Press `Enter`.
3. Your browser opens Spotify's authorization page for library access. Click **Agree**.
4. A second authorization page opens, for playback (it is shown as Spotify's own desktop app). Click **Agree** again.

The Library tab shows which of the two steps `mixed` is waiting for. Each one must be approved within three minutes. If one fails, press `Enter` again: a step that already succeeded is not repeated.

Your library (Liked Songs, Recently Played, Top Tracks, full Playlists without 500-track limits, and Albums) will immediately appear and songs are ready to stream.

### (Optional) Using a Personal Developer Client ID

If you prefer to use your own personal Spotify Developer Client ID:
1. Go to the [Spotify Developer Dashboard](https://developer.spotify.com/dashboard) and create an app.
2. Under **Redirect URIs**, add:
   ```
   http://127.0.0.1:8898/login
   ```
3. In the Spotify Library tab (`F3`) press `c`, type or paste your Client ID at the `Credentials:` prompt and press `Enter` (`Esc` cancels).

This also works while you are signed in, for example when the built-in Client ID is being rate-limited: only the library approval is repeated. Entering `default` at the prompt switches back to the built-in Client ID.

Apps you create yourself run in Spotify's Development Mode, which offers a smaller Web API than the built-in Client ID, so Spotify may refuse some requests that work with the built-in one.

## What is stored

Credentials are kept in `credentials.json` in the `mixed` configuration directory:

| Platform | Location |
|---|---|
| Linux | `~/.config/mixed/credentials.json` |
| macOS | `~/Library/Application Support/mixed/credentials.json` |
| Windows | `%APPDATA%\mixed\config\credentials.json` |

```json
{
    "spotify_client_id": "d420a117a32841c2b3474932e49fb54b",
    "spotify_token_cache": "{\"access_token\":\"…\",\"refresh_token\":\"…\"}"
}
```

On Linux and macOS the file is written with owner-only permissions (`0600`). You do not need to edit it by hand.

`librespot` keeps the playback login (`credentials.json`) and up to 1 GB of audio (`files/`) in the `spotify` folder of the `mixed` cache directory (`~/.cache/mixed/spotify/` on Linux).

## How authentication works

Search and library browsing go through the Web API; audio goes through `librespot`. Spotify serves the two to different client IDs, so signing in takes two approvals (other `librespot` players such as ncspot and spotify-player work the same way):

1. **Web API token** — issued to the client ID that ncspot, spotify-player and spotatui share, or to your own. Access tokens last one hour. `mixed` refreshes the token automatically (at startup and before a request when the token is older than 45 minutes) and saves the new tokens.
2. **`librespot` login** — issued to Spotify's desktop client ID. At sign-in `librespot` exchanges it for a reusable login that does not expire hourly. The session connects with it the first time you play a Spotify track, so that track takes a little longer to start; if the connection cannot be made within 15 seconds the track fails with `Spotify connection failed`.

## Using Spotify in `mixed`

- **Library (`F3`):** `Enter` on a folder, playlist or album opens it; `Enter` again closes it. `Enter` on a track adds it to the queue (or removes it if it is already queued), `Alt+Enter` plays it now, `Shift+Enter` queues it next.
- **Search (`F5` or `/`):** results arrive as you type and contain tracks, albums and playlists. Spotify returns at most 10 results of each kind per request.
- **Queue:** Spotify tracks sit in the same queue as local and YouTube tracks, and are restored with the queue on the next launch.
- **Album and cover art:** the Now Playing view shows the album name and cover of a Spotify track. Tracks queued by a version of `mixed` from before this was added show `Unknown Album` until they are removed and added again.
- **Progress and seeking:** elapsed time follows the position reported by Spotify, so it stays at zero while a track is still buffering.
- **Tracks that fail to load:** a track is loaded up to three times before it is skipped. After three skipped tracks in a row, playback stops.
- **Not signed in:** playing a queued Spotify track while disconnected stops playback and shows `Spotify not connected. Switch to Spotify (Ctrl+2/Alt+2) to log in.` The track stays in the queue.
- **Lyrics:** Spotify's own lyrics are shown when it has them, otherwise lyrics from other services. See [Lyrics](lyrics.md).

Podcast episodes and local files inside a Spotify playlist are not listed.

## Troubleshooting

- **The browser shows "INVALID_CLIENT: Invalid redirect URI":** the Redirect URI in your Spotify app does not match `http://127.0.0.1:8898/login` exactly.
- **Sign-in never completes:** make sure nothing else is using port `8989` (`8898` with your own Client ID), then press `Enter` in the Library tab to try again. If no browser opens, copy the `Spotify sign-in URL` line from `mixed.log` into a browser on the same machine.
- **`Spotify is rate limiting requests. Try again in N s.`:** Spotify is rate-limiting the Client ID, which for the built-in one is shared with other players. Waits of up to 30 seconds are handled automatically; longer ones are reported like this.
- **`Spotify could not load the track (unavailable, or the connection to Spotify timed out)`:** either the track is not playable for your account or region, or Spotify's audio servers did not answer in time. `librespot` waits only 1.5 seconds for a track's decryption key, so a connection that loses packets on the way to those servers causes this even when browsing works. `mixed.log` tells the two apart: `Audio key response timeout` means the connection.
- **Signed in before but now "Not connected":** the saved tokens were rejected. Sign in again from the Library tab.
- **Changing the Client ID:** press `c` in the Library tab and sign in with the new one; the old tokens no longer apply.
- **`Playback sign-in is no longer valid`:** Spotify rejected the stored playback login. Press `Enter` in the Library tab; only the playback approval is repeated.
- **Details of an error:** `mixed` writes diagnostics to `mixed.log` in its cache directory (`~/.cache/mixed/mixed.log` on Linux), including the response Spotify gave to a failed request.

## Building without Spotify

Spotify support is a Cargo feature that is on by default. `cargo build --release --no-default-features --features youtube` builds `mixed` without it.

## A note on `librespot`

`librespot` is an open-source, reverse-engineered Spotify client library, not an official Spotify SDK. Search and library browsing use the official Web API.
