//! The menu bar panel, ported from `menubar.go`.
//!
//! Left-clicking the menu bar status item (or pressing `⌃⌥⌘D`) toggles a
//! native floating `NSPanel` popover ([`WindowKind::Popup`]) anchored beneath
//! the status item with macOS 26 Liquid Glass (`NSGlassEffectView`),
//! content-driven height (`SizeToContent::Height`), fine-grained reactive rows,
//! collapsible Homebrew and Login Agents service sections, `nsh_base::switch()`
//! controls with subtle haptic feedback, and an Open / Refresh / Quit footer
//! with native tooltips. Right-clicking (or Control-clicking) opens a native
//! `NSMenu` with monochrome template SVG icons on every item and explicit
//! section headers.
//!
//! The rows are a plain model ([`Panel`]) built by [`panel`]. Both the tray
//! title/menu ([`title_runs`], [`menu_entries`], [`menu`]) and headless tests
//! read it, while [`popover_content`] binds directly to store signals and
//! keyed `each(...)` lists so toggling switches or refreshing never rebuilds
//! the scroll container or resets scroll position. Nothing in a handler
//! blocks: every switch calls the store, which runs its provider call on a
//! blocking thread and writes its signals.

use std::{
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

use devcore::service::{Action as ServiceAction, Source, State as ServiceState};
use nsh_base::{button, collapsible, switch, theme};
use nsh_core::{
    AccessibilityState, GlassEffect, Haptic, LayoutAnimation, LayoutCurve, LayoutTransition,
    PointerEvents,
    color::rgb,
    desktop::{
        Display, SizeToContent, TrayTitleRun, WindowAttributes, WindowCloseCause, WindowKind,
        WindowPosition,
    },
    element::{
        ImageSrc, ReadSignal, Svg, animate_layout,
        desktop::{
            MenuItem, TrayClickEvent, displays, main_window, register_global_shortcut, set_tray,
            tray, window,
        },
        haptic, reduce_motion, svg,
    },
    geometry::{Bounds, Point, deg, point, size},
    platform::Os,
    prelude::*,
};

use crate::{
    components::controls,
    store::{self, Section, databases, proxy, services, software::SoftwareKind, system},
    theme as tokens,
};

/// The popover's width and height bounds in points (matching `menubar.go`).
const POPOVER_WIDTH: f32 = 340.0;
const POPOVER_MIN_HEIGHT: f32 = 240.0;
const POPOVER_MAX_HEIGHT: f32 = 560.0;
const POPOVER_MARGIN: f32 = 8.0;

/// Suppresses the `LeftMouseUp` reopen bounce when clicking the status item
/// while the popover was already open caused `close_on_blur` to hide it on
/// `LeftMouseDown`.
const BLUR_REOPEN_DEBOUNCE: Duration = Duration::from_millis(200);

/// Above this service count in a single source group, the right-click `NSMenu`
/// nests that group inside a clean submenu instead of expanding every row inline.
const SERVICE_MENU_SUBMENU_THRESHOLD: usize = 6;

/// System-wide shortcut that toggles the menu bar popover on macOS.
pub const GLOBAL_SHORTCUT: &str = "ctrl+alt+cmd+d";

/// macOS symbol representation of [`GLOBAL_SHORTCUT`].
pub const GLOBAL_SHORTCUT_LABEL: &str = "⌃⌥⌘D";

/// macOS red tint for the menu bar failure count run (`#ff3b30`).
const TRAY_FAILED_COLOR_HEX: u32 = 0xff3b30;

/// When enabled by tests (`use_fake_menubar`), skips OS-level global shortcut
/// registration so headless tests never touch Carbon hotkeys.
static FAKE_MODE: AtomicBool = AtomicBool::new(false);

/// Opts out of host OS global-shortcut registration during headless tests.
pub fn use_fake_menubar() {
    FAKE_MODE.store(true, Ordering::Relaxed);
}

pub(crate) fn is_fake_mode() -> bool {
    FAKE_MODE.load(Ordering::Relaxed)
}

/// Number of currently failed managed resources (databases, proxy, tunnel).
pub(crate) fn failed_count() -> u32 {
    let failed_instances = databases::instances()
        .iter()
        .filter(|inst| databases::status(&inst.id) == "Failed")
        .count();
    let failed_proxy = usize::from(proxy::proxy_status() == "Failed");
    let failed_tunnel = usize::from(proxy::tunnel_status() == "Failed");
    (failed_instances + failed_proxy + failed_tunnel).min(u32::MAX as usize) as u32
}

/// The icon `assets/icons/menubar.svg` draws, inline so a cargo run and a
/// bundled app show the same template.
const MENUBAR_ICON: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="36" height="36" viewBox="0 0 36 36">
  <path fill-rule="evenodd" d="M9.5 3h17a6.5 6.5 0 0 1 0 13h-17a6.5 6.5 0 0 1 0-13Zm17 2.9a3.6 3.6 0 1 0 0 7.2 3.6 3.6 0 0 0 0-7.2Z"/>
  <rect x="4.2" y="21.2" width="27.6" height="10.6" rx="5.3" fill="none" stroke="#000" stroke-width="2.4"/>
  <circle cx="9.5" cy="26.5" r="2.7"/>
</svg>"##;

const ICON_OPEN: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round">"#;
const ICON_CLOSE: &str = "</svg>";

const PACKAGE_BODY: &str = r#"<path d="M11 21.73a2 2 0 0 0 2 0l7-4A2 2 0 0 0 21 16V8a2 2 0 0 0-1-1.73l-7-4a2 2 0 0 0-2 0l-7 4A2 2 0 0 0 3 8v8a2 2 0 0 0 1 1.73z"/><path d="M12 22V12"/><polyline points="3.29 7 12 12 20.71 7"/><path d="m7.5 4.27 9 5.15"/>"#;
const DATABASE_BODY: &str = r#"<ellipse cx="12" cy="5" rx="9" ry="3"/><path d="M3 5V19A9 3 0 0 0 21 19V5"/><path d="M3 12A9 3 0 0 0 21 12"/>"#;
const SHIELD_BODY: &str = r#"<path d="M20 13c0 5-3.5 7.5-7.66 8.95a1 1 0 0 1-.67-.01C7.5 20.5 4 18 4 13V6a1 1 0 0 1 1-1c2 0 4.5-1.2 6.24-2.72a1.17 1.17 0 0 1 1.52 0C14.51 3.81 17 5 19 5a1 1 0 0 1 1 1z"/><path d="m9 12 2 2 4-4"/>"#;
const CLOUD_BODY: &str = r#"<path d="M17.5 19H9a7 7 0 1 1 6.71-9h1.79a4.5 4.5 0 1 1 0 9Z"/>"#;
const WINDOW_BODY: &str = r#"<rect x="2" y="4" width="20" height="16" rx="2"/><path d="M10 4v4"/><path d="M2 8h20"/><path d="M6 4v4"/>"#;
const REFRESH_BODY: &str =
    r#"<path d="M21 12a9 9 0 1 1-9-9c2.52 0 4.93 1 6.74 2.74L21 8"/><path d="M21 3v5h-5"/>"#;
const QUIT_BODY: &str = r#"<path d="M12 2v10"/><path d="M18.4 6.6a9 9 0 1 1-12.77.04"/>"#;
const PLUS_BODY: &str = r#"<path d="M5 12h14"/><path d="M12 5v14"/>"#;
const LAYERS_BODY: &str = r#"<path d="M12.83 2.18a2 2 0 0 0-1.66 0L2.6 6.08a1 1 0 0 0 0 1.83l8.58 3.91a2 2 0 0 0 1.66 0l8.58-3.9a1 1 0 0 0 0-1.83z"/><path d="M2 12a1 1 0 0 0 .58.91l8.6 3.91a2 2 0 0 0 1.65 0l8.58-3.9A1 1 0 0 0 22 12"/><path d="M2 17a1 1 0 0 0 .58.91l8.6 3.91a2 2 0 0 0 1.65 0l8.58-3.9A1 1 0 0 0 22 17"/>"#;
const PLAY_BODY: &str = r#"<polygon points="6 3 20 12 6 21 6 3"/>"#;
const STOP_BODY: &str = r#"<rect x="5" y="5" width="14" height="14" rx="2"/>"#;
const CHEVRON_RIGHT_BODY: &str = r#"<path d="m9 18 6-6-6-6"/>"#;
const TERMINAL_BODY: &str =
    r#"<polyline points="4 17 10 11 4 5"/><line x1="12" x2="20" y1="19" y2="19"/>"#;

fn icon_src(body: &str) -> ImageSrc {
    ImageSrc::svg_markup(&format!("{ICON_OPEN}{body}{ICON_CLOSE}"))
}

fn icon_svg(body: &str) -> Svg {
    svg(icon_src(body))
}

/// One kind of row, so a test can tell a heading from a switch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowKind {
    /// The app's name and how much runs.
    Header,
    /// A read-only readout: the workspace root, the proxy port, the helper.
    Status,
    /// A database instance, and the switch that starts or stops it.
    Instance,
    /// The HTTPS proxy's switch.
    Proxy,
    /// The Cloudflare tunnel's switch, shown only once it is configured.
    Tunnel,
    /// An optional Homebrew or UserAgent service row when loaded.
    Service,
    /// Open Devbase, Refresh, Quit.
    Action,
    /// A line between groups.
    Separator,
}

