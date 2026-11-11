//! Small helpers shared by every module: errors, paths, logging, time, OS glue.

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;

pub type Res<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[macro_export]
macro_rules! bail {
    ($($t:tt)*) => { return Err(format!($($t)*).into()) };
}

// --------------------------------------------------------------------------- paths

/// Where config, state, token and log live.
///
/// "Portable mode": if a `config.toml` sits next to the executable, that folder is used.
/// Otherwise the usual per-user config folder of the OS.
pub fn app_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("PR2MD_HOME") {
        return PathBuf::from(dir);
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            if dir.join("config.toml").is_file() {
                return dir.to_path_buf();
            }
        }
    }
    let home = || PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
    let base = if cfg!(windows) {
        std::env::var_os("APPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(home)
    } else if cfg!(target_os = "macos") {
        home().join("Library/Application Support")
    } else {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home().join(".config"))
    };
    base.join("pr2md")
}

pub fn config_path() -> PathBuf {
    app_dir().join("config.toml")
}
pub fn state_path() -> PathBuf {
    app_dir().join("state.json")
}
pub fn log_path() -> PathBuf {
    app_dir().join("pr2md.log")
}

/// Write a file atomically (tmp + rename) so a crash never leaves half a file behind.
pub fn write_atomic(path: &Path, content: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("pr2md-tmp");
    fs::write(&tmp, content)?;
    fs::rename(&tmp, path)
}

/// Write only when the content differs, so Obsidian / sync tools don't see pointless changes.
pub fn write_if_changed(path: &Path, content: &str) -> std::io::Result<bool> {
    if fs::read_to_string(path).is_ok_and(|old| old == content) {
        return Ok(false);
    }
    write_atomic(path, content.as_bytes())?;
    Ok(true)
}

