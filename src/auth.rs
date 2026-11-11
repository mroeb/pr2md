//! Signing in to GitHub.
//!
//! Token sources, in order:
//! 1. `PR2MD_TOKEN` environment variable
//! 2. the token stored by "Sign in with GitHub" (OAuth device flow) or `pr2md login --token`
//! 3. the GitHub CLI (`gh auth token`)
//! 4. `GITHUB_TOKEN` / `GH_TOKEN` environment variables
//!
//! The stored token is encrypted with DPAPI on Windows and saved with 0600 permissions elsewhere.

use crate::config::Config;
use crate::util::{self, Res};
use serde_json::Value;
use std::path::PathBuf;
use std::time::{Duration, Instant};

pub const SCOPES: &str = "repo read:org";

#[derive(Debug, Clone, PartialEq)]
pub enum Source {
    Env,
    Stored,
    GhCli,
}

impl Source {
    pub fn describe(&self) -> &'static str {
        match self {
            Source::Env => "environment variable",
            Source::Stored => "pr2md sign-in",
            Source::GhCli => "GitHub CLI",
        }
    }
}

pub fn token() -> Option<(String, Source)> {
    let env = |k: &str| std::env::var(k).ok().filter(|s| !s.trim().is_empty());
    if let Some(t) = env("PR2MD_TOKEN") {
        return Some((t, Source::Env));
    }
    if let Some(t) = load_stored() {
        return Some((t, Source::Stored));
    }
    if let Some(t) = gh_cli_token() {
        return Some((t, Source::GhCli));
    }
    env("GITHUB_TOKEN")
        .or_else(|| env("GH_TOKEN"))
        .map(|t| (t, Source::Env))
}

/// Ask the GitHub CLI for its stored login (ignoring a possibly stale GITHUB_TOKEN).
pub fn gh_cli_token() -> Option<String> {
    let out = util::quiet_command("gh")
        .args(["auth", "token"])
        .env_remove("GITHUB_TOKEN")
        .env_remove("GH_TOKEN")
        .output()
        .ok()?;
    let t = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (out.status.success() && !t.is_empty()).then_some(t)
}

// --------------------------------------------------------------------------- storage

fn token_path() -> PathBuf {
    util::app_dir().join(if cfg!(windows) { "token.bin" } else { "token" })
}

