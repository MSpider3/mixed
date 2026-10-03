# YouTube Music Setup & Authentication

`mixed` streams YouTube Music tracks using `yt-dlp` in the background for fetching audio streams. While you can stream public tracks without authentication, many features (and stable streaming) require logging into YouTube Music by providing your session cookies.

## Why Authentication is Recommended
1. **Age-restricted content**: Some tracks cannot be played without verifying your age through a logged-in account.
2. **Rate Limits & Blocking**: YouTube frequently throttles or blocks IPs that stream anonymously. Using a valid session cookie bypasses these restrictions.
3. **Premium Audio Quality**: If you have YouTube Premium, logging in ensures you get the highest quality stream.

## How to Get Your Cookie
You need to extract the cookie header from an active YouTube Music session in your web browser.

1. Open a new tab in your web browser (Chrome, Firefox, Safari, Edge).
2. Go to [music.youtube.com](https://music.youtube.com) and log in to your account.
3. Open the **Developer Tools** (Press `F12` or `Ctrl+Shift+I` / `Cmd+Option+I`).
4. Navigate to the **Network** tab.
5. Refresh the page (`F5` or `Ctrl+R`).
6. Click on the first network request (usually named `music.youtube.com` or `browse`).
7. In the right pane, scroll down to the **Request Headers** section.
8. Find the header named `cookie:`. Right-click its value and select **Copy Value**.

## Adding the Cookie to `mixed`
`mixed` stores its secure credentials in `credentials.json`, which is located in your configuration directory (usually `~/.config/mixed/credentials.json` on Linux/macOS or `%APPDATA%\mixed\credentials.json` on Windows).

1. Open or create the `credentials.json` file.
2. Add your copied cookie string under the `youtube_cookie` field:
   ```json
   {
       "youtube_cookie": "YOUR_COPIED_COOKIE_STRING_HERE"
   }
   ```
   *(Ensure the JSON format is valid and the cookie is enclosed in double quotes.)*
3. Save the file and restart `mixed`.

Once configured, all YouTube Music requests (`Ctrl+3` / `Alt+3`) will securely use your session cookie.

## Important Notes
- **Cookie Expiration**: These cookies typically last for a few months but may eventually expire. If YouTube playback suddenly starts failing with errors, simply repeat this process to extract a fresh cookie.
- **Privacy**: The cookie file is stored locally on your machine and is never shared. It contains full access to your YouTube session, so keep it secure.