impl RowKind {
    /// A short name for a test harness's text.
    pub fn tag(self) -> &'static str {
        match self {
            RowKind::Header => "header",
            RowKind::Status => "status",
            RowKind::Instance => "instance",
            RowKind::Proxy => "proxy",
            RowKind::Tunnel => "tunnel",
            RowKind::Service => "service",
            RowKind::Action => "action",
            RowKind::Separator => "separator",
        }
    }
}

/// What choosing a row does. The tray's handlers call [`dispatch`] with it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Show the Databases section at an instance.
    OpenInstance(String),
    /// Show the new-instance form in the Databases section.
    OpenNewInstance,
    /// Start or stop a database instance.
    ToggleInstance { id: String, start: bool },
    /// Apply the saved proxy rules, or stop the proxy job.
    ToggleProxy(bool),
    /// Start the Cloudflare tunnel, or stop it.
    ToggleTunnel(bool),
    /// Show the HTTPS Proxy section.
    OpenHttpsProxy,
    /// Show the Services section at a Homebrew or UserAgent item.
    OpenService { source: Source, id: String },
    /// Start or stop a Homebrew or UserAgent service item.
    ToggleService { id: String, start: bool },
    /// Bring the main window forward.
    OpenWindow,
    /// Read the workspace, proxy and settings again.
    Refresh,
    /// Read the managed-database scope and ask to quit, as Go's requestQuit.
    Quit,
}

/// One row of the panel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    /// Stable identity, also the menu item's key in tests.
    pub key: String,
    pub kind: RowKind,
    /// The row's title, as Go's `title`.
    pub label: String,
    /// The line under it, as Go's `sub`.
    pub detail: String,
    /// The state the row's status dot shows.
    pub state: String,
    /// A switch's state; `None` for rows that are not switches.
    pub checked: Option<bool>,
    /// Whether it can be chosen.
    pub enabled: bool,
    pub action: Option<Action>,
}

/// The panel as the tray renders it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Panel {
    /// The number beside the icon (`menubar.go`'s `SetTitle`): running
    /// resources, or empty at rest.
    pub title: String,
    /// The icon's tooltip.
    pub tooltip: String,
    pub running: usize,
    pub failed: usize,
    /// What the status dot in the header shows.
    pub state: String,
    /// The header's summary line (`3 running · 1 failed`).
    pub summary: String,
    pub rows: Vec<Row>,
}

/// One database instance in [`PanelInput`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstanceRow {
    pub id: String,
    /// The engine's name (`PostgreSQL`).
    pub title: String,
    pub port: i64,
    pub status: String,
}

/// One optional Homebrew or UserAgent service in [`PanelInput`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceRow {
    pub id: String,
    pub source: Source,
    pub name: String,
    pub status: String,
    pub running: bool,
    pub busy: bool,
    pub can_toggle: bool,
}

/// Everything the panel reads from the stores, in one value, so the labels
/// can be built without a platform or a render.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PanelInput {
    pub instances: Vec<InstanceRow>,
    pub services: Vec<ServiceRow>,
    pub proxy_status: String,
    pub proxy_rules: usize,
    pub proxy_port: i64,
    pub busy: bool,
    pub tunnel_status: String,
    pub tunnel_detail: String,
    pub tunnel_rules: usize,
    pub tunnel_workers: usize,
    pub tunnel_configured: bool,
    pub helper: String,
    pub workspace_root: String,
    pub quit_open: bool,
}

fn is_internal_devbase_service(name: &str, id: &str) -> bool {
    name.starts_with("xyz.jyeu.devbase.dev.") || id.contains("xyz.jyeu.devbase.dev.")
}

fn running_status(status: &str) -> bool {
    matches!(status, "Running" | "Healthy")
}

fn busy_status(status: &str) -> bool {
    matches!(status, "Unknown" | "Starting" | "Stopping" | "Restarting")
}

