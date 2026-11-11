//! Desktop notifications without extra dependencies:
//! Windows toast via PowerShell (click opens the note in Obsidian), `osascript` on macOS,
//! `notify-send` on Linux.

use std::path::Path;

pub fn show(title: &str, body: &str, open: Option<&Path>) {
    crate::info!("NOTIFY: {title} - {}", body.replace('\n', "; "));
    let (title, body) = (title.to_string(), body.to_string());
    let uri = open.map(crate::util::obsidian_uri);
    // Fire and forget: never block a sync on a notification.
    std::thread::spawn(move || {
        if let Err(e) = platform(&title, &body, uri.as_deref()) {
            crate::warn!("Notification failed: {e}");
        }
    });
}

#[cfg(windows)]
fn platform(title: &str, body: &str, uri: Option<&str>) -> std::io::Result<()> {
    const PS: &str = r#"
[Windows.UI.Notifications.ToastNotificationManager, Windows.UI.Notifications, ContentType = WindowsRuntime] | Out-Null
[Windows.Data.Xml.Dom.XmlDocument, Windows.Data.Xml.Dom.XmlDocument, ContentType = WindowsRuntime] | Out-Null
$xml = New-Object Windows.Data.Xml.Dom.XmlDocument
$xml.LoadXml($env:PR2MD_TOAST)
$toast = New-Object Windows.UI.Notifications.ToastNotification $xml
[Windows.UI.Notifications.ToastNotificationManager]::CreateToastNotifier('{1AC14E77-02E7-4E5D-B744-2EB1AE5198B7}\WindowsPowerShell\v1.0\powershell.exe').Show($toast)
"#;
    let esc = |s: &str| {
        s.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
            .replace('"', "&quot;")
    };
    let launch = uri
        .map(|u| format!(" activationType=\"protocol\" launch=\"{}\"", esc(u)))
        .unwrap_or_default();
    let xml = format!(
        "<toast{launch}><visual><binding template=\"ToastGeneric\"><text>{}</text><text>{}</text></binding></visual></toast>",
        esc(title),
        esc(body)
    );
    crate::util::quiet_command("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", PS])
        .env("PR2MD_TOAST", xml)
        .output()?;
    Ok(())
}

#[cfg(target_os = "macos")]
fn platform(title: &str, body: &str, _uri: Option<&str>) -> std::io::Result<()> {
    let q = |s: &str| format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""));
    let script = format!("display notification {} with title {}", q(body), q(title));
    std::process::Command::new("osascript")
        .args(["-e", &script])
        .output()?;
    Ok(())
}

#[cfg(all(unix, not(target_os = "macos")))]
fn platform(title: &str, body: &str, _uri: Option<&str>) -> std::io::Result<()> {
    std::process::Command::new("notify-send")
        .args(["--app-name=pr2md", title, body])
        .output()?;
    Ok(())
}
