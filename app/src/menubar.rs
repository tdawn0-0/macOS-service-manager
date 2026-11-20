//! The menu bar panel, ported from `menubar.go`.
//!
//! The Go app opened a flyout window under its status item. NSH's tray API
//! opens the system menu instead (`nsh/desktop`'s [`Tray::menu`]), so the
//! panel is a [`MenuItem`] tree: the same glance at the workspace, built from
//! the stores every time one of them changes.
//!
//! The rows are a plain model ([`Panel`]) built by [`panel`]. Two things read
//! it: [`tray_for`] turns it into system menu items whose handlers call
//! [`dispatch`], and tests read it to pin the labels and switches without a
//! platform. Nothing in a menu handler blocks: every switch calls the store,
//! which runs its provider call on a blocking thread and writes its signals.
//!
//! The Go panel also listed the Homebrew and login-agent sources under folding
//! headings. Those stores have no panel rows yet; this port shows the
//! workspace's own resources (database instances, the HTTPS proxy and its
//! tunnel), which is what the tray acts on. System agents and daemons stay
//! out, as in Go: their changes affect the whole machine.

use nsh_core::{
    desktop::TrayTitleRun,
    element::{
        ImageSrc,
        desktop::{MenuItem, main_window, set_tray, tray},
    },
    prelude::*,
};

use crate::store::{self, Section, databases, proxy, software::SoftwareKind, system};

/// The icon `assets/icons/menubar.svg` draws, inline so a cargo run and a
/// bundled app show the same template.
const MENUBAR_ICON: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="36" height="36" viewBox="0 0 36 36">
  <path fill-rule="evenodd" d="M9.5 3h17a6.5 6.5 0 0 1 0 13h-17a6.5 6.5 0 0 1 0-13Zm17 2.9a3.6 3.6 0 1 0 0 7.2 3.6 3.6 0 0 0 0-7.2Z"/>
  <rect x="4.2" y="21.2" width="27.6" height="10.6" rx="5.3" fill="none" stroke="#000" stroke-width="2.4"/>
  <circle cx="9.5" cy="26.5" r="2.7"/>
</svg>"##;

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

/// Everything the panel reads from the stores, in one value, so the labels
/// can be built without a platform or a render.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PanelInput {
    pub instances: Vec<InstanceRow>,
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
    let rules = proxy::saved_rules();
    let proxy_status = proxy::proxy_status();
    let tunnel_status = proxy::tunnel_status();
    let helper = system::helper_status();
    Panel::from(PanelInput {
        instances,
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

/// Runs a row's action. Menu handlers call this; every call returns at once.
pub fn dispatch(action: Action) {
    match action {
        Action::OpenInstance(id) => {
            store::select_section(Section::Databases);
            databases::open_instance(&id);
            show_window();
        }
        Action::OpenNewInstance => {
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
        Action::OpenWindow => show_window(),
        Action::Refresh => refresh(),
        Action::Quit => system::request_quit(),
    }
}

/// Reads the workspace, proxy and settings again. Go's panel refreshed the
/// development state and the sources it listed.
pub fn refresh() {
    databases::refresh();
    proxy::refresh();
    system::refresh_settings();
}

/// Reads what the tray shows the first time the app starts.
pub fn load() {
    databases::refresh();
    proxy::refresh();
    system::load_settings();
}

fn menu_item(row: &Row) -> MenuItem {
    if row.kind == RowKind::Separator {
        return MenuItem::separator();
    }
    let label = if row.detail.is_empty() {
        row.label.clone()
    } else {
        format!("{} · {}", row.label, row.detail)
    };
    let mut item = MenuItem::new(label);
    if let Some(checked) = row.checked {
        item = item.checked(checked);
    }
    item = item.disabled(!row.enabled);
    if row.enabled
        && let Some(action) = row.action.clone()
    {
        item = item.on_select(move || dispatch(action.clone()));
    }
    item
}

/// The system menu the tray opens.
fn menu(panel: &Panel) -> Vec<MenuItem> {
    panel.rows.iter().map(menu_item).collect()
}

fn tray_for(panel: Panel) -> nsh_core::element::desktop::Tray {
    let mut icon = tray()
        .icon(ImageSrc::svg_markup(MENUBAR_ICON))
        .tooltip(panel.tooltip.clone())
        .menu(menu(&panel));
    if !panel.title.is_empty() {
        icon = icon.title([TrayTitleRun::new(panel.title.clone())]);
    }
    icon
}

/// Installs the status item on a desktop. The effect reads the stores, so
/// the title and every row follow them; off desktops `set_tray` does nothing.
pub(crate) fn install() {
    if os().is_desktop() {
        load();
    }
    effect(move || {
        let panel = panel();
        set_tray(tray_for(panel));
    });
}
