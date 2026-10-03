//! Credential loading, validation, and in-memory session handling.
//!
//! Only `auth login` persists credentials; secrets are never logged and are
//! zeroized on drop. Session tokens are never persisted.

use std::fmt;
use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};

use md5::{Digest, Md5};
use zeroize::{Zeroize, ZeroizeOnDrop, Zeroizing};

use crate::error::{Error, Result};

/// Environment variable holding the account email address.
pub const ENV_EMAIL: &str = "DIVOOM_EMAIL";
/// Environment variable holding the hexadecimal MD5 digest of the password.
pub const ENV_PASSWORD_MD5: &str = "DIVOOM_PASSWORD_MD5";

/// Login credentials. The password is only ever held as its MD5 digest.
#[derive(Clone, serde::Serialize, serde::Deserialize, Zeroize, ZeroizeOnDrop)]
pub struct Credentials {
    pub email: String,
    pub password_md5: String,
}

impl fmt::Debug for Credentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Credentials")
            .field("email", &redact_email(&self.email))
            .field("password_md5", &"<redacted>")
            .finish()
    }
}

/// Session token as returned by the API: the live server returns an integer,
/// historical clients documented a string. It is echoed back unchanged.
#[derive(Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
pub enum Token {
    Number(i64),
    Text(String),
}

impl Token {
    pub fn is_empty(&self) -> bool {
        match self {
            Token::Number(_) => false,
            Token::Text(s) => s.is_empty(),
        }
    }
}

impl Zeroize for Token {
    fn zeroize(&mut self) {
        match self {
            Token::Number(n) => n.zeroize(),
            Token::Text(s) => s.zeroize(),
        }
    }
}

impl fmt::Debug for Token {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<redacted>")
    }
}

/// Authenticated session. The token lives in memory only.
#[derive(Clone, Zeroize, ZeroizeOnDrop)]
pub struct Session {
    #[zeroize(skip)]
    pub user_id: u64,
    pub token: Token,
}

impl fmt::Debug for Session {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Session")
            .field("user_id", &self.user_id)
            .field("token", &"<redacted>")
            .finish()
    }
}

/// Partially mask an email address for diagnostics (`ab***@example.com`).
pub fn redact_email(email: &str) -> String {
    match email.split_once('@') {
        Some((local, domain)) => {
            let shown: String = local.chars().take(2).collect();
            format!("{shown}***@{domain}")
        }
        None => "***".to_string(),
    }
}

/// Validate and normalize a hexadecimal MD5 digest: exactly 32 ASCII hex
/// characters, returned in lowercase.
pub fn normalize_md5_hex(value: &str) -> Result<String> {
    let trimmed = value.trim();
    if trimmed.len() != 32 || !trimmed.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(Error::Configuration(format!(
            "{ENV_PASSWORD_MD5} must be exactly 32 hexadecimal characters"
        )));
    }
    Ok(trimmed.to_ascii_lowercase())
}

/// Compute the lowercase hexadecimal MD5 digest of a plaintext password.
pub fn md5_hex(plaintext: &[u8]) -> String {
    let digest = Md5::digest(plaintext);
    let mut out = String::with_capacity(32);
    for b in digest {
        use std::fmt::Write;
        let _ = write!(out, "{b:02x}");
    }
    out
}

/// Resolve the XDG credentials location, falling back to `$HOME/.config`.
pub fn credentials_path() -> Result<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| {
            std::env::var_os("HOME")
                .map(PathBuf::from)
                .filter(|path| path.is_absolute())
                .map(|path| path.join(".config"))
        })
        .ok_or_else(|| {
            Error::Configuration("set XDG_CONFIG_HOME or HOME to an absolute path".into())
        })?;
    Ok(base.join("divoom-export/credentials.toml"))
}

/// Missing files are normal before the first login. Parser errors deliberately
/// omit TOML diagnostics, which can contain source text and credentials.
pub fn read_credentials(path: &Path) -> Result<Option<Credentials>> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => Zeroizing::new(text),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(Error::io(path, error)),
    };
    toml::from_str(&text).map(Some).map_err(|_| {
        Error::Configuration(format!(
            "invalid credentials file {}; run `divoom-export auth login` to replace it",
            path.display()
        ))
    })
}