impl Panel {
    /// Builds the rows from one read of the stores.
    pub fn from(input: PanelInput) -> Panel {
        let instances_running = input
            .instances
            .iter()
            .filter(|row| running_status(&row.status))
            .count();
        let proxy_on = input.proxy_status == "Running";
        let tunnel_on = input.tunnel_status == "Running";
        let running = instances_running + usize::from(proxy_on) + usize::from(tunnel_on);
        let failed = input
            .instances
            .iter()
            .filter(|row| row.status == "Failed")
            .count()
            + usize::from(input.proxy_status == "Failed")
            + usize::from(input.tunnel_status == "Failed");
        let state = if failed > 0 {
            "Failed"
        } else if running > 0 {
            "Running"
        } else {
            "Stopped"
        }
        .to_string();
        let summary = if failed > 0 {
            format!("{running} running · {failed} failed")
        } else {
            format!("{running} running")
        };

        let mut rows = Vec::new();
        rows.push(Row {
            key: "status".to_string(),
            kind: RowKind::Header,
            label: "Devbase".to_string(),
            detail: summary.clone(),
            state: state.clone(),
            checked: None,
            enabled: false,
            action: None,
        });
        rows.push(Row {
            key: "root".to_string(),
            kind: RowKind::Status,
            label: "Workspace".to_string(),
            detail: if input.workspace_root.is_empty() {
                "Unknown".to_string()
            } else {
                input.workspace_root.clone()
            },
            state: String::new(),
            checked: None,
            enabled: false,
            action: None,
        });
        rows.push(Row {
            key: "proxy-port".to_string(),
            kind: RowKind::Status,
            label: "Proxy port".to_string(),
            detail: format!(":{}", input.proxy_port),
            state: String::new(),
            checked: None,
            enabled: false,
            action: None,
        });
        rows.push(Row {
            key: "helper".to_string(),
            kind: RowKind::Status,
            label: "Helper".to_string(),
            detail: input.helper.clone(),
            state: String::new(),
            checked: None,
            enabled: false,
            action: None,
        });
        rows.push(Row {
            key: "status-separator".to_string(),
            kind: RowKind::Separator,
            label: String::new(),
            detail: String::new(),
            state: String::new(),
            checked: None,
            enabled: false,
            action: None,
        });

        rows.push(Row {
            key: "databases".to_string(),
            kind: RowKind::Header,
            label: "Databases".to_string(),
            detail: if input.instances.is_empty() {
                String::new()
            } else {
                format!("{instances_running}/{}", input.instances.len())
            },
            state: String::new(),
            checked: None,
            enabled: false,
            action: None,
        });
        if input.instances.is_empty() {
            rows.push(Row {
                key: "no-instances".to_string(),
                kind: RowKind::Instance,
                label: "No instances".to_string(),
                detail: "New database instance".to_string(),
                state: String::new(),
                checked: None,
                enabled: true,
                action: Some(Action::OpenNewInstance),
            });
        }
        for instance in &input.instances {
            let status = if instance.status.is_empty() {
                "Unknown".to_string()
            } else {
                instance.status.clone()
            };
            let on = running_status(&status);
            let busy = input.busy || input.quit_open || busy_status(&status);
            rows.push(Row {
                key: format!("instance:{}", instance.id),
                kind: RowKind::Instance,
                label: instance.id.clone(),
                detail: format!("{} · :{} · {}", instance.title, instance.port, status),
                state: status.clone(),
                checked: None,
                enabled: true,
                action: Some(Action::OpenInstance(instance.id.clone())),
            });
            rows.push(Row {
                key: format!("instance:{}:toggle", instance.id),
                kind: RowKind::Instance,
                label: if on {
                    format!("Stop {}", instance.id)
                } else {
                    format!("Start {}", instance.id)
                },
                detail: status.clone(),
                state: status,
                checked: Some(on),
                enabled: !busy,
                action: Some(Action::ToggleInstance {
                    id: instance.id.clone(),
                    start: !on,
                }),
            });
        }

        rows.push(Row {
            key: "https".to_string(),
            kind: RowKind::Header,
            label: "HTTPS".to_string(),
            detail: String::new(),
            state: String::new(),
            checked: None,
            enabled: false,
            action: None,
        });
        rows.push(Row {
            key: "proxy".to_string(),
            kind: RowKind::Proxy,
            label: format!("Proxy · {}", input.proxy_status),
            detail: format!(
                "{} · :{}",
                store::software::plural(input.proxy_rules, "rule"),
                input.proxy_port
            ),
            state: input.proxy_status.clone(),
            checked: Some(proxy_on),
            enabled: !input.busy && !input.quit_open && (proxy_on || input.proxy_rules > 0),
            action: Some(Action::ToggleProxy(!proxy_on)),
        });
        if input.tunnel_configured {
            let detail = if tunnel_on && !input.tunnel_detail.is_empty() {
                input.tunnel_detail.clone()
            } else {
                match input.tunnel_workers {
                    0 => "Cloudflare Workers".to_string(),
                    1 => "1 worker".to_string(),
                    n => format!("{n} workers"),
                }
            };
            rows.push(Row {
                key: "tunnel".to_string(),
                kind: RowKind::Tunnel,
                label: format!("Tunnel · {}", input.tunnel_status),
                detail,
                state: input.tunnel_status.clone(),
                checked: Some(tunnel_on),
                enabled: !input.busy && !input.quit_open && (tunnel_on || input.tunnel_rules > 0),
                action: Some(Action::ToggleTunnel(!tunnel_on)),
            });
        }

        let visible_services: Vec<&ServiceRow> = input
            .services
            .iter()
            .filter(|svc| !is_internal_devbase_service(&svc.name, &svc.id))
            .collect();
        if !visible_services.is_empty() {
            let services_running = visible_services.iter().filter(|svc| svc.running).count();
            rows.push(Row {
                key: "services".to_string(),
                kind: RowKind::Header,
                label: "Services".to_string(),
                detail: format!("{services_running}/{}", visible_services.len()),
                state: String::new(),
                checked: None,
                enabled: false,
                action: None,
            });
            for svc in visible_services {
                rows.push(Row {
                    key: format!("service:{}", svc.id),
                    kind: RowKind::Service,
                    label: svc.name.clone(),
                    detail: format!("{} · {}", svc.source.title(), svc.status),
                    state: svc.status.clone(),
                    checked: None,
                    enabled: true,
                    action: Some(Action::OpenService {
                        source: svc.source,
                        id: svc.id.clone(),
                    }),
                });
                if svc.can_toggle {
                    rows.push(Row {
                        key: format!("service:{}:toggle", svc.id),
                        kind: RowKind::Service,
                        label: if svc.running {
                            format!("Stop {}", svc.name)
                        } else {
                            format!("Start {}", svc.name)
                        },
                        detail: svc.status.clone(),
                        state: svc.status.clone(),
                        checked: Some(svc.running),
                        enabled: !input.quit_open && !svc.busy,
                        action: Some(Action::ToggleService {
                            id: svc.id.clone(),
                            start: !svc.running,
                        }),
                    });
                }
            }
        }

        rows.push(Row {
            key: "footer-separator".to_string(),
            kind: RowKind::Separator,
            label: String::new(),
            detail: String::new(),
            state: String::new(),
            checked: None,
            enabled: false,
            action: None,
        });
        rows.push(Row {
            key: "open".to_string(),
            kind: RowKind::Action,
            label: "Open Devbase".to_string(),
            detail: String::new(),
            state: String::new(),
            checked: None,
            enabled: true,
            action: Some(Action::OpenWindow),
        });
        rows.push(Row {
            key: "refresh".to_string(),
            kind: RowKind::Action,
            label: "Refresh".to_string(),
            detail: String::new(),
            state: String::new(),
            checked: None,
            enabled: true,
            action: Some(Action::Refresh),
        });
        rows.push(Row {
            key: "quit".to_string(),
            kind: RowKind::Action,
            label: "Quit Devbase".to_string(),
            detail: String::new(),
            state: String::new(),
            checked: None,
            enabled: true,
            action: Some(Action::Quit),
        });

        Panel {
            title: if running > 0 {
                running.to_string()
            } else {
                String::new()
            },
            tooltip: if failed > 0 {
                format!("Devbase: {running} running · {failed} failed")
            } else {
                format!("Devbase: {running} running")
            },
            running,
            failed,
            state,
            summary,
            rows,
        }
    }
}

fn helper_label(status: &system::HelperStatus) -> String {
    if !status.message.is_empty() {
        status.message.clone()
    } else if status.installed {
        "Installed".to_string()
    } else {
        "Not installed".to_string()
    }
}

fn instance_title(kind: &str) -> String {
    SoftwareKind::from_key(kind)
        .map(|kind| kind.title().to_string())
        .unwrap_or_else(|| kind.to_string())
}

fn tunnel_configured(rules: &[devcore::model::ProxyRule]) -> bool {
    proxy::token_configured()
        || proxy::worker_count() > 0
        || proxy::tunnel_on()
        || rules.iter().any(|rule| rule.tunnel)
}

fn service_status_label(state: ServiceState) -> &'static str {
    match state {
        ServiceState::Running => "Running",
        ServiceState::Scheduled => "Scheduled",
        ServiceState::Failed => "Failed",
        ServiceState::Loaded => "Idle",
        ServiceState::Invalid => "Invalid",
        ServiceState::Stopped | ServiceState::None => "Stopped",
    }
}

fn service_group_title(source: Source) -> &'static str {
    match source {
        Source::Brew => "Homebrew",
        Source::UserAgent => "Login Agents",
        other => other.title(),
    }
}

fn service_group_icon(source: Source) -> &'static str {
    match source {
        Source::Brew => LAYERS_BODY,
        Source::UserAgent => TERMINAL_BODY,
        _ => LAYERS_BODY,
    }
}

fn loaded_services_for(source: Source) -> Vec<ServiceRow> {
    let provider = services::provider(source);
    let mut rows = Vec::new();
    for item in services::loaded_items(source) {
        if is_internal_devbase_service(&item.name, &item.id) {
            continue;
        }
        let running = item.up();
        let can_toggle = provider.as_ref().is_some_and(|p| {
            let actions = p.actions(&item);
            if running {
                actions
                    .iter()
                    .any(|a| matches!(a, ServiceAction::Stop | ServiceAction::Unload))
            } else {
                actions
                    .iter()
                    .any(|a| matches!(a, ServiceAction::Start | ServiceAction::Load))
            }
        });
        rows.push(ServiceRow {
            busy: services::is_busy(&item.id),
            status: service_status_label(item.state).to_string(),
            id: item.id,
            source,
            name: item.name,
            running,
            can_toggle,
        });
    }
    rows
}

fn loaded_service_rows() -> Vec<ServiceRow> {
    let mut rows = loaded_services_for(Source::Brew);
    rows.extend(loaded_services_for(Source::UserAgent));
    rows
}

/// Reads the stores the tray shows. Call inside a reactive context (a
/// component or an effect); every read is tracked.
pub fn panel() -> Panel {
    let instances: Vec<InstanceRow> = databases::instances()
        .into_iter()
        .map(|instance| InstanceRow {
            title: instance_title(&instance.kind),
            port: instance.port,
            status: databases::status(&instance.id),
            id: instance.id,
        })
        .collect();
    let services = loaded_service_rows();
    let rules = proxy::saved_rules();
    let proxy_status = proxy::proxy_status();
    let tunnel_status = proxy::tunnel_status();
    let helper = system::helper_status();
    Panel::from(PanelInput {
        instances,
        services,
        proxy_status,
        proxy_rules: rules.len(),
        proxy_port: proxy::listener_port(),
        busy: proxy::busy() || databases::operations_busy(),
        tunnel_status,
        tunnel_detail: proxy::tunnel_detail(),
        tunnel_rules: rules.iter().filter(|rule| rule.tunnel).count(),
        tunnel_workers: proxy::worker_count(),
        tunnel_configured: tunnel_configured(&rules),
        helper: helper_label(&helper),
        workspace_root: system::workspace_root(),
        quit_open: system::quit_open(),
    })
}

/// Brings the main window forward, as Go's `showWindow` did.
fn show_window() {
    let window = main_window();
    window.show();
    window.focus();
}

