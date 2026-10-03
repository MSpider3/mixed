# Spotify Authentication & Setup

`mixed` requires a **Spotify Premium** account to stream music directly using `librespot`. It authenticates against the Spotify Web API using the [OAuth 2.0 authorization code flow with PKCE](https://developer.spotify.com/documentation/web-api/tutorials/code-pkce-flow). No client secret is stored or required.

## The Simplest Way (Default Login)
The simplest way to authenticate is to run `mixed` and switch to the Spotify source (`Ctrl+2` or `Alt+2`).
1. On first use, it prompts for credentials.
2. It will open the Spotify authorization page in your browser.
3. After you approve access, Spotify redirects to a local loopback address (`http://127.0.0.1:8989/login`).
4. `mixed` captures the authorization code and exchanges it for an access token.

Credentials are cached in the application's configuration folder (e.g., `~/.config/mixed/credentials.json`), so this is a one-time step per machine.

## How Authentication Works
Two kinds of credentials are involved:
1. **Web API token**: Used for REST calls (playback control, library, search, playlists, etc.).
2. **librespot session**: Used for direct playback streaming and Spotify Connect device registration.

They authenticate through your Spotify account. By default, you might see two authorization flows: one for the Web API token and another for the librespot session. Each requires a separate approval on the first launch.

## Using a Custom Client ID (Highly Recommended)
Every request to the Spotify Web API is attributed to a Spotify application identified by a client ID. Using a shared default client ID can lead to `429 Too Many Requests` errors if the API quota is exhausted by multiple users.

Registering your own custom Client ID ensures your routine requests use a quota dedicated entirely to your setup.

### Step-by-Step Custom Client Setup:
1. Go to the [Spotify Developer Dashboard](https://developer.spotify.com/dashboard) and log in.
2. Click **Create App**.
3. Fill in the App name and description.
4. **Important**: Under **Redirect URIs**, add `http://127.0.0.1:8989/login`.
5. Save your application.
6. Navigate to your App's settings and copy the **Client ID**.
7. Open the `mixed` configuration file (usually `~/.config/mixed/mixed.toml`).
8. Add your custom Client ID:
   ```toml
   [spotify]
   client_id = "YOUR_CUSTOM_CLIENT_ID"
   ```
9. Restart `mixed` and navigate to the Spotify tab. It will prompt you to authenticate your new custom application via the browser.

*(Note: If you change your client ID in the future, you will need to re-authenticate.)*