/// Atomically replace the saved credentials using a private temporary file.
pub fn save_credentials(path: &Path, credentials: &Credentials) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| Error::Configuration("credentials path has no parent directory".into()))?;
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder
        .create(parent)
        .map_err(|error| Error::io(parent, error))?;
    let text = Zeroizing::new(
        toml::to_string(credentials)
            .map_err(|_| Error::Configuration("cannot serialize credentials".into()))?,
    );
    let mut file =
        tempfile::NamedTempFile::new_in(parent).map_err(|error| Error::io(parent, error))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.as_file()
            .set_permissions(std::fs::Permissions::from_mode(0o600))?;
    }
    file.write_all(text.as_bytes())?;
    file.as_file().sync_all()?;
    file.persist(path)
        .map_err(|error| Error::io(path, error.error))?;
    Ok(())
}

/// Remove only the credentials file; a repeated logout is a successful no-op.
pub fn remove_credentials(path: &Path) -> Result<bool> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(Error::io(path, error)),
    }
}

/// Environment variables override saved values, then interactive prompts fill
/// missing values. Complete environment credentials do not require a config file.
pub fn load_credentials() -> Result<Credentials> {
    load_credentials_inner(true)
}

/// Login ignores saved credentials so users can replace a stale password or
/// switch accounts, while retaining environment support for scripts.
pub fn load_login_credentials() -> Result<Credentials> {
    load_credentials_inner(false)
}

fn load_credentials_inner(use_saved: bool) -> Result<Credentials> {
    let env_value = |name| {
        std::env::var(name)
            .ok()
            .filter(|value| !value.trim().is_empty())
    };
    let email = env_value(ENV_EMAIL);
    let password_md5 = env_value(ENV_PASSWORD_MD5);
    let path = if use_saved && (email.is_none() || password_md5.is_none()) {
        Some(credentials_path()?)
    } else {
        None
    };
    resolve_credentials(
        email,
        password_md5,
        path.as_deref(),
        std::io::stdin().is_terminal(),
    )
}

fn resolve_credentials(
    email: Option<String>,
    password_md5: Option<String>,
    saved_path: Option<&Path>,
    interactive: bool,
) -> Result<Credentials> {
    let password_md5 = password_md5.map(Zeroizing::new);
    let saved = if email.is_none() || password_md5.is_none() {
        saved_path.map(read_credentials).transpose()?.flatten()
    } else {
        None
    };
    let email = email
        .as_deref()
        .or_else(|| saved.as_ref().map(|c| c.email.as_str()));
    let email = match email.filter(|value| !value.trim().is_empty()) {
        Some(value) => value.trim().to_string(),
        None if interactive => prompt_email()?,
        None => {
            return Err(Error::Configuration(format!(
                "{ENV_EMAIL} is not set, no saved email is available and stdin is not interactive; run `divoom-export auth login`"
            )))
        }
    };

    let password_md5 = password_md5
        .as_ref()
        .map(|value| value.as_str())
        .or_else(|| saved.as_ref().map(|c| c.password_md5.as_str()));
    let password_md5 = match password_md5.filter(|value| !value.trim().is_empty()) {
        Some(value) => normalize_md5_hex(value)?,
        None if interactive => prompt_password_md5()?,
        None => {
            return Err(Error::Configuration(format!(
                "{ENV_PASSWORD_MD5} is not set, no saved password is available and stdin is not interactive; run `divoom-export auth login`"
            )))
        }
    };

    Ok(Credentials {
        email,
        password_md5,
    })
}

fn prompt_email() -> Result<String> {
    let mut stderr = std::io::stderr();
    write!(stderr, "Divoom email: ").map_err(Error::from)?;
    stderr.flush().map_err(Error::from)?;
    let mut line = String::new();
    std::io::stdin().read_line(&mut line).map_err(Error::from)?;
    let email = line.trim().to_string();
    if email.is_empty() {
        return Err(Error::Configuration("empty email address".into()));
    }
    Ok(email)
}

