//! Ely.by authentication (Yggdrasil-compatible) and skin handling.
//!
//! Endpoints (see https://docs.ely.by/en/authentication.html):
//!   POST https://authserver.ely.by/auth/authenticate
//!   POST https://authserver.ely.by/auth/refresh
//!   POST https://authserver.ely.by/auth/validate
//!   POST https://authserver.ely.by/auth/invalidate
//!
//! In game we do NOT talk to those endpoints directly: authlib-injector is
//! attached with `-javaagent:authlib-injector.jar=ely.by`, which transparently
//! redirects Mojang's session/profile calls to Ely.by.

use crate::config::Account;
use crate::util::{agent, download};
use anyhow::{anyhow, bail, Result};
use serde::Deserialize;
use std::path::{Path, PathBuf};

pub const AUTH_SERVER: &str = "https://authserver.ely.by";
pub const API_ROOT: &str = "ely.by";
pub const AUTHLIB_INJECTOR_VERSION: &str = "1.2.8";
pub const AUTHLIB_INJECTOR_URL: &str =
    "https://github.com/yushijinhun/authlib-injector/releases/download/v1.2.8/authlib-injector-1.2.8.jar";
pub const AUTHLIB_INJECTOR_SHA1: Option<&str> = None;

#[derive(Debug, Deserialize)]
struct AuthProfile {
    id: String,
    name: String,
}

#[derive(Debug, Deserialize)]
struct AuthResponse {
    #[serde(rename = "accessToken")]
    access_token: String,
    #[serde(rename = "clientToken")]
    client_token: String,
    #[serde(rename = "selectedProfile")]
    selected_profile: Option<AuthProfile>,
    #[serde(rename = "availableProfiles")]
    available_profiles: Option<Vec<AuthProfile>>,
}

#[derive(Debug, Deserialize)]
struct AuthError {
    #[serde(default)]
    error: String,
    #[serde(rename = "errorMessage", default)]
    error_message: String,
}

pub struct LoginOutcome {
    pub account: Account,
    /// Set when Ely.by asks for a one-time 2FA code.
    pub needs_totp: bool,
}

/// Authenticate against Ely.by. `totp` is appended to the password as
/// `password:token`, which is how Ely.by expects two-factor codes.
pub fn login(login: &str, password: &str, totp: &str, client_token: &str) -> Result<LoginOutcome> {
    let login = login.trim();
    if login.is_empty() || password.is_empty() {
        bail!("Empty credentials");
    }

    let effective_password = if totp.trim().is_empty() {
        password.to_string()
    } else {
        format!("{}:{}", password, totp.trim())
    };

    let body = serde_json::json!({
        "username": login,
        "password": effective_password,
        "clientToken": client_token,
        "requestUser": true,
    });

    let response = agent()
        .post(&format!("{AUTH_SERVER}/auth/authenticate"))
        .set("Content-Type", "application/json")
        .send_json(body);

    let json: serde_json::Value = match response {
        Ok(resp) => resp.into_json()?,
        Err(ureq::Error::Status(_code, resp)) => {
            let err: AuthError = resp.into_json().unwrap_or(AuthError {
                error: "UnknownError".into(),
                error_message: "Unexpected response from Ely.by".into(),
            });
            // Ely.by signals a missing 2FA code with this exact error.
            let needs_totp = err.error_message.contains("two factor auth")
                || err.error_message.to_lowercase().contains("totp")
                || err.error == "ForbiddenOperationException"
                    && err.error_message.contains("Account protected with two factor auth");
            if needs_totp {
                return Ok(LoginOutcome {
                    account: Account {
                        id: String::new(),
                        username: String::new(),
                        uuid: String::new(),
                        kind: "ely".into(),
                        access_token: String::new(),
                        client_token: client_token.into(),
                        skin_url: String::new(),
                    },
                    needs_totp: true,
                });
            }
            return Err(anyhow!(friendly_error(&err)));
        }
        Err(e) => return Err(anyhow!("Network error: {e}")),
    };

    let auth: AuthResponse = serde_json::from_value(json)?;
    let profile = auth
        .selected_profile
        .or_else(|| auth.available_profiles.and_then(|mut v| if v.is_empty() { None } else { Some(v.remove(0)) }))
        .ok_or_else(|| anyhow!("Ely.by returned no Minecraft profile for this account"))?;

    Ok(LoginOutcome {
        account: Account {
            id: profile.id.clone(),
            username: profile.name,
            uuid: dashed_uuid(&profile.id),
            kind: "ely".into(),
            access_token: auth.access_token,
            client_token: auth.client_token,
            skin_url: String::new(),
        },
        needs_totp: false,
    })
}

