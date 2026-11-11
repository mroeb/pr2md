//! System tray app: syncs on a timer, shows status, opens the dashboard and settings.
//! Runs on winit's event loop (shared with the settings window's toolkit) on all platforms.

use crate::config::Config;
use crate::icon::{self, Badge};
use crate::state::State;
use crate::sync::{self, Options, Summary};
use crate::util::{self, autostart};
use std::path::Path;
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime};
use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};
use winit::application::ApplicationHandler;
use winit::event::{StartCause, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::window::WindowId;

enum Ev {
    Menu(MenuEvent),
    Tray(TrayIconEvent),
    SyncDone(Result<Summary, String>),
    SettingsClosed,
}

struct Items {
    status: MenuItem,
    open: MenuItem,
    sync: MenuItem,
    settings: MenuItem,
    pause: CheckMenuItem,
    autostart: CheckMenuItem,
    log: MenuItem,
    quit: MenuItem,
}

struct App {
    proxy: EventLoopProxy<Ev>,
    tray: Option<TrayIcon>,
    items: Items,
    next_sync: Instant,
    syncing: bool,
    paused: bool,
    settings_open: Arc<AtomicBool>,
    last: Option<Result<Summary, String>>,
    config_mtime: Option<SystemTime>,
}

pub fn run() -> util::Res<()> {
    let Some(_instance) = util::Lock::try_acquire("tray.lock") else {
        // Already running: be helpful and open the settings window instead.
        crate::info!("Tray already running - opening settings");
        spawn_settings();
        return Ok(());
    };
    crate::info!("Tray started (v{})", env!("CARGO_PKG_VERSION"));

    #[allow(unused_mut)]
    let mut builder = EventLoop::<Ev>::with_user_event();
    #[cfg(target_os = "macos")]
    {
        use winit::platform::macos::{ActivationPolicy, EventLoopBuilderExtMacOS};
        builder.with_activation_policy(ActivationPolicy::Accessory); // no Dock icon
    }
    let event_loop = builder.build()?;

    let proxy = event_loop.create_proxy();
    let p = proxy.clone();
    TrayIconEvent::set_event_handler(Some(move |e| {
        let _ = p.send_event(Ev::Tray(e));
    }));
    let p = proxy.clone();
    MenuEvent::set_event_handler(Some(move |e| {
        let _ = p.send_event(Ev::Menu(e));
    }));

    let items = Items {
        status: MenuItem::new("pr2md", false, None),
        open: MenuItem::new("Open dashboard", true, None),
        sync: MenuItem::new("Sync now", true, None),
        settings: MenuItem::new("Settings…", true, None),
        pause: CheckMenuItem::new("Pause syncing", true, false, None),
        autostart: CheckMenuItem::new("Start at login", true, autostart::is_enabled(), None),
        log: MenuItem::new("Open log file", true, None),
        quit: MenuItem::new("Quit pr2md", true, None),
    };
    let mut app = App {
        proxy,
        tray: None,
        items,
        next_sync: Instant::now(),
        syncing: false,
        paused: false,
        settings_open: Arc::new(AtomicBool::new(false)),
        last: None,
        config_mtime: mtime(&Config::path()),
    };
    event_loop.run_app(&mut app)?;
    Ok(())
}

fn mtime(p: &Path) -> Option<SystemTime> {
    std::fs::metadata(p).and_then(|m| m.modified()).ok()
}

fn tray_icon(badge: Badge, dimmed: bool) -> Option<Icon> {
    Icon::from_rgba(icon::rgba(32, badge, dimmed), 32, 32).ok()
}

fn spawn_settings() -> Option<std::process::Child> {
    let exe = std::env::current_exe().ok()?;
    Command::new(exe).arg("settings").spawn().ok()
}

impl App {
    fn create_tray(&mut self) {
        let menu = Menu::new();
        let i = &self.items;
        let _ = menu.append_items(&[
            &i.status,
            &PredefinedMenuItem::separator(),
            &i.open,
            &i.sync,
            &PredefinedMenuItem::separator(),
            &i.settings,
            &i.pause,
            &i.autostart,
            &i.log,
            &PredefinedMenuItem::separator(),
            &i.quit,
        ]);
        #[allow(unused_mut)]
        let mut builder = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_tooltip("pr2md")
            .with_title("");
        if let Some(icon) = tray_icon(Badge::None, false) {
            builder = builder.with_icon(icon);
        }
        #[cfg(windows)]
        {
            builder = builder.with_menu_on_left_click(false); // left click opens the dashboard
        }
        match builder.build() {
            Ok(t) => self.tray = Some(t),
            Err(e) => crate::warn!("Could not create tray icon: {e}"),
        }
        // Restore what we know from the last run.
        if let Some(st) = State::load() {
            if let Some(err) = st.last_error {
                self.last = Some(Err(err));
            } else if st.last_sync.is_some() {
                self.last = Some(Ok(Summary {
                    todos: st.todo_count,
                    ..Default::default()
                }));
            }
        }
        self.refresh_ui();

        // First start: nothing configured yet -> show the settings window.
        let cfg = Config::load().unwrap_or_default();
        if !Config::exists() || !cfg.problems().is_empty() || crate::auth::token().is_none() {
            self.open_settings();
        }
    }

    fn refresh_ui(&self) {
        let last_sync = State::load()
            .and_then(|s| s.last_sync)
            .map(|t| util::fmt_time(&t))
            .unwrap_or_default();
        let time = last_sync.split(' ').nth(1).unwrap_or("").to_string();
        let (status, badge) = if self.syncing {
            ("Syncing…".to_string(), self.badge())
        } else {
            match &self.last {
                None => ("Not synced yet".to_string(), Badge::None),
                Some(Ok(s)) => (
                    format!(
                        "Synced {time} · {} to-do{}",
                        s.todos,
                        if s.todos == 1 { "" } else { "s" }
                    ),
                    self.badge(),
                ),
                Some(Err(e)) => (
                    format!("⚠ {}", e.chars().take(70).collect::<String>()),
                    Badge::Error,
                ),
            }
        };
        let status = if self.paused {
            format!("Paused · {status}")
        } else {
            status
        };
        self.items.status.set_text(&status);
        self.items.sync.set_enabled(!self.syncing);
        if let Some(tray) = &self.tray {
            let tip: String = format!("pr2md · {status}").chars().take(120).collect();
            let _ = tray.set_tooltip(Some(tip));
            let _ = tray.set_icon(tray_icon(badge, self.paused));
        }
    }

    fn badge(&self) -> Badge {
        match &self.last {
            Some(Ok(s)) if s.todos > 0 => Badge::Todo,
            Some(Err(_)) => Badge::Error,
            _ => Badge::None,
        }
    }

    fn start_sync(&mut self) {
        if self.syncing {
            return;
        }
        let cfg = match Config::load() {
            Ok(c) => c,
            Err(e) => {
                self.last = Some(Err(e.to_string()));
                self.next_sync = Instant::now() + Duration::from_secs(60);
                self.refresh_ui();
                return;
            }
        };
        self.syncing = true;
        self.next_sync = Instant::now() + Duration::from_secs(cfg.interval_minutes * 60);
        self.refresh_ui();
        let proxy = self.proxy.clone();
        std::thread::spawn(move || {
            let r = sync::run(
                &cfg,
                &Options {
                    single: None,
                    notify: true,
                },
            )
            .map_err(|e| e.to_string());
            let _ = proxy.send_event(Ev::SyncDone(r));
        });
    }

    fn open_settings(&self) {
        if self.settings_open.swap(true, Ordering::SeqCst) {
            return; // already open
        }
        let flag = self.settings_open.clone();
        let proxy = self.proxy.clone();
        std::thread::spawn(move || {
            if let Some(mut child) = spawn_settings() {
                let _ = child.wait();
            }
            flag.store(false, Ordering::SeqCst);
            let _ = proxy.send_event(Ev::SettingsClosed);
        });
    }

    fn open_dashboard(&self) {
        let cfg = Config::load().unwrap_or_default();
        if cfg.vault.is_empty() {
            self.open_settings();
            return;
        }
        let path = Path::new(&cfg.vault).join(crate::render::dashboard_file(&cfg));
        util::open(&util::obsidian_uri(&path));
    }
}

impl ApplicationHandler<Ev> for App {
    fn new_events(&mut self, _el: &ActiveEventLoop, cause: StartCause) {
        match cause {
            StartCause::Init => self.create_tray(),
            StartCause::ResumeTimeReached { .. } => {
                // Settings edited by hand? Sync right away with the new config.
                let m = mtime(&Config::path());
                if m != self.config_mtime {
                    self.config_mtime = m;
                    self.next_sync = Instant::now();
                }
                if !self.paused && Instant::now() >= self.next_sync {
                    self.start_sync();
                }
            }
            _ => {}
        }
    }

    fn user_event(&mut self, el: &ActiveEventLoop, ev: Ev) {
        match ev {
            Ev::Menu(e) => {
                let i = &self.items;
                if e.id == *i.open.id() {
                    self.open_dashboard();
                } else if e.id == *i.sync.id() {
                    self.start_sync();
                } else if e.id == *i.settings.id() {
                    self.open_settings();
                } else if e.id == *i.pause.id() {
                    self.paused = i.pause.is_checked();
                    if !self.paused {
                        self.next_sync = Instant::now();
                    }
                    self.refresh_ui();
                } else if e.id == *i.autostart.id() {
                    if let Err(e) = autostart::set(i.autostart.is_checked()) {
                        crate::warn!("Autostart: {e}");
                    }
                    i.autostart.set_checked(autostart::is_enabled());
                } else if e.id == *i.log.id() {
                    util::open(&util::log_path().to_string_lossy());
                } else if e.id == *i.quit.id() {
                    self.tray = None;
                    el.exit();
                }
            }
            Ev::Tray(TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            }) => {
                #[cfg(windows)]
                self.open_dashboard();
            }
            Ev::Tray(_) => {}
            Ev::SyncDone(r) => {
                self.syncing = false;
                if r.is_err() {
                    // Retry sooner after errors (network hiccups), but not in a tight loop.
                    let retry = Instant::now() + Duration::from_secs(120);
                    self.next_sync = self.next_sync.min(retry);
                }
                self.last = Some(r);
                self.config_mtime = mtime(&Config::path());
                self.refresh_ui();
            }
            Ev::SettingsClosed => {
                self.items.autostart.set_checked(autostart::is_enabled());
                self.config_mtime = mtime(&Config::path());
                if !self.paused {
                    self.start_sync();
                }
            }
        }
    }

    fn about_to_wait(&mut self, el: &ActiveEventLoop) {
        // Wake at the next sync, and every few seconds to notice config edits.
        let wake = if self.paused {
            Instant::now() + Duration::from_secs(5)
        } else {
            self.next_sync.min(Instant::now() + Duration::from_secs(5))
        };
        el.set_control_flow(ControlFlow::WaitUntil(wake));
    }

    fn resumed(&mut self, _el: &ActiveEventLoop) {}

    fn window_event(&mut self, _el: &ActiveEventLoop, _id: WindowId, _ev: WindowEvent) {}
}