fn prompt_password_md5() -> Result<String> {
    let mut plaintext = rpassword::prompt_password("Divoom password: ")
        .map_err(|e| Error::Configuration(format!("cannot read password: {e}")))?;
    if plaintext.is_empty() {
        plaintext.zeroize();
        return Err(Error::Configuration("empty password".into()));
    }
    let digest = md5_hex(plaintext.as_bytes());
    plaintext.zeroize();
    Ok(digest)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saved_credentials_round_trip_privately_and_logout_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config/credentials.toml");
        let credentials = Credentials {
            email: "o'brien+\"test\\@example.com".into(),
            password_md5: md5_hex(b"password"),
        };
        assert!(read_credentials(&path).unwrap().is_none());
        save_credentials(&path, &credentials).unwrap();
        let saved = read_credentials(&path).unwrap().unwrap();
        assert_eq!(saved.email, credentials.email);
        assert_eq!(saved.password_md5, credentials.password_md5);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
            assert_eq!(
                std::fs::metadata(path.parent().unwrap())
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o700
            );
        }
        assert!(remove_credentials(&path).unwrap());
        assert!(!remove_credentials(&path).unwrap());
    }

    #[test]
    fn environment_values_override_saved_credentials() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("credentials.toml");
        let saved = Credentials {
            email: "saved@example.com".into(),
            password_md5: md5_hex(b"saved"),
        };
        save_credentials(&path, &saved).unwrap();
        for (email, password) in [
            (None, None),
            (Some("env@example.com".to_string()), None),
            (None, Some(md5_hex(b"env").to_uppercase())),
        ] {
            let resolved =
                resolve_credentials(email.clone(), password.clone(), Some(&path), false).unwrap();
            assert_eq!(resolved.email, email.unwrap_or_else(|| saved.email.clone()));
            assert_eq!(
                resolved.password_md5,
                password
                    .unwrap_or_else(|| saved.password_md5.clone())
                    .to_lowercase()
            );
        }
        std::fs::write(&path, "invalid TOML containing a secret").unwrap();
        let error = resolve_credentials(None, None, Some(&path), false).unwrap_err();
        assert!(!error.to_string().contains("containing a secret"));
        assert!(resolve_credentials(
            Some("env@example.com".into()),
            Some(md5_hex(b"env")),
            Some(&path),
            false
        )
        .is_ok());
        // Login can replace a broken file without trying to read it.
        assert!(resolve_credentials(
            Some("env@example.com".into()),
            Some(md5_hex(b"env")),
            None,
            false
        )
        .is_ok());
    }

    #[test]
    fn md5_matches_known_vector() {
        assert_eq!(md5_hex(b"password"), "5f4dcc3b5aa765d61d8327deb882cf99");
    }

    #[test]
    fn normalizes_hex_case() {
        let v = normalize_md5_hex("5F4DCC3B5AA765D61D8327DEB882CF99").unwrap();
        assert_eq!(v, "5f4dcc3b5aa765d61d8327deb882cf99");
    }

    #[test]
    fn rejects_bad_hex() {
        assert!(normalize_md5_hex("abc").is_err());
        assert!(normalize_md5_hex("zz4dcc3b5aa765d61d8327deb882cf99").is_err());
    }

    #[test]
    fn debug_output_redacts_secrets() {
        let c = Credentials {
            email: "someone@example.com".into(),
            password_md5: "5f4dcc3b5aa765d61d8327deb882cf99".into(),
        };
        let s = format!("{c:?}");
        assert!(!s.contains("5f4dcc3b"));
        assert!(!s.contains("someone@"));
        let sess = Session {
            user_id: 1,
            token: Token::Text("supersecret".into()),
        };
        assert!(!format!("{sess:?}").contains("supersecret"));
        let sess = Session {
            user_id: 1,
            token: Token::Number(151_176_847),
        };
        assert!(!format!("{sess:?}").contains("151176847"));
    }
}
