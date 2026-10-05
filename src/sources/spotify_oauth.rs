//! Spotify sign-in: OAuth 2.0 authorization code flow with PKCE.

use oauth2::basic::BasicClient;
use oauth2::{
    AuthUrl, AuthorizationCode, ClientId, CsrfToken, EndpointNotSet, EndpointSet,
    PkceCodeChallenge, RedirectUrl, RefreshToken, Scope, TokenResponse, TokenUrl,
};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};

const AUTHORIZE_URL: &str = "https://accounts.spotify.com/authorize";
const TOKEN_URL: &str = "https://accounts.spotify.com/api/token";

/// How long the user has to approve the sign-in in the browser.
const LOGIN_TIMEOUT: Duration = Duration::from_secs(180);

/// How long a connection to the callback server may stay silent.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

type PkceClient =
    BasicClient<EndpointSet, EndpointNotSet, EndpointNotSet, EndpointNotSet, EndpointSet>;

pub struct Token {
    pub access_token: String,
    /// Spotify may omit this when refreshing; the previous refresh token then stays valid.
    pub refresh_token: Option<String>,
}

fn oauth_client(client_id: &str) -> Result<PkceClient, String> {
    Ok(BasicClient::new(ClientId::new(client_id.to_string()))
        .set_auth_uri(AuthUrl::new(AUTHORIZE_URL.to_string()).map_err(|e| e.to_string())?)
        .set_token_uri(TokenUrl::new(TOKEN_URL.to_string()).map_err(|e| e.to_string())?))
}

fn into_token(resp: impl TokenResponse) -> Token {
    Token {
        access_token: resp.access_token().secret().clone(),
        refresh_token: resp
            .refresh_token()
            .map(|t| t.secret().clone())
            .filter(|t| !t.is_empty()),
    }
}

/// Sign in through the user's browser and return the tokens Spotify issues.
/// `redirect_uri` must be an `http://127.0.0.1:<port>/...` address: the redirect
/// is received by a short-lived local server on that port.
pub async fn authorize(
    client_id: &str,
    redirect_uri: &str,
    scopes: &[&str],
) -> Result<Token, String> {
    let redirect = RedirectUrl::new(redirect_uri.to_string()).map_err(|e| e.to_string())?;
    let addr = redirect
        .url()
        .socket_addrs(|| None)
        .ok()
        .and_then(|addrs| addrs.into_iter().next())
        .ok_or_else(|| format!("Invalid Spotify redirect URI: {}", redirect_uri))?;
    let client = oauth_client(client_id)?.set_redirect_uri(redirect);

    // Listen before the browser opens, so the redirect never meets a closed port.
    let listener = TcpListener::bind(addr)
        .await
        .map_err(|e| format!("Cannot listen on {} for the Spotify sign-in: {}", addr, e))?;

    let (challenge, verifier) = PkceCodeChallenge::new_random_sha256();
    let (url, state) = client
        .authorize_url(CsrfToken::new_random)
        .add_scopes(scopes.iter().map(|s| Scope::new(s.to_string())))
        .set_pkce_challenge(challenge)
        .url();

    // The log is the only place the link can be copied from while the TUI is up.
    log::info!("Spotify sign-in URL: {}", url);
    if let Err(e) = open::that_detached(url.as_str()) {
        log::warn!("Could not open a browser for the Spotify sign-in: {}", e);
    }

    let code = tokio::time::timeout(LOGIN_TIMEOUT, wait_for_code(&listener, state.secret()))
        .await
        .map_err(|_| "Spotify sign-in timed out waiting for the browser".to_string())??;
    drop(listener);

    let http = reqwest::Client::new();
    let resp = client
        .exchange_code(AuthorizationCode::new(code))
        .set_pkce_verifier(verifier)
        .request_async(&http)
        .await
        .map_err(|e| format!("Spotify token exchange failed: {}", e))?;
    Ok(into_token(resp))
}

/// Exchange a refresh token for a new access token.
pub async fn refresh(client_id: &str, refresh_token: &str) -> Result<Token, String> {
    let http = reqwest::Client::new();
    let resp = oauth_client(client_id)?
        .exchange_refresh_token(&RefreshToken::new(refresh_token.to_string()))
        .request_async(&http)
        .await
        .map_err(|e| format!("Spotify token refresh failed: {}", e))?;
    Ok(into_token(resp))
}