/// Runs a row's action. Menu and popover handlers call this; every call
/// returns at once.
pub fn dispatch(action: Action) {
    match action {
        Action::OpenInstance(id) => {
            close_popover();
            store::select_section(Section::Databases);
            databases::open_instance(&id);
            show_window();
        }
        Action::OpenNewInstance => {
            close_popover();
            store::select_section(Section::Databases);
            databases::open_new();
            show_window();
        }
        Action::ToggleInstance { id, start } => {
            if start {
                databases::start_instance(&id);
            } else {
                databases::stop_instance(&id);
            }
        }
        Action::ToggleProxy(on) => proxy::set_proxy_enabled(on),
        Action::ToggleTunnel(on) => proxy::set_tunnel_enabled(on),
        Action::OpenHttpsProxy => {
            close_popover();
            store::select_section(Section::HttpsProxy);
            show_window();
        }
        Action::OpenService { source, id } => {
            close_popover();
            store::select_section(Section::Services);
            services::open_item(source, &id);
            show_window();
        }
        Action::ToggleService { id, start } => {
            if let Some(item) = services::item_by_id(&id)
                && let Some(provider) = services::provider(item.source)
            {
                let actions = provider.actions(&item);
                let chosen = if start {
                    actions
                        .iter()
                        .copied()
                        .find(|action| matches!(action, ServiceAction::Start | ServiceAction::Load))
                } else {
                    actions.iter().copied().find(|action| {
                        matches!(action, ServiceAction::Stop | ServiceAction::Unload)
                    })
                };
                if let Some(service_action) = chosen {
                    services::request_action(item, service_action);
                }
            }
        }
        Action::OpenWindow => {
            close_popover();
            show_window();
        }
        Action::Refresh => refresh(),
        Action::Quit => {
            close_popover();
            system::request_quit();
        }
    }
}

/// Reads the workspace, proxy, settings, and any loaded services again.
pub fn refresh() {
    databases::refresh();
    proxy::refresh();
    system::refresh_settings();
    services::refresh_if_loaded(Source::Brew);
    services::refresh_if_loaded(Source::UserAgent);
}

/// Reads what the tray shows the first time the app starts.
pub fn load() {
    databases::refresh();
    proxy::refresh();
    system::load_settings();
}

// --- Native NSPanel popover --------------------------------------------------

#[derive(Debug, Clone, Default)]
struct PopoverState {
    open: bool,
    anchor: Option<Bounds<Points>>,
    last_blur_close: Option<Instant>,
}

#[derive(Debug, Clone)]
struct PopoverSectionState {
    brew_expanded: bool,
    user_agent_expanded: bool,
}

impl Default for PopoverSectionState {
    fn default() -> Self {
        Self {
            brew_expanded: true,
            user_agent_expanded: true,
        }
    }
}

fn popover_state() -> Signal<PopoverState> {
    global::<PopoverState>()
}

fn popover_sections() -> Signal<PopoverSectionState> {
    global::<PopoverSectionState>()
}

fn section_expanded(source: Source) -> bool {
    popover_sections().with(|state| match source {
        Source::Brew => state.brew_expanded,
        Source::UserAgent => state.user_agent_expanded,
        _ => true,
    })
}

fn toggle_service_section(source: Source) {
    if !reduce_motion() {
        animate_layout(LayoutAnimation::all(LayoutTransition::new(
            LayoutCurve::EaseInEaseOut,
            180.0,
        )));
    }
    popover_sections().update(|state| match source {
        Source::Brew => state.brew_expanded = !state.brew_expanded,
        Source::UserAgent => state.user_agent_expanded = !state.user_agent_expanded,
        _ => {}
    });
}

/// Whether the native tray popover is currently open.
pub fn popover_open() -> bool {
    popover_state().with(|state| state.open)
}

/// Closes the tray popover if open.
pub fn close_popover() {
    popover_state().update(|state| {
        state.open = false;
        state.last_blur_close = None;
    });
}

fn close_popover_with_cause(cause: WindowCloseCause) {
    let blur_at = (cause == WindowCloseCause::Blur).then(Instant::now);
    popover_state().update(|state| {
        state.open = false;
        state.last_blur_close = blur_at;
    });
}

/// Returns `false` when the popover just closed due to focus loss (`Blur`)
/// within [`BLUR_REOPEN_DEBOUNCE`], preventing a tray icon click that closed
/// the panel on mouse-down from immediately bouncing it open on mouse-up.
pub fn should_reopen_after_blur(last_blur: Option<Instant>, now: Instant) -> bool {
    match last_blur {
        Some(closed_at) => now.saturating_duration_since(closed_at) >= BLUR_REOPEN_DEBOUNCE,
        None => true,
    }
}

/// Toggles the tray popover on a primary tray click, anchoring it to the
/// clicked status item's screen bounds while suppressing blur-reopen bounce.
fn toggle_popover(event: &TrayClickEvent) {
    let now = Instant::now();
    let mut opened = false;
    popover_state().update(|state| {
        if state.open {
            state.open = false;
            state.last_blur_close = None;
        } else if !should_reopen_after_blur(state.last_blur_close, now) {
            state.last_blur_close = None;
        } else {
            state.anchor = event.bounds;
            state.last_blur_close = None;
            state.open = true;
            opened = true;
        }
    });
    if opened {
        refresh();
    }
}

/// Toggles the tray popover from the global shortcut (`⌃⌥⌘D`), reusing the
/// last known tray anchor if available or falling back to top-right placement.
pub fn toggle_popover_shortcut() {
    let mut opened = false;
    popover_state().update(|state| {
        if state.open {
            state.open = false;
            state.last_blur_close = None;
        } else {
            state.last_blur_close = None;
            state.open = true;
            opened = true;
        }
    });
    if opened {
        refresh();
    }
}

/// Computes the popover's top-left screen point within `screens`:
/// - Beneath `anchor` (centered horizontally and clamped inside the owning
///   display's `visible_bounds` inset by `POPOVER_MARGIN`) when `anchor` is
///   `Some`.
/// - At the top-right of the primary display's `visible_bounds` (under the
///   menu bar, inset by `POPOVER_MARGIN`) when `anchor` is `None`.
pub fn anchor_position_in(
    anchor: Option<Bounds<Points>>,
    screens: &[Display],
) -> Option<Point<Points>> {
    if let Some(bounds) = anchor {
        let center_x = bounds.origin.x.0 + bounds.size.width.0 / 2.0;
        let mut x = (center_x - POPOVER_WIDTH / 2.0).round();
        let y = (bounds.origin.y.0 + bounds.size.height.0 + 6.0).round();

        let target = screens
            .iter()
            .find(|d| {
                let left = d.bounds.origin.x.0;
                let right = left + d.bounds.size.width.0;
                let top = d.bounds.origin.y.0;
                let bottom = top + d.bounds.size.height.0;
                center_x >= left
                    && center_x <= right
                    && bounds.origin.y.0 >= top
                    && bounds.origin.y.0 <= bottom
            })
            .or_else(|| screens.iter().find(|d| d.primary))
            .or_else(|| screens.first());

        if let Some(display) = target {
            let min_x = display.visible_bounds.origin.x.0 + POPOVER_MARGIN;
            let max_x = (display.visible_bounds.origin.x.0 + display.visible_bounds.size.width.0
                - POPOVER_WIDTH
                - POPOVER_MARGIN)
                .max(min_x);
            x = x.clamp(min_x, max_x);
        }

        return Some(point(px(x), px(y)));
    }

    let display = screens
        .iter()
        .find(|d| d.primary)
        .or_else(|| screens.first())?;
    let min_x = display.visible_bounds.origin.x.0 + POPOVER_MARGIN;
    let x = (display.visible_bounds.origin.x.0 + display.visible_bounds.size.width.0
        - POPOVER_WIDTH
        - POPOVER_MARGIN)
        .max(min_x)
        .round();
    let y = (display.visible_bounds.origin.y.0 + POPOVER_MARGIN).round();
    Some(point(px(x), px(y)))
}

fn anchor_position(anchor: Option<Bounds<Points>>) -> Option<Point<Points>> {
    anchor_position_in(anchor, &displays())
}