/// Refresh a stored token; returns the new access token.
pub fn refresh(access_token: &str, client_token: &str) -> Result<String> {
    let body = serde_json::json!({
        "accessToken": access_token,
        "clientToken": client_token,
        "requestUser": false,
    });
    let json: serde_json::Value = agent()
        .post(&format!("{AUTH_SERVER}/auth/refresh"))
        .set("Content-Type", "application/json")
        .send_json(body)
        .map_err(|e| anyhow!("token refresh failed: {e}"))?
        .into_json()?;
    json.get("accessToken")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| anyhow!("no accessToken in refresh response"))
}

/// Cheap validity check for a stored token.
pub fn validate(access_token: &str, client_token: &str) -> bool {
    let body = serde_json::json!({
        "accessToken": access_token,
        "clientToken": client_token,
    });
    matches!(
        agent()
            .post(&format!("{AUTH_SERVER}/auth/validate"))
            .set("Content-Type", "application/json")
            .send_json(body),
        Ok(_)
    )
}

pub fn invalidate(access_token: &str, client_token: &str) {
    let body = serde_json::json!({
        "accessToken": access_token,
        "clientToken": client_token,
    });
    let _ = agent()
        .post(&format!("{AUTH_SERVER}/auth/invalidate"))
        .set("Content-Type", "application/json")
        .send_json(body);
}

/// Download authlib-injector into the launcher directory and return its path.
pub fn ensure_authlib_injector(data_dir: &Path) -> Result<PathBuf> {
    let dest = data_dir
        .join("libraries")
        .join("authlib-injector")
        .join(format!("authlib-injector-{AUTHLIB_INJECTOR_VERSION}.jar"));
    download(&agent(), AUTHLIB_INJECTOR_URL, &dest, AUTHLIB_INJECTOR_SHA1, None)?;
    Ok(dest)
}

/// Fetch the player's skin PNG (via the Ely.by skin system) so the launcher
/// can render a head icon. Returns raw PNG bytes.
pub fn fetch_skin(username: &str) -> Result<Vec<u8>> {
    let url = format!("http://skinsystem.ely.by/skins/{username}.png");
    let resp = agent().get(&url).call()?;
    let mut bytes = Vec::new();
    std::io::Read::read_to_end(&mut resp.into_reader(), &mut bytes)?;
    Ok(bytes)
}

fn friendly_error(err: &AuthError) -> String {
    let msg = err.error_message.trim();
    match msg {
        m if m.contains("Invalid credentials") => {
            "Неверный логин или пароль / Invalid login or password".into()
        }
        m if m.contains("Account protected with two factor auth") => {
            "Нужен код 2FA / Two-factor code required".into()
        }
        m if m.contains("banned") => "Аккаунт заблокирован / Account is banned".into(),
        m if m.is_empty() => format!("Ely.by error: {}", err.error),
        m => m.to_string(),
    }
}

pub fn dashed_uuid(raw: &str) -> String {
    let clean: String = raw.chars().filter(|c| c.is_ascii_alphanumeric()).collect();
    if clean.len() != 32 {
        return raw.to_string();
    }
    format!(
        "{}-{}-{}-{}-{}",
        &clean[0..8],
        &clean[8..12],
        &clean[12..16],
        &clean[16..20],
        &clean[20..32]
    )
}

/// Deterministic offline UUID (same scheme Mojang uses for cracked servers).
pub fn offline_uuid(username: &str) -> String {
    use sha1::{Digest, Sha1};
    // Mojang uses MD5 of "OfflinePlayer:<name>"; we approximate with a stable
    // hash so the same nickname always maps to the same UUID.
    let mut hasher = Sha1::new();
    hasher.update(format!("OfflinePlayer:{username}").as_bytes());
    let digest = hasher.finalize();
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    bytes[6] = (bytes[6] & 0x0f) | 0x30; // version 3
    bytes[8] = (bytes[8] & 0x3f) | 0x80; // RFC 4122 variant
    dashed_uuid(&hex::encode(bytes))
}