/// Exclusive lock so the tray, the settings window and the CLI never sync at the same time.
pub struct Lock(#[allow(dead_code)] File);

impl Lock {
    pub fn acquire(name: &str) -> Res<Lock> {
        let dir = app_dir();
        fs::create_dir_all(&dir)?;
        let f = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(dir.join(name))?;
        f.lock()?;
        Ok(Lock(f))
    }
    pub fn try_acquire(name: &str) -> Option<Lock> {
        let dir = app_dir();
        fs::create_dir_all(&dir).ok()?;
        let f = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(dir.join(name))
            .ok()?;
        f.try_lock().ok()?;
        Some(Lock(f))
    }
}

// --------------------------------------------------------------------------- logging

static LOG: Mutex<Option<File>> = Mutex::new(None);
static ECHO: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn init_log(echo: bool) {
    ECHO.store(echo, std::sync::atomic::Ordering::Relaxed);
    let path = log_path();
    let _ = fs::create_dir_all(app_dir());
    if fs::metadata(&path).is_ok_and(|m| m.len() > 1_000_000) {
        let _ = fs::rename(&path, path.with_extension("log.old"));
    }
    *LOG.lock().unwrap() = OpenOptions::new().create(true).append(true).open(path).ok();
}

pub fn log(level: &str, msg: &str) {
    let line = format!(
        "{} {level:5} {msg}",
        chrono::Local::now().format("%Y-%m-%d %H:%M:%S")
    );
    if ECHO.load(std::sync::atomic::Ordering::Relaxed) {
        eprintln!("{msg}");
    }
    if let Some(f) = LOG.lock().unwrap().as_mut() {
        let _ = writeln!(f, "{line}");
    }
}

#[macro_export]
macro_rules! info { ($($t:tt)*) => { $crate::util::log("INFO", &format!($($t)*)) }; }
#[macro_export]
macro_rules! warn { ($($t:tt)*) => { $crate::util::log("WARN", &format!($($t)*)) }; }

// --------------------------------------------------------------------------- time

pub fn now_ts() -> i64 {
    chrono::Utc::now().timestamp()
}

pub fn parse_ts(iso: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    chrono::DateTime::parse_from_rfc3339(iso)
        .ok()
        .map(|d| d.with_timezone(&chrono::Utc))
}

/// `2026-11-11T09:42:00Z` -> `2026-11-11 10:42` (local time).
pub fn fmt_time(iso: &str) -> String {
    parse_ts(iso)
        .map(|d| {
            d.with_timezone(&chrono::Local)
                .format("%Y-%m-%d %H:%M")
                .to_string()
        })
        .unwrap_or_default()
}

pub fn fmt_date(iso: &str) -> String {
    parse_ts(iso)
        .map(|d| {
            d.with_timezone(&chrono::Local)
                .format("%Y-%m-%d")
                .to_string()
        })
        .unwrap_or_default()
}

// --------------------------------------------------------------------------- text

pub fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

// --------------------------------------------------------------------------- OS glue

#[cfg(windows)]
pub const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// A `Command` that never flashes a console window on Windows.
pub fn quiet_command(program: &str) -> Command {
    #[allow(unused_mut)]
    let mut cmd = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

/// Open a URL (https://, obsidian://) or a file with the default application.
pub fn open(target: &str) {
    let result = if cfg!(windows) {
        quiet_command("rundll32")
            .args(["url.dll,FileProtocolHandler", target])
            .spawn()
    } else if cfg!(target_os = "macos") {
        Command::new("open").arg(target).spawn()
    } else {
        Command::new("xdg-open").arg(target).spawn()
    };
    if let Err(e) = result {
        crate::warn!("Could not open {target}: {e}");
    }
}

/// `obsidian://open?path=...` for an absolute file path.
pub fn obsidian_uri(path: &Path) -> String {
    format!(
        "obsidian://open?path={}",
        percent_encode(&path.to_string_lossy())
    )
}

/// Native "choose folder" dialog without pulling in a GUI toolkit.
#[allow(dead_code)]
pub fn pick_folder() -> Option<String> {
    let out = if cfg!(windows) {
        quiet_command("powershell")
            .args(["-NoProfile", "-STA", "-Command",
                "Add-Type -AssemblyName System.Windows.Forms; $d = New-Object System.Windows.Forms.FolderBrowserDialog; \
                 $d.Description = 'Choose your Obsidian vault'; \
                 if ($d.ShowDialog() -eq 'OK') { [Console]::OutputEncoding = [Text.Encoding]::UTF8; $d.SelectedPath }"])
            .output()
    } else if cfg!(target_os = "macos") {
        Command::new("osascript").args(["-e", "POSIX path of (choose folder with prompt \"Choose your Obsidian vault\")"]).output()
    } else {
        Command::new("zenity").args(["--file-selection", "--directory", "--title=Choose your Obsidian vault"]).output()
    }
    .ok()?;
    let path = String::from_utf8_lossy(&out.stdout)
        .trim()
        .trim_end_matches('/')
        .to_string();
    (!path.is_empty()).then_some(path)
}

// --------------------------------------------------------------------------- autostart

#[allow(dead_code)]
pub mod autostart {
    use super::*;

    fn exe() -> String {
        std::env::current_exe()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default()
    }

    #[cfg(windows)]
    const RUN_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";

    #[cfg(target_os = "macos")]
    fn plist() -> PathBuf {
        PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
            .join("Library/LaunchAgents/io.github.pr2md.plist")
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    fn desktop_file() -> PathBuf {
        let base = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".config")
            });
        base.join("autostart/pr2md.desktop")
    }

    pub fn is_enabled() -> bool {
        #[cfg(windows)]
        return quiet_command("reg")
            .args(["query", RUN_KEY, "/v", "pr2md"])
            .output()
            .is_ok_and(|o| o.status.success());
        #[cfg(target_os = "macos")]
        return plist().exists();
        #[cfg(all(unix, not(target_os = "macos")))]
        return desktop_file().exists();
    }

    pub fn set(enable: bool) -> Res<()> {
        #[cfg(windows)]
        {
            let ok = if enable {
                let value = format!("\"{}\" tray", exe());
                quiet_command("reg")
                    .args([
                        "add", RUN_KEY, "/v", "pr2md", "/t", "REG_SZ", "/d", &value, "/f",
                    ])
                    .output()?
            } else {
                quiet_command("reg")
                    .args(["delete", RUN_KEY, "/v", "pr2md", "/f"])
                    .output()?
            };
            if enable && !ok.status.success() {
                bail!("reg.exe failed: {}", String::from_utf8_lossy(&ok.stderr));
            }
        }
        #[cfg(target_os = "macos")]
        {
            if enable {
                let xml = format!(
                    "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
                     <plist version=\"1.0\"><dict><key>Label</key><string>io.github.pr2md</string>\
                     <key>ProgramArguments</key><array><string>{}</string><string>tray</string></array>\
                     <key>RunAtLoad</key><true/></dict></plist>\n",
                    exe()
                );
                write_atomic(&plist(), xml.as_bytes())?;
            } else {
                let _ = fs::remove_file(plist());
            }
        }
        #[cfg(all(unix, not(target_os = "macos")))]
        {
            if enable {
                let entry = format!(
                    "[Desktop Entry]\nType=Application\nName=pr2md\nComment=GitHub pull requests to Markdown\nExec=\"{}\" tray\nX-GNOME-Autostart-enabled=true\n",
                    exe()
                );
                write_atomic(&desktop_file(), entry.as_bytes())?;
            } else {
                let _ = fs::remove_file(desktop_file());
            }
        }
        Ok(())
    }
}