fn estimated_popover_height(panel: &Panel) -> f32 {
    let instance_count = panel
        .rows
        .iter()
        .filter(|row| row.kind == RowKind::Instance && row.checked.is_some())
        .count()
        .max(1);
    let brew_count = panel
        .rows
        .iter()
        .filter(|row| {
            row.kind == RowKind::Service
                && row.checked.is_none()
                && matches!(
                    row.action,
                    Some(Action::OpenService {
                        source: Source::Brew,
                        ..
                    })
                )
        })
        .count();
    let agent_count = panel
        .rows
        .iter()
        .filter(|row| {
            row.kind == RowKind::Service
                && row.checked.is_none()
                && matches!(
                    row.action,
                    Some(Action::OpenService {
                        source: Source::UserAgent,
                        ..
                    })
                )
        })
        .count();
    let has_tunnel = panel.rows.iter().any(|row| row.kind == RowKind::Tunnel);
    let brew_height = if brew_count > 0 {
        26.0 + if untracked(|| section_expanded(Source::Brew)) {
            brew_count as f32 * 44.0
        } else {
            0.0
        }
    } else {
        0.0
    };
    let agent_height = if agent_count > 0 {
        26.0 + if untracked(|| section_expanded(Source::UserAgent)) {
            agent_count as f32 * 44.0
        } else {
            0.0
        }
    } else {
        0.0
    };
    let height = 164.0
        + (instance_count as f32 * 44.0)
        + if has_tunnel { 96.0 } else { 52.0 }
        + brew_height
        + agent_height
        + 76.0;
    height.clamp(POPOVER_MIN_HEIGHT, POPOVER_MAX_HEIGHT)
}

/// The floating `WindowKind::Popup` `NSPanel` anchored beneath the tray icon.
/// Mounted by [`crate::App`] on desktop platforms.
pub(crate) fn popover_window() -> impl IntoElement {
    when(popover_open, move || {
        let anchor = popover_state().with_untracked(|state| state.anchor);
        let initial_height = untracked(|| estimated_popover_height(&panel()));

        let mut attrs = WindowAttributes::default();
        attrs.close_on_blur = Some(true);
        attrs.close_on_escape = Some(true);
        attrs.transparent = Some(true);
        attrs.shadow = Some(true);
        attrs.resizable = Some(false);
        attrs.size_to_content = Some(SizeToContent::Height);
        attrs.min_size = Some(Some(size(px(POPOVER_WIDTH), px(POPOVER_MIN_HEIGHT))));
        attrs.max_size = Some(Some(size(px(POPOVER_WIDTH), px(POPOVER_MAX_HEIGHT))));

        let mut popup = window()
            .id("menubar-panel")
            .kind(WindowKind::Popup)
            .default_size(size(px(POPOVER_WIDTH), px(initial_height)))
            .attributes(attrs)
            .open(true)
            .on_open_change(|change| {
                if !change.open {
                    close_popover_with_cause(change.reason);
                }
            })
            .content(popover_content);

        if let Some(pos) = anchor_position(anchor) {
            popup = popup.default_position(WindowPosition::At(pos));
        }

        popup
    })
}

fn status_dot_color(state: &str) -> AdaptiveColor {
    match state {
        "Running" | "Healthy" => tokens::success(),
        "Failed" => theme().destructive,
        "Starting" | "Stopping" | "Restarting" => tokens::warning(),
        _ => theme().muted_foreground.opacity(0.55),
    }
}

fn status_dot(state: impl IntoReactive<String> + 'static) -> impl IntoElement {
    let state = state.into_reactive();
    view()
        .w(px(7.0))
        .h(px(7.0))
        .flex_none()
        .rounded(px(4.0))
        .bg(move || status_dot_color(&state.get()))
}

fn popover_switch(
    checked: impl IntoReactive<bool> + 'static,
    disabled: impl IntoReactive<bool> + 'static,
    label: String,
    test_id: String,
    on_change: impl Fn(bool) + 'static,
) -> impl IntoElement {
    let checked = checked.into_reactive();
    let disabled = disabled.into_reactive();
    let checked_track = checked.clone();
    let checked_thumb = checked.clone();
    let disabled_opacity = disabled.clone();
    switch()
        .checked(move || checked.get())
        .disabled(move || disabled.get())
        .accessibility_label(label)
        .test_id(&test_id)
        .flex_none()
        .rounded(px(9.0))
        .focus_visible(|s| s.ring(px(2.0)).ring_color(move || theme().ring))
        .opacity(move || {
            if disabled_opacity.get() {
                0.45_f32
            } else {
                1.0_f32
            }
        })
        .track_style(move |s| {
            let checked = checked_track.clone();
            s.w(px(32.0))
                .h(px(18.0))
                .p(px(2.0))
                .rounded(px(9.0))
                .flex_row()
                .items_center()
                .bg(move || {
                    if checked.get() {
                        theme().primary
                    } else {
                        theme().foreground.opacity(0.18)
                    }
                })
        })
        .thumb_style(move |s| {
            let checked = checked_thumb.clone();
            s.w(px(14.0))
                .h(px(14.0))
                .rounded(px(7.0))
                .bg(AdaptiveColor::rgb(0xffffff, 0xffffff))
                .ml(move || if checked.get() { px(14.0) } else { px(0.0) })
        })
        .on_change(move |value: &bool| {
            haptic(Haptic::Selection);
            on_change(*value);
        })
}

fn section_heading(
    title: &'static str,
    detail: impl IntoReactive<String> + 'static,
) -> impl IntoElement {
    let detail = detail.into_reactive();
    view()
        .w_full()
        .flex_row()
        .items_center()
        .justify_between()
        .px(px(6.0))
        .pt(px(6.0))
        .pb(px(4.0))
        .child(
            text(title)
                .text_size(px(tokens::TEXT_CAPTION))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(move || theme().muted_foreground),
        )
        .child(
            text(move || detail.get())
                .text_size(px(tokens::TEXT_CAPTION))
                .tabular_nums()
                .text_color(move || theme().muted_foreground),
        )
}

fn popover_busy() -> bool {
    proxy::busy() || databases::operations_busy() || system::quit_open()
}

fn effective_instance_status(status: &str) -> String {
    if status.is_empty() {
        "Unknown".to_string()
    } else {
        status.to_string()
    }
}

fn database_instance_rows() -> Vec<InstanceRow> {
    databases::instances()
        .into_iter()
        .map(|instance| InstanceRow {
            title: instance_title(&instance.kind),
            port: instance.port,
            status: effective_instance_status(&databases::status(&instance.id)),
            id: instance.id,
        })
        .collect()
}

fn header_status_summary() -> (String, String) {
    let instances = databases::instances();
    let instances_running = instances
        .iter()
        .filter(|inst| running_status(&databases::status(&inst.id)))
        .count();
    let instances_failed = instances
        .iter()
        .filter(|inst| databases::status(&inst.id) == "Failed")
        .count();
    let proxy_status = proxy::proxy_status();
    let tunnel_status = proxy::tunnel_status();
    let running = instances_running
        + usize::from(proxy_status == "Running")
        + usize::from(tunnel_status == "Running");
    let failed = instances_failed
        + usize::from(proxy_status == "Failed")
        + usize::from(tunnel_status == "Failed");
    let state = if failed > 0 {
        "Failed"
    } else if running > 0 {
        "Running"
    } else {
        "Stopped"
    }
    .to_string();
    let summary = if failed > 0 {
        format!("{running} running · {failed} failed")
    } else {
        format!("{running} running")
    };
    (state, summary)
}

/// The native tray popover's interior UI. Rendered inside the `WindowKind::Popup`
/// `NSPanel` on desktop and mountable directly in headless tests.
pub fn popover_content() -> impl IntoElement {
    view()
        .test_id("menubar-popover")
        .relative()
        .w_full()
        .min_h(px(POPOVER_MIN_HEIGHT))
        .max_h(px(POPOVER_MAX_HEIGHT))
        .flex_col()
        .rounded(px(12.0))
        .overflow_hidden()
        .border_1()
        .border_color(tokens::hairline)
        .bg(move || {
            if os() == Os::MacOs {
                AdaptiveColor::TRANSPARENT
            } else {
                tokens::panel()
            }
        })
        .text_color(move || theme().foreground)
        .child(when(
            || os() == Os::MacOs,
            || {
                view()
                    .test_id("menubar-popover-glass")
                    .absolute()
                    .top(px(0.0))
                    .right(px(0.0))
                    .bottom(px(0.0))
                    .left(px(0.0))
                    .rounded(px(12.0))
                    .pointer_events(PointerEvents::None)
                    .glass_effect(GlassEffect::Regular)
            },
        ))
        .child(popover_header())
        .child(view().h(px(1.0)).w_full().flex_none().bg(tokens::hairline))
        .child(
            scroll()
                .test_id("menubar-popover-body")
                .flex_auto()
                .min_h(px(0.0))
                .px(px(8.0))
                .py(px(6.0))
                .child(popover_body()),
        )
        .child(view().h(px(1.0)).w_full().flex_none().bg(tokens::hairline))
        .child(popover_footer())
}