/// Accept connections until one of them is Spotify's redirect. Browsers also open
/// idle and favicon connections to the callback port; those are answered and ignored.
async fn wait_for_code(listener: &TcpListener, state: &str) -> Result<String, String> {
    let (tx, mut rx) = tokio::sync::mpsc::channel(1);
    loop {
        tokio::select! {
            Some(result) = rx.recv() => return result,
            accepted = listener.accept() => {
                let Ok((stream, _)) = accepted else { continue };
                let tx = tx.clone();
                let state = state.to_string();
                tokio::spawn(async move {
                    if let Some(result) = serve_callback(stream, &state).await {
                        let _ = tx.send(result).await;
                    }
                });
            }
        }
    }
}

async fn serve_callback(stream: TcpStream, state: &str) -> Option<Result<String, String>> {
    let mut stream = BufReader::new(stream);
    let mut request_line = String::new();
    tokio::time::timeout(REQUEST_TIMEOUT, stream.read_line(&mut request_line))
        .await
        .ok()?
        .ok()?;

    let result = parse_callback(&request_line, state);
    let (status, body) = match &result {
        Some(Ok(_)) => (
            "200 OK",
            "Spotify sign-in approved. You can close this tab and return to mixed.",
        ),
        Some(Err(_)) => (
            "200 OK",
            "Spotify sign-in was not approved. You can close this tab.",
        ),
        None => ("404 Not Found", ""),
    };
    let response = format!(
        "HTTP/1.1 {}\r\ncontent-type: text/plain; charset=utf-8\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
        status,
        body.len(),
        body
    );
    let _ = stream.write_all(response.as_bytes()).await;
    result
}

/// Read the outcome of the sign-in from an HTTP request line such as
/// `GET /login?code=...&state=... HTTP/1.1`. `None` if it is not Spotify's redirect.
fn parse_callback(request_line: &str, state: &str) -> Option<Result<String, String>> {
    let target = request_line.split_whitespace().nth(1)?;
    let url = reqwest::Url::parse(&format!("http://localhost{}", target)).ok()?;
    let param = |name: &str| {
        url.query_pairs()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.into_owned())
    };
    if param("state").as_deref() != Some(state) {
        return None;
    }
    if let Some(error) = param("error") {
        return Some(Err(format!("Spotify sign-in was not approved ({})", error)));
    }
    param("code").map(Ok)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_callback_reads_the_code_of_the_redirect() {
        assert_eq!(
            parse_callback("GET /login?code=abc123&state=xyz HTTP/1.1\r\n", "xyz"),
            Some(Ok("abc123".to_string()))
        );
    }

    #[test]
    fn parse_callback_reports_a_denied_sign_in() {
        let result = parse_callback("GET /login?error=access_denied&state=xyz HTTP/1.1", "xyz");
        assert!(matches!(result, Some(Err(e)) if e.contains("access_denied")));
    }

    #[test]
    fn parse_callback_ignores_requests_that_are_not_the_redirect() {
        assert_eq!(parse_callback("GET /favicon.ico HTTP/1.1", "xyz"), None);
        assert_eq!(parse_callback("GET /login HTTP/1.1", "xyz"), None);
        assert_eq!(parse_callback("\r\n", "xyz"), None);
        // A code that does not carry this sign-in's state belongs to another attempt
        assert_eq!(
            parse_callback("GET /login?code=abc&state=other HTTP/1.1", "xyz"),
            None
        );
    }

    #[tokio::test]
    async fn wait_for_code_survives_stray_connections() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let browser = tokio::spawn(async move {
            // A connection that never sends anything, as browsers open speculatively
            let _idle = TcpStream::connect(addr).await.unwrap();
            let mut favicon = TcpStream::connect(addr).await.unwrap();
            favicon
                .write_all(b"GET /favicon.ico HTTP/1.1\r\n\r\n")
                .await
                .unwrap();
            let mut redirect = TcpStream::connect(addr).await.unwrap();
            redirect
                .write_all(b"GET /login?code=the-code&state=xyz HTTP/1.1\r\n\r\n")
                .await
                .unwrap();
            let mut reply = String::new();
            let _ = BufReader::new(redirect).read_line(&mut reply).await;
            reply
        });

        let code = tokio::time::timeout(Duration::from_secs(5), wait_for_code(&listener, "xyz"))
            .await
            .expect("the redirect is found behind the stray connections");
        assert_eq!(code, Ok("the-code".to_string()));
        assert!(browser.await.unwrap().starts_with("HTTP/1.1 200 OK"));
    }
}