pub fn store(token: &str) -> Res<()> {
    let data = protect(token.trim().as_bytes())?;
    let path = token_path();
    util::write_atomic(&path, &data)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

pub fn forget() {
    let _ = std::fs::remove_file(token_path());
}

fn load_stored() -> Option<String> {
    let data = std::fs::read(token_path()).ok()?;
    let t = String::from_utf8(unprotect(&data)?).ok()?;
    let t = t.trim().to_string();
    (!t.is_empty()).then_some(t)
}

#[cfg(windows)]
fn protect(data: &[u8]) -> Res<Vec<u8>> {
    dpapi(data, true).ok_or_else(|| "Could not encrypt the token (DPAPI)".into())
}
#[cfg(windows)]
fn unprotect(data: &[u8]) -> Option<Vec<u8>> {
    dpapi(data, false)
}

/// Windows Data Protection API: encrypts with the current user's login credentials.
#[cfg(windows)]
fn dpapi(data: &[u8], encrypt: bool) -> Option<Vec<u8>> {
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Cryptography::{
        CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData, CryptUnprotectData,
    };
    let input = CRYPT_INTEGER_BLOB {
        cbData: data.len() as u32,
        pbData: data.as_ptr() as *mut u8,
    };
    let mut output = CRYPT_INTEGER_BLOB {
        cbData: 0,
        pbData: std::ptr::null_mut(),
    };
    let ok = unsafe {
        if encrypt {
            CryptProtectData(
                &input,
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        } else {
            CryptUnprotectData(
                &input,
                std::ptr::null_mut(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        }
    };
    if ok == 0 || output.pbData.is_null() {
        return None;
    }
    let result =
        unsafe { std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec() };
    unsafe { LocalFree(output.pbData as _) };
    Some(result)
}

#[cfg(not(windows))]
fn protect(data: &[u8]) -> Res<Vec<u8>> {
    Ok(data.to_vec())
}
#[cfg(not(windows))]
fn unprotect(data: &[u8]) -> Option<Vec<u8>> {
    Some(data.to_vec())
}

// --------------------------------------------------------------------------- device flow

#[derive(Debug, Clone)]
pub struct DeviceCode {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub interval: u64,
    pub expires_in: u64,
}

fn form_post(url: &str, fields: &[(&str, &str)]) -> Res<Value> {
    let body: Vec<String> = fields
        .iter()
        .map(|(k, v)| format!("{k}={}", util::percent_encode(v)))
        .collect();
    let mut resp = crate::github::agent()
        .post(url)
        .header("Accept", "application/json")
        .header("Content-Type", "application/x-www-form-urlencoded")
        .send(body.join("&").as_bytes())?;
    let status = resp.status().as_u16();
    let text = resp.body_mut().read_to_string()?;
    let v: Value = serde_json::from_str(&text)
        .map_err(|_| format!("GitHub returned HTTP {status}: {text}"))?;
    Ok(v)
}

/// Step 1: get a code the user types in at github.com/login/device.
pub fn device_start(cfg: &Config) -> Res<DeviceCode> {
    let Some(client_id) = cfg.client_id() else {
        crate::bail!(
            "No OAuth client id configured. Set `client_id` in config.toml (see README), \
             or use the GitHub CLI login / a personal access token instead."
        );
    };
    let v = form_post(
        &format!("{}/login/device/code", cfg.web_url()),
        &[("client_id", &client_id), ("scope", SCOPES)],
    )?;
    if let Some(err) = v["error"].as_str() {
        crate::bail!("{err}: {}", v["error_description"].as_str().unwrap_or(""));
    }
    Ok(DeviceCode {
        device_code: v["device_code"].as_str().unwrap_or_default().into(),
        user_code: v["user_code"].as_str().unwrap_or_default().into(),
        verification_uri: v["verification_uri"]
            .as_str()
            .unwrap_or("https://github.com/login/device")
            .into(),
        interval: v["interval"].as_u64().unwrap_or(5),
        expires_in: v["expires_in"].as_u64().unwrap_or(900),
    })
}

/// Step 2: poll until the user approved (or denied / the code expired). Stores the token.
pub fn device_finish(cfg: &Config, code: &DeviceCode, cancelled: &dyn Fn() -> bool) -> Res<String> {
    let client_id = cfg.client_id().unwrap_or_default();
    let url = format!("{}/login/oauth/access_token", cfg.web_url());
    let deadline = Instant::now() + Duration::from_secs(code.expires_in);
    let mut interval = code.interval.max(1);
    while Instant::now() < deadline {
        for _ in 0..interval * 10 {
            if cancelled() {
                crate::bail!("Sign-in cancelled.");
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        let v = form_post(
            &url,
            &[
                ("client_id", &client_id),
                ("device_code", &code.device_code),
                ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
            ],
        )?;
        if let Some(token) = v["access_token"].as_str() {
            store(token)?;
            return Ok(token.to_string());
        }
        match v["error"].as_str() {
            Some("authorization_pending") => {}
            Some("slow_down") => interval = v["interval"].as_u64().unwrap_or(interval + 5),
            Some("expired_token") => crate::bail!("The code expired. Please try again."),
            Some("access_denied") => crate::bail!("Sign-in was denied in the browser."),
            Some(e) => crate::bail!("{e}: {}", v["error_description"].as_str().unwrap_or("")),
            None => crate::bail!("Unexpected answer from GitHub: {v}"),
        }
    }
    crate::bail!("The code expired. Please try again.")
}

/// Check a token and return the GitHub login it belongs to.
pub fn whoami(cfg: &Config, token: &str) -> Res<String> {
    crate::github::Client::new(cfg.graphql_url(), token.to_string()).viewer()
}