fn popover_header() -> impl IntoElement {
    view()
        .test_id("menubar-popover-header")
        .w_full()
        .flex_none()
        .flex_row()
        .items_center()
        .gap(px(10.0))
        .px(px(12.0))
        .py(px(10.0))
        .child(
            view()
                .size(px(30.0))
                .flex_none()
                .items_center()
                .justify_center()
                .rounded(px(8.0))
                .bg(move || theme().primary)
                .child(
                    icon_svg(PACKAGE_BODY)
                        .size(px(16.0))
                        .text_color(move || theme().primary_foreground),
                ),
        )
        .child(
            view()
                .flex_1()
                .min_w(px(0.0))
                .flex_col()
                .gap(px(2.0))
                .child(
                    text("Devbase")
                        .text_size(px(14.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(move || theme().foreground),
                )
                .child(
                    view()
                        .flex_row()
                        .items_center()
                        .gap(px(6.0))
                        .child(status_dot(move || header_status_summary().0))
                        .child(
                            text(move || header_status_summary().1)
                                .test_id("popover-summary")
                                .text_size(px(tokens::TEXT_CAPTION))
                                .tabular_nums()
                                .text_color(move || theme().muted_foreground),
                        ),
                ),
        )
        .child(
            view()
                .flex_none()
                .tooltip(controls::tooltip(format!(
                    "Toggle panel ({GLOBAL_SHORTCUT_LABEL})"
                )))
                .px(px(5.0))
                .py(px(2.0))
                .rounded(px(4.0))
                .bg(tokens::inset)
                .border_1()
                .border_color(tokens::hairline)
                .child(
                    text(GLOBAL_SHORTCUT_LABEL)
                        .test_id("popover-shortcut-hint")
                        .text_size(px(10.0))
                        .text_color(move || theme().muted_foreground),
                ),
        )
}

fn popover_no_instances_row() -> impl IntoElement {
    button()
        .test_id("popover-no-instances")
        .w_full()
        .flex_row()
        .items_center()
        .justify_between()
        .px(px(8.0))
        .py(px(7.0))
        .rounded(move || theme().radius)
        .hover(|s| s.bg(tokens::row_hover()))
        .on_press(|| dispatch(Action::OpenNewInstance))
        .child(
            view()
                .flex_col()
                .gap(px(1.0))
                .child(
                    text("No instances")
                        .text_size(px(tokens::TEXT_BODY))
                        .text_color(move || theme().foreground),
                )
                .child(
                    text("New database instance")
                        .text_size(px(tokens::TEXT_CAPTION))
                        .text_color(move || theme().muted_foreground),
                ),
        )
        .child(
            icon_svg(PLUS_BODY)
                .size(px(14.0))
                .text_color(move || theme().muted_foreground),
        )
}

fn popover_instance_row(row: ReadSignal<InstanceRow>) -> impl IntoElement {
    let id = row.with_untracked(|r| r.id.clone());
    let open_id = id.clone();
    let toggle_id = id.clone();
    view()
        .test_id(&format!("popover-instance-{id}"))
        .w_full()
        .flex_row()
        .items_center()
        .gap(px(8.0))
        .px(px(6.0))
        .py(px(5.0))
        .rounded(move || theme().radius)
        .hover(|s| s.bg(tokens::row_hover()))
        .child(
            button()
                .test_id(&format!("popover-open-instance-{id}"))
                .flex_1()
                .min_w(px(0.0))
                .flex_row()
                .items_center()
                .justify_start()
                .gap(px(8.0))
                .on_press(move || dispatch(Action::OpenInstance(open_id.clone())))
                .child(status_dot(move || row.with(|r| r.status.clone())))
                .child(
                    view()
                        .flex_1()
                        .min_w(px(0.0))
                        .flex_col()
                        .gap(px(1.0))
                        .child(
                            text(id.clone())
                                .text_size(px(tokens::TEXT_BODY))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(move || theme().foreground),
                        )
                        .child(
                            text(move || {
                                row.with(|r| format!("{} · :{} · {}", r.title, r.port, r.status))
                            })
                            .text_size(px(tokens::TEXT_CAPTION))
                            .tabular_nums()
                            .text_color(move || theme().muted_foreground)
                            .truncate(),
                        ),
                ),
        )
        .child(popover_switch(
            move || row.with(|r| running_status(&r.status)),
            move || row.with(|r| popover_busy() || busy_status(&r.status)),
            format!("Toggle {id}"),
            format!("popover-switch-instance-{id}"),
            move |start| {
                dispatch(Action::ToggleInstance {
                    id: toggle_id.clone(),
                    start,
                });
            },
        ))
}

fn popover_proxy_row() -> impl IntoElement {
    view()
        .test_id("popover-proxy")
        .w_full()
        .flex_row()
        .items_center()
        .gap(px(8.0))
        .px(px(6.0))
        .py(px(5.0))
        .rounded(move || theme().radius)
        .hover(|s| s.bg(tokens::row_hover()))
        .child(
            button()
                .test_id("popover-open-proxy")
                .flex_1()
                .min_w(px(0.0))
                .flex_row()
                .items_center()
                .justify_start()
                .gap(px(8.0))
                .on_press(|| dispatch(Action::OpenHttpsProxy))
                .child(status_dot(proxy::proxy_status))
                .child(
                    view()
                        .flex_1()
                        .min_w(px(0.0))
                        .flex_col()
                        .gap(px(1.0))
                        .child(
                            text(move || format!("Proxy · {}", proxy::proxy_status()))
                                .text_size(px(tokens::TEXT_BODY))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(move || theme().foreground)
                                .truncate(),
                        )
                        .child(
                            text(move || {
                                format!(
                                    "{} · :{}",
                                    store::software::plural(proxy::saved_rules().len(), "rule"),
                                    proxy::listener_port()
                                )
                            })
                            .text_size(px(tokens::TEXT_CAPTION))
                            .tabular_nums()
                            .text_color(move || theme().muted_foreground)
                            .truncate(),
                        ),
                ),
        )
        .child(popover_switch(
            move || proxy::proxy_status() == "Running",
            move || {
                let on = proxy::proxy_status() == "Running";
                popover_busy() || (!on && proxy::saved_rules().is_empty())
            },
            "Toggle HTTPS proxy".to_string(),
            "popover-switch-proxy".to_string(),
            |on| dispatch(Action::ToggleProxy(on)),
        ))
}

fn tunnel_row_detail() -> String {
    let tunnel_on = proxy::tunnel_status() == "Running";
    let detail = proxy::tunnel_detail();
    if tunnel_on && !detail.is_empty() {
        detail
    } else {
        match proxy::worker_count() {
            0 => "Cloudflare Workers".to_string(),
            1 => "1 worker".to_string(),
            n => format!("{n} workers"),
        }
    }
}

fn popover_tunnel_row() -> impl IntoElement {
    view()
        .test_id("popover-tunnel")
        .w_full()
        .flex_row()
        .items_center()
        .gap(px(8.0))
        .px(px(6.0))
        .py(px(5.0))
        .rounded(move || theme().radius)
        .hover(|s| s.bg(tokens::row_hover()))
        .child(
            button()
                .test_id("popover-open-tunnel")
                .flex_1()
                .min_w(px(0.0))
                .flex_row()
                .items_center()
                .justify_start()
                .gap(px(8.0))
                .on_press(|| dispatch(Action::OpenHttpsProxy))
                .child(status_dot(proxy::tunnel_status))
                .child(
                    view()
                        .flex_1()
                        .min_w(px(0.0))
                        .flex_col()
                        .gap(px(1.0))
                        .child(
                            text(move || format!("Tunnel · {}", proxy::tunnel_status()))
                                .text_size(px(tokens::TEXT_BODY))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(move || theme().foreground)
                                .truncate(),
                        )
                        .child(
                            text(tunnel_row_detail)
                                .text_size(px(tokens::TEXT_CAPTION))
                                .tabular_nums()
                                .text_color(move || theme().muted_foreground)
                                .truncate(),
                        ),
                ),
        )
        .child(popover_switch(
            move || proxy::tunnel_status() == "Running",
            move || {
                let on = proxy::tunnel_status() == "Running";
                let has_rules = proxy::saved_rules().iter().any(|rule| rule.tunnel);
                popover_busy() || (!on && !has_rules)
            },
            "Toggle Cloudflare tunnel".to_string(),
            "popover-switch-tunnel".to_string(),
            |on| dispatch(Action::ToggleTunnel(on)),
        ))
}

fn popover_service_row(svc: ReadSignal<ServiceRow>) -> impl IntoElement {
    let initial = svc.get_untracked();
    let name = initial.name;
    let source = initial.source;
    let open_id = initial.id.clone();
    let toggle_id = initial.id;
    let toggle_name = name.clone();
    view()
        .test_id(&format!("popover-service-{name}"))
        .w_full()
        .flex_row()
        .items_center()
        .gap(px(8.0))
        .px(px(6.0))
        .py(px(5.0))
        .rounded(move || theme().radius)
        .hover(|s| s.bg(tokens::row_hover()))
        .child(
            button()
                .test_id(&format!("popover-open-service-{name}"))
                .flex_1()
                .min_w(px(0.0))
                .flex_row()
                .items_center()
                .justify_start()
                .gap(px(8.0))
                .on_press(move || {
                    dispatch(Action::OpenService {
                        source,
                        id: open_id.clone(),
                    });
                })
                .child(status_dot(move || svc.with(|s| s.status.clone())))
                .child(
                    view()
                        .flex_1()
                        .min_w(px(0.0))
                        .flex_col()
                        .gap(px(1.0))
                        .child(
                            text(name.clone())
                                .text_size(px(tokens::TEXT_BODY))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(move || theme().foreground)
                                .truncate(),
                        )
                        .child(
                            text(move || {
                                svc.with(|s| format!("{} · {}", s.source.title(), s.status))
                            })
                            .text_size(px(tokens::TEXT_CAPTION))
                            .text_color(move || theme().muted_foreground)
                            .truncate(),
                        ),
                ),
        )
        .child(when(
            move || svc.with(|s| s.can_toggle),
            move || {
                let toggle_id = toggle_id.clone();
                let toggle_name = toggle_name.clone();
                popover_switch(
                    move || svc.with(|s| s.running),
                    move || svc.with(|s| system::quit_open() || s.busy),
                    format!("Toggle {toggle_name}"),
                    format!("popover-switch-service-{toggle_name}"),
                    move |start| {
                        dispatch(Action::ToggleService {
                            id: toggle_id.clone(),
                            start,
                        });
                    },
                )
            },
        ))
}

fn collapsible_service_section(source: Source, section_key: &'static str) -> impl IntoElement {
    let title = service_group_title(source);
    let toggle_test_id = format!("popover-section-{section_key}-toggle");
    let count_test_id = format!("popover-section-{section_key}-count");
    collapsible()
        .w_full()
        .flex_col()
        .open(move || section_expanded(source))
        .child(
            view()
                .test_id(&toggle_test_id)
                .role(AccessibilityRole::Button)
                .label(format!("Toggle {title} section"))
                .accessibility_state(move || AccessibilityState {
                    expanded: Some(section_expanded(source)),
                    ..AccessibilityState::default()
                })
                .focus_visible(|s| s.ring(px(2.0)).ring_color(move || theme().ring))
                .w_full()
                .flex_row()
                .items_center()
                .justify_between()
                .px(px(6.0))
                .pt(px(6.0))
                .pb(px(4.0))
                .rounded(move || theme().radius)
                .hover(|s| s.bg(tokens::row_hover()))
                .on_press(move |_: &()| toggle_service_section(source))
                .child(
                    view()
                        .flex_row()
                        .items_center()
                        .gap(px(4.0))
                        .child(
                            icon_svg(CHEVRON_RIGHT_BODY)
                                .size(px(12.0))
                                .flex_none()
                                .text_color(move || theme().muted_foreground)
                                .rotate(move || {
                                    if section_expanded(source) {
                                        deg(90.0)
                                    } else {
                                        deg(0.0)
                                    }
                                }),
                        )
                        .child(
                            text(title)
                                .text_size(px(tokens::TEXT_CAPTION))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(move || theme().muted_foreground),
                        ),
                )
                .child(
                    text(move || {
                        let items = loaded_services_for(source);
                        let running = items.iter().filter(|s| s.running).count();
                        format!("{running}/{}", items.len())
                    })
                    .test_id(&count_test_id)
                    .text_size(px(tokens::TEXT_CAPTION))
                    .tabular_nums()
                    .text_color(move || theme().muted_foreground),
                ),
        )
        .content(view().w_full().flex_col().gap(px(2.0)).child(each(
            move || loaded_services_for(source),
            |svc| svc.id.clone(),
            |svc, _| popover_service_row(svc),
        )))
}

fn popover_status_row(
    label: &'static str,
    value: impl IntoReactive<String> + 'static,
) -> impl IntoElement {
    let value = value.into_reactive();
    view()
        .w_full()
        .flex_row()
        .items_center()
        .justify_between()
        .gap(px(8.0))
        .child(
            text(label)
                .flex_none()
                .text_size(px(tokens::TEXT_CAPTION))
                .text_color(move || theme().muted_foreground),
        )
        .child(
            text(move || value.get())
                .min_w(px(0.0))
                .text_size(px(tokens::TEXT_CAPTION))
                .tabular_nums()
                .text_color(move || theme().foreground)
                .truncate(),
        )
}

fn popover_status_box() -> impl IntoElement {
    view()
        .w_full()
        .flex_col()
        .gap(px(3.0))
        .mt(px(6.0))
        .px(px(8.0))
        .py(px(6.0))
        .rounded(move || theme().radius)
        .bg(tokens::inset)
        .child(popover_status_row("Workspace", move || {
            let root = system::workspace_root();
            if root.is_empty() {
                "Unknown".to_string()
            } else {
                root
            }
        }))
        .child(popover_status_row("Proxy port", move || {
            format!(":{}", proxy::listener_port())
        }))
        .child(popover_status_row("Helper", move || {
            helper_label(&system::helper_status())
        }))
}

fn popover_body() -> impl IntoElement {
    view()
        .w_full()
        .flex_col()
        .gap(px(2.0))
        .child(section_heading("Databases", move || {
            let list = databases::instances();
            if list.is_empty() {
                String::new()
            } else {
                let running = list
                    .iter()
                    .filter(|inst| running_status(&databases::status(&inst.id)))
                    .count();
                format!("{running}/{}", list.len())
            }
        }))
        .child(when(
            move || databases::instances().is_empty(),
            popover_no_instances_row,
        ))
        .child(each(
            database_instance_rows,
            |row| row.id.clone(),
            |row, _| popover_instance_row(row),
        ))
        .child(section_heading("HTTPS", ""))
        .child(popover_proxy_row())
        .child(when(
            move || tunnel_configured(&proxy::saved_rules()),
            popover_tunnel_row,
        ))
        .child(when(
            move || !loaded_services_for(Source::Brew).is_empty(),
            || collapsible_service_section(Source::Brew, "brew"),
        ))
        .child(when(
            move || !loaded_services_for(Source::UserAgent).is_empty(),
            || collapsible_service_section(Source::UserAgent, "user-agent"),
        ))
        .child(popover_status_box())
}

fn footer_button(
    label: &'static str,
    test_id: &'static str,
    icon_body: &'static str,
    grow: bool,
    on_press: impl Fn() + 'static,
) -> impl IntoElement {
    let mut btn = button()
        .test_id(test_id)
        .accessibility_label(label)
        .h(px(28.0))
        .px(px(10.0))
        .flex_row()
        .items_center()
        .justify_center()
        .gap(px(6.0))
        .rounded(move || theme().radius)
        .bg(tokens::control)
        .border_1()
        .border_color(tokens::hairline)
        .hover(|s| s.bg(tokens::row_hover()))
        .on_press(on_press)
        .child(
            icon_svg(icon_body)
                .size(px(13.0))
                .flex_none()
                .text_color(move || theme().foreground),
        );
    if grow {
        btn = btn.flex_1().child(
            text(label)
                .text_size(px(tokens::TEXT_SMALL))
                .text_color(move || theme().foreground),
        );
        btn.into_element()
    } else {
        view()
            .flex_none()
            .tooltip(controls::tooltip(label))
            .child(btn)
            .into_element()
    }
}

fn popover_footer() -> impl IntoElement {
    view()
        .test_id("menubar-popover-footer")
        .w_full()
        .flex_none()
        .flex_row()
        .items_center()
        .gap(px(6.0))
        .px(px(10.0))
        .py(px(8.0))
        .child(footer_button(
            "Open Devbase",
            "popover-open",
            WINDOW_BODY,
            true,
            || dispatch(Action::OpenWindow),
        ))
        .child(footer_button(
            "Refresh",
            "popover-refresh",
            REFRESH_BODY,
            false,
            || dispatch(Action::Refresh),
        ))
        .child(footer_button(
            "Quit Devbase",
            "popover-quit",
            QUIT_BODY,
            false,
            || dispatch(Action::Quit),
        ))
}

// --- Right-click native NSMenu -----------------------------------------------

/// One entry in the right-click native `NSMenu`, modeled plainly so headless
/// tests can verify every macOS section header, icon, and submenu without
/// calling AppKit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MenuEntry {
    Separator,
    Item {
        label: String,
        icon: &'static str,
        enabled: bool,
        checked: Option<bool>,
        action: Option<Action>,
    },
    Submenu {
        label: String,
        icon: &'static str,
        items: Vec<MenuEntry>,
    },
}

fn row_menu_icon(row: &Row) -> &'static str {
    match row.kind {
        RowKind::Instance if row.key == "no-instances" => PLUS_BODY,
        RowKind::Instance | RowKind::Service if row.checked.is_some() => {
            if row.checked == Some(true) {
                STOP_BODY
            } else {
                PLAY_BODY
            }
        }
        RowKind::Instance => DATABASE_BODY,
        RowKind::Proxy => SHIELD_BODY,
        RowKind::Tunnel => CLOUD_BODY,
        RowKind::Service => match row.action {
            Some(Action::OpenService { source, .. }) => service_group_icon(source),
            _ => LAYERS_BODY,
        },
        RowKind::Action => match row.action {
            Some(Action::OpenWindow) => WINDOW_BODY,
            Some(Action::Refresh) => REFRESH_BODY,
            Some(Action::Quit) => QUIT_BODY,
            _ => PACKAGE_BODY,
        },
        RowKind::Header | RowKind::Status | RowKind::Separator => PACKAGE_BODY,
    }
}

