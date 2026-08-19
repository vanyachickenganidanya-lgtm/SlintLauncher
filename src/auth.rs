use serde::Deserialize;
use serde_json::json;

use crate::config::{Account, AccountKind};
use crate::error::{Error, Result};
use crate::mojang::USER_AGENT;

/// Public Azure application used by several open-source launchers.
/// Override with SLINTLAUNCHER_MSA_CLIENT_ID if you register your own.
const DEFAULT_CLIENT_ID: &str = "1ce6e35a-126f-48fd-97fb-54d143ac6d45";

#[derive(Debug, Clone)]
pub struct DeviceCode {
    pub user_code: String,
    pub device_code: String,
    pub verification_uri: String,
    pub interval: u64,
    pub expires_in: u64,
}

#[derive(Debug, Deserialize)]
struct DeviceCodeResponse {
    user_code: String,
    device_code: String,
    verification_uri: Option<String>,
    verification_url: Option<String>,
    interval: Option<u64>,
    expires_in: Option<u64>,
    error: Option<String>,
    error_description: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: Option<String>,
    refresh_token: Option<String>,
    expires_in: Option<i64>,
    error: Option<String>,
    error_description: Option<String>,
}

#[derive(Debug, Deserialize)]
struct XboxResponse {
    #[serde(rename = "Token")]
    token: String,
    #[serde(rename = "DisplayClaims")]
    display_claims: XboxClaims,
}

#[derive(Debug, Deserialize)]
struct XboxClaims {
    xui: Vec<XboxUser>,
}

#[derive(Debug, Deserialize)]
struct XboxUser {
    uhs: String,
    xid: Option<String>,
}

