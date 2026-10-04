use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

/// Secure credentials storage (stored separately from user configuration in credentials.json).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Credentials {
    /// Spotify personal client ID
    #[serde(default)]
    pub spotify_client_id: Option<String>,
    /// Spotify cached token / refresh data
    #[serde(default)]
    pub spotify_token_cache: Option<String>,
    /// Spotify username (optional, for streaming if OAuth token is restricted)
    #[serde(default)]
    pub spotify_username: Option<String>,
    /// Spotify password (optional, for streaming)
    #[serde(default)]
    pub spotify_password: Option<String>,
    /// YouTube Music session cookie
    #[serde(default)]
    pub youtube_cookie: Option<String>,
}

/// Returns the path to `credentials.json`.
pub fn credentials_path() -> PathBuf {
    if let Some(override_path) = std::env::var_os("MIXED_CREDENTIALS_PATH") {
        return PathBuf::from(override_path);
    }
    #[cfg(test)]
    {
        std::env::temp_dir().join("mixed_test_credentials.json")
    }
    #[cfg(not(test))]
    {
        let is_test = std::env::var_os("MIXED_TEST").is_some()
            || std::env::current_exe()
                .ok()
                .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
                .map(|name| {
                    name.starts_with("mixed-")
                        || name.contains("test_")
                        || name == "tests"
                        || name.contains("headless")
                        || name.contains("mpris")
                })
                .unwrap_or(false);

        if is_test {
            std::env::temp_dir().join("mixed_test_credentials.json")
        } else if let Some(dir) = directories::ProjectDirs::from("", "", "mixed") {
            let path = dir.config_dir().to_path_buf();
            let _ = fs::create_dir_all(&path);
            path.join("credentials.json")
        } else {
            let user = std::env::var("USER").unwrap_or_else(|_| "default".to_string());
            let fallback_dir = std::env::temp_dir().join(format!("mixed-{}", user));
            let _ = fs::create_dir_all(&fallback_dir);
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = fs::set_permissions(&fallback_dir, fs::Permissions::from_mode(0o700));
            }
            fallback_dir.join("credentials.json")
        }
    }
}

impl Credentials {
    /// Load credentials from disk returning a detailed error if JSON is malformed.
    pub fn load_result() -> Result<Self, String> {
        let path = credentials_path();
        if path.exists() {
            let data = fs::read_to_string(&path)
                .map_err(|e| format!("Failed to read {}: {}", path.display(), e))?;
            let creds = serde_json::from_str(&data)
                .map_err(|e| format!("Invalid JSON in {}: {}", path.display(), e))?;
            return Ok(creds);
        }
        Ok(Self::default())
    }

    /// Load credentials from disk, or return default and log if nonexistent or corrupted.
    pub fn load() -> Self {
        match Self::load_result() {
            Ok(creds) => creds,
            Err(e) => {
                log::error!("{}", e);
                eprintln!("[mixed] Warning: {}", e);
                Self::default()
            }
        }
    }

    /// Save credentials to disk with mode 0600 permissions using atomic rename.
    pub fn save(&self) -> std::io::Result<()> {
        let path = credentials_path();
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = fs::set_permissions(parent, fs::Permissions::from_mode(0o700));
            }
        }

        let tmp_path = path.with_extension("tmp");
        let json = serde_json::to_string_pretty(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;

        #[cfg(unix)]
        {
            use std::io::Write;
            use std::os::unix::fs::OpenOptionsExt;
            let mut file = fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .mode(0o600)
                .open(&tmp_path)?;
            file.write_all(json.as_bytes())?;
            file.flush()?;
        }

        #[cfg(not(unix))]
        {
            fs::write(&tmp_path, json.as_bytes())?;
        }

        fs::rename(&tmp_path, &path)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_credentials_roundtrip_and_mode() {
        let path = credentials_path();
        let _ = fs::remove_file(&path);

        let mut creds = Credentials::load();
        assert_eq!(creds.spotify_client_id, None);
        assert_eq!(creds.youtube_cookie, None);

        creds.spotify_client_id = Some("test_client_id_123".into());
        creds.youtube_cookie = Some("VISITOR_INFO1_LIVE=abc; HSID=def;".into());
        assert!(creds.save().is_ok());

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let metadata = fs::metadata(&path).expect("File must exist");
            let mode = metadata.permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "credentials.json must have mode 0600");
        }

        let loaded = Credentials::load();
        assert_eq!(loaded, creds);

        let _ = fs::remove_file(&path);
    }
}