fn menu_entry_for(row: &Row) -> MenuEntry {
    if row.kind == RowKind::Separator {
        return MenuEntry::Separator;
    }
    let label = if row.detail.is_empty() {
        row.label.clone()
    } else {
        format!("{} · {}", row.label, row.detail)
    };
    MenuEntry::Item {
        label,
        icon: row_menu_icon(row),
        enabled: row.enabled,
        checked: row.checked,
        action: row.enabled.then(|| row.action.clone()).flatten(),
    }
}

fn service_menu_group(panel: &Panel, source: Source) -> (usize, usize, Vec<MenuEntry>) {
    let mut total = 0usize;
    let mut running = 0usize;
    let mut items = Vec::new();
    let mut i = 0;
    while i < panel.rows.len() {
        let row = &panel.rows[i];
        let matches_source = row.kind == RowKind::Service
            && row.checked.is_none()
            && matches!(row.action, Some(Action::OpenService { source: s, .. }) if s == source);
        if matches_source {
            total += 1;
            let toggle = panel
                .rows
                .get(i + 1)
                .filter(|next| next.key == format!("{}:toggle", row.key));
            let is_running = toggle
                .and_then(|t| t.checked)
                .unwrap_or_else(|| running_status(&row.state));
            if is_running {
                running += 1;
            }
            items.push(menu_entry_for(row));
            if let Some(toggle_row) = toggle {
                items.push(menu_entry_for(toggle_row));
                i += 1;
            }
        }
        i += 1;
    }
    (running, total, items)
}