#[derive(Debug, Deserialize)]
struct McLogin {
    access_token: String,
    expires_in: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct McProfile {
    id: String,
    name: String,
}

pub fn client_id() -> String {
    std::env::var("SLINTLAUNCHER_MSA_CLIENT_ID").unwrap_or_else(|_| DEFAULT_CLIENT_ID.into())
}

pub async fn start_device_code(http: &reqwest::Client) -> Result<DeviceCode> {
    let resp = http
        .post("https://login.microsoftonline.com/consumers/oauth2/v2.0/devicecode")
        .header("User-Agent", USER_AGENT)
        .form(&[
            ("client_id", client_id()),
            ("scope", "XboxLive.signin offline_access".into()),
        ])
        .send()
        .await?;
    let body: DeviceCodeResponse = resp.json().await?;
    if let Some(err) = body.error {
        return Err(Error::Auth(
            body.error_description.unwrap_or(err),
        ));
    }
    Ok(DeviceCode {
        user_code: body.user_code,
        device_code: body.device_code,
        verification_uri: body
            .verification_uri
            .or(body.verification_url)
            .unwrap_or_else(|| "https://www.microsoft.com/link".into()),
        interval: body.interval.unwrap_or(5).max(1),
        expires_in: body.expires_in.unwrap_or(900),
    })
}

pub async fn poll_device_code(
    http: &reqwest::Client,
    device: &DeviceCode,
) -> Result<Option<Account>> {
    let resp = http
        .post("https://login.microsoftonline.com/consumers/oauth2/v2.0/token")
        .form(&[
            ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
            ("client_id", &client_id()),
            ("device_code", &device.device_code),
        ])
        .send()
        .await?;
    let token: TokenResponse = resp.json().await?;
    match token.error.as_deref() {
        Some("authorization_pending") | Some("slow_down") => return Ok(None),
        Some(other) => {
            return Err(Error::Auth(
                token.error_description.unwrap_or_else(|| other.to_string()),
            ))
        }
        None => {}
    }
    let access = token
        .access_token
        .ok_or_else(|| Error::Auth("нет access_token".into()))?;
    let refresh = token.refresh_token.unwrap_or_default();
    let expires = token.expires_in.unwrap_or(3600);
    let account = xbox_to_minecraft(http, &access, &refresh, expires).await?;
    Ok(Some(account))
}

pub async fn refresh_account(http: &reqwest::Client, account: &Account) -> Result<Account> {
    if account.kind != AccountKind::Microsoft || account.refresh_token.is_empty() {
        return Ok(account.clone());
    }
    let resp = http
        .post("https://login.microsoftonline.com/consumers/oauth2/v2.0/token")
        .form(&[
            ("grant_type", "refresh_token"),
            ("client_id", &client_id()),
            ("refresh_token", &account.refresh_token),
            ("scope", "XboxLive.signin offline_access"),
        ])
        .send()
        .await?;
    let token: TokenResponse = resp.json().await?;
    if let Some(err) = token.error {
        return Err(Error::Auth(token.error_description.unwrap_or(err)));
    }
    let access = token
        .access_token
        .ok_or_else(|| Error::Auth("refresh не вернул токен".into()))?;
    let refresh = token
        .refresh_token
        .unwrap_or_else(|| account.refresh_token.clone());
    xbox_to_minecraft(http, &access, &refresh, token.expires_in.unwrap_or(3600)).await
}

async fn xbox_to_minecraft(
    http: &reqwest::Client,
    msa_token: &str,
    refresh: &str,
    _msa_expires: i64,
) -> Result<Account> {
    let xbox: XboxResponse = http
        .post("https://user.auth.xboxlive.com/user/authenticate")
        .json(&json!({
            "Properties": {
                "AuthMethod": "RPS",
                "SiteName": "user.auth.xboxlive.com",
                "RpsTicket": format!("d={msa_token}")
            },
            "RelyingParty": "http://auth.xboxlive.com",
            "TokenType": "JWT"
        }))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let uhs = xbox
        .display_claims
        .xui
        .first()
        .map(|u| u.uhs.clone())
        .ok_or_else(|| Error::Auth("Xbox Live не вернул uhs".into()))?;

    let xsts: XboxResponse = http
        .post("https://xsts.auth.xboxlive.com/xsts/authorize")
        .json(&json!({
            "Properties": {
                "SandboxId": "RETAIL",
                "UserTokens": [xbox.token]
            },
            "RelyingParty": "rp://api.minecraftservices.com/",
            "TokenType": "JWT"
        }))
        .send()
        .await?
        .error_for_status()
        .map_err(|e| Error::Auth(format!("XSTS: {e}. Нет Game Pass / Minecraft?")))?
        .json()
        .await?;

    let xuid = xsts
        .display_claims
        .xui
        .first()
        .and_then(|u| u.xid.clone())
        .unwrap_or_else(|| "0".into());

    let mc: McLogin = http
        .post("https://api.minecraftservices.com/authentication/login_with_xbox")
        .json(&json!({
            "identityToken": format!("XBL3.0 x={};{}", uhs, xsts.token)
        }))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;

    let profile_resp = http
        .get("https://api.minecraftservices.com/minecraft/profile")
        .bearer_auth(&mc.access_token)
        .send()
        .await?;
    if profile_resp.status().as_u16() == 404 {
        return Err(Error::Auth(
            "у этого Microsoft-аккаунта нет профиля Java Edition".into(),
        ));
    }
    let profile: McProfile = profile_resp.error_for_status()?.json().await?;
    let uuid = format_uuid(&profile.id);
    Ok(Account {
        id: uuid.clone(),
        kind: AccountKind::Microsoft,
        name: profile.name,
        uuid,
        access_token: mc.access_token,
        refresh_token: refresh.to_string(),
        xuid,
        expires_at: chrono::Utc::now().timestamp() + mc.expires_in.unwrap_or(86400),
    })
}

fn format_uuid(raw: &str) -> String {
    let hex: String = raw.chars().filter(|c| c.is_ascii_hexdigit()).collect();
    if hex.len() != 32 {
        return raw.to_string();
    }
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}