/// Builds the right-click `NSMenu` structure for macOS: explicit disabled
/// section headers between separators (or non-inline submenus for long service
/// lists) and monochrome template SVG icons on every item.
pub fn menu_entries(panel: &Panel) -> Vec<MenuEntry> {
    let mut entries = Vec::new();
    entries.push(MenuEntry::Item {
        label: format!("Devbase · {}", panel.summary),
        icon: PACKAGE_BODY,
        enabled: false,
        checked: None,
        action: None,
    });

    let db_items: Vec<MenuEntry> = panel
        .rows
        .iter()
        .filter(|row| row.kind == RowKind::Instance)
        .map(menu_entry_for)
        .collect();
    if !db_items.is_empty() {
        entries.push(MenuEntry::Separator);
        let db_detail = panel
            .rows
            .iter()
            .find(|row| row.key == "databases")
            .map(|row| row.detail.as_str())
            .unwrap_or("");
        let db_heading = if db_detail.is_empty() {
            "Databases".to_string()
        } else {
            format!("Databases · {db_detail}")
        };
        entries.push(MenuEntry::Item {
            label: db_heading,
            icon: DATABASE_BODY,
            enabled: false,
            checked: None,
            action: None,
        });
        entries.extend(db_items);
    }

    let https_items: Vec<MenuEntry> = panel
        .rows
        .iter()
        .filter(|row| matches!(row.kind, RowKind::Proxy | RowKind::Tunnel))
        .map(menu_entry_for)
        .collect();
    if !https_items.is_empty() {
        entries.push(MenuEntry::Separator);
        entries.push(MenuEntry::Item {
            label: "HTTPS".to_string(),
            icon: SHIELD_BODY,
            enabled: false,
            checked: None,
            action: None,
        });
        entries.extend(https_items);
    }

    for source in [Source::Brew, Source::UserAgent] {
        let (running, total, group_items) = service_menu_group(panel, source);
        if total == 0 {
            continue;
        }
        entries.push(MenuEntry::Separator);
        let heading = format!("{} · {running}/{total}", service_group_title(source));
        let group_icon = service_group_icon(source);
        if total > SERVICE_MENU_SUBMENU_THRESHOLD {
            entries.push(MenuEntry::Submenu {
                label: heading,
                icon: group_icon,
                items: group_items,
            });
        } else {
            entries.push(MenuEntry::Item {
                label: heading,
                icon: group_icon,
                enabled: false,
                checked: None,
                action: None,
            });
            entries.extend(group_items);
        }
    }

    entries.push(MenuEntry::Separator);
    for row in panel.rows.iter().filter(|row| row.kind == RowKind::Action) {
        entries.push(menu_entry_for(row));
    }
    entries
}

fn build_menu_item(entry: MenuEntry) -> MenuItem {
    match entry {
        MenuEntry::Separator => MenuItem::separator(),
        MenuEntry::Item {
            label,
            icon,
            enabled,
            checked,
            action,
        } => {
            let mut item = MenuItem::new(label).icon(icon_src(icon)).disabled(!enabled);
            if let Some(checked) = checked {
                item = item.checked(checked);
            }
            if enabled && let Some(action) = action {
                item = item.on_select(move || dispatch(action.clone()));
            }
            item
        }
        MenuEntry::Submenu { label, icon, items } => {
            MenuItem::submenu(label, items.into_iter().map(build_menu_item)).icon(icon_src(icon))
        }
    }
}

/// The right-click system menu of the tray item for macOS `NSMenu`.
fn menu(panel: &Panel) -> Vec<MenuItem> {
    menu_entries(panel)
        .into_iter()
        .map(build_menu_item)
        .collect()
}

/// Colored menu bar title runs beside the status item icon:
/// - Empty when nothing is running and nothing has failed.
/// - Running count in default menu-bar `labelColor` (`color: None`).
/// - Red failure indicator (` · <N>!`, or `<N>!` if nothing is running) when
///   `panel.failed > 0`.
pub fn title_runs(panel: &Panel) -> Vec<TrayTitleRun> {
    let mut runs = Vec::new();
    if panel.running > 0 {
        runs.push(TrayTitleRun::new(panel.running.to_string()));
    }
    if panel.failed > 0 {
        let label = if panel.running > 0 {
            format!(" · {}!", panel.failed)
        } else {
            format!("{}!", panel.failed)
        };
        runs.push(TrayTitleRun::new(label).color(rgb(TRAY_FAILED_COLOR_HEX)));
    }
    runs
}

fn tray_for(panel: Panel) -> nsh_core::element::desktop::Tray {
    let mut icon = tray()
        .icon(ImageSrc::svg_markup(MENUBAR_ICON))
        .tooltip(panel.tooltip.clone())
        .menu(menu(&panel))
        .on_click(toggle_popover);
    let runs = title_runs(&panel);
    if !runs.is_empty() {
        icon = icon.title(runs);
    }
    icon
}

/// Installs the status item and global toggle shortcut on a desktop. The
/// effect reads the stores, so the title, popover and right-click menu follow
/// them; off desktops `set_tray` does nothing and tests with
/// [`use_fake_menubar`] never register OS global shortcuts.
pub(crate) fn install() {
    if os().is_desktop() {
        load();
        if !FAKE_MODE.load(Ordering::Relaxed) {
            drop(register_global_shortcut(
                GLOBAL_SHORTCUT,
                toggle_popover_shortcut,
            ));
        }
    }
    effect(move || {
        let panel = panel();
        set_tray(tray_for(panel));
    });
}
