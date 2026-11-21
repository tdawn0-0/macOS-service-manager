//! Native `Cmd+K` Command Palette backed by `nsh_base::command()`.
//!
//! Opens as a centered modal overlay (`test_id("command-palette")`), filters
//! navigation, workspace actions, and loaded database instances natively as the
//! user types, and executes the selected command on Return or click.

use devcore::service::Source;
use nsh_base::{CommandEntry, CommandGroup, CommandItem, command, dialog, theme};
use nsh_core::prelude::*;

use crate::{
    components::icons,
    dev_logs, editor, guide, shell,
    store::{self, Section, databases, proxy, software::SoftwareKind, system},
    theme as tokens,
};

#[derive(Debug, Clone, Default)]
struct PaletteState {
    open: bool,
}

fn state() -> Signal<PaletteState> {
    global::<PaletteState>()
}

/// Whether the command palette overlay is currently open.
pub fn is_open() -> bool {
    state().with(|s| s.open)
}

/// Opens the command palette.
pub fn open() {
    state().update(|s| s.open = true);
}

/// Closes the command palette.
pub fn close() {
    state().update(|s| s.open = false);
}

/// Toggles the command palette open or closed.
pub fn toggle() {
    state().update(|s| s.open = !s.open);
}

fn tunnel_configured(rules: &[devcore::model::ProxyRule]) -> bool {
    proxy::token_configured()
        || proxy::worker_count() > 0
        || proxy::tunnel_on()
        || rules.iter().any(|rule| rule.tunnel)
}

fn instance_title(kind: &str) -> String {
    SoftwareKind::from_key(kind)
        .map(|k| k.title().to_string())
        .unwrap_or_else(|| kind.to_string())
}

/// Builds the reactive list of commands shown in the palette.
pub(crate) fn build_items() -> Vec<CommandItem> {
    let active = store::active_section();
    let logs_open = dev_logs::is_open();
    let proxy_on = proxy::proxy_on();
    let tunnel_on = proxy::tunnel_on();
    let proxy_busy = proxy::busy();
    let saved_rules = proxy::saved_rules();
    let has_tunnel = tunnel_configured(&saved_rules);
    let has_tunnel_rules = saved_rules.iter().any(|r| r.tunnel);
    let db_instances = databases::instances();
    let has_running_databases = db_instances
        .iter()
        .any(|inst| matches!(databases::status(&inst.id).as_str(), "Running" | "Healthy"));
    let db_busy = databases::operations_busy();

    let mut items = vec![
        CommandItem::new("nav:development", "Go to Development")
            .group("Navigate")
            .shortcut("mod+1")
            .keywords([
                "development",
                "software",
                "runtimes",
                "node",
                "python",
                "java",
                "shell",
            ])
            .checked(active == Section::Development),
        CommandItem::new("nav:databases", "Go to Databases")
            .group("Navigate")
            .shortcut("mod+2")
            .keywords([
                "databases",
                "postgres",
                "postgresql",
                "mysql",
                "mariadb",
                "redis",
                "mongodb",
            ])
            .checked(active == Section::Databases),
        CommandItem::new("nav:https-proxy", "Go to HTTPS Proxy")
            .group("Navigate")
            .shortcut("mod+3")
            .keywords([
                "https",
                "proxy",
                "tls",
                "certificate",
                "ca",
                "tunnel",
                "cloudflare",
            ])
            .checked(active == Section::HttpsProxy),
        CommandItem::new("nav:services", "Go to Services")
            .group("Navigate")
            .shortcut("mod+4")
            .keywords([
                "services", "launchd", "homebrew", "agents", "daemons", "login",
            ])
            .checked(active == Section::Services),
        CommandItem::new("nav:diagnostics", "Go to Diagnostics")
            .group("Navigate")
            .shortcut("mod+5")
            .keywords(["diagnostics", "ports", "checks", "health", "operations"])
            .checked(active == Section::Diagnostics),
        CommandItem::new("nav:settings", "Go to Settings")
            .group("Navigate")
            .shortcut("mod+6")
            .keywords(["settings", "preferences", "helper", "cli", "cache", "login"])
            .checked(active == Section::Settings),
        CommandItem::new(
            "action:toggle-logs",
            if logs_open {
                "Hide Live Log Drawer"
            } else {
                "Show Live Log Drawer"
            },
        )
        .group("Actions")
        .shortcut("mod+l")
        .keywords(["logs", "drawer", "tail", "stderr", "stdout", "console"]),
        CommandItem::new("action:new-job", "New LaunchAgent…")
            .group("Actions")
            .shortcut("mod+n")
            .keywords([
                "new",
                "create",
                "job",
                "plist",
                "launchagent",
                "service",
                "editor",
            ]),
        CommandItem::new("action:new-database", "New Database Instance…")
            .group("Actions")
            .shortcut("mod+shift+n")
            .keywords([
                "new", "create", "database", "instance", "postgres", "redis", "mysql",
            ]),
        CommandItem::new("action:stop-all-databases", "Stop All Databases")
            .group("Actions")
            .keywords([
                "stop",
                "all",
                "databases",
                "shutdown",
                "postgres",
                "mysql",
                "redis",
                "mongodb",
            ])
            .disabled(db_busy || !has_running_databases),
        CommandItem::new("action:refresh", "Refresh Active Section")
            .group("Actions")
            .shortcut("mod+r")
            .keywords(["refresh", "reload", "sync", "update"]),
        CommandItem::new(
            "action:toggle-proxy",
            if proxy_on {
                "Stop HTTPS Proxy"
            } else {
                "Start HTTPS Proxy"
            },
        )
        .group("Actions")
        .keywords(["https", "proxy", "toggle", "start", "stop", "reverse"])
        .disabled(proxy_busy || (!proxy_on && saved_rules.is_empty())),
    ];

    if has_tunnel {
        items.push(
            CommandItem::new(
                "action:toggle-tunnel",
                if tunnel_on {
                    "Stop Cloudflare Tunnel"
                } else {
                    "Start Cloudflare Tunnel"
                },
            )
            .group("Actions")
            .keywords(["cloudflare", "tunnel", "workers", "toggle", "start", "stop"])
            .disabled(proxy_busy || (!tunnel_on && !has_tunnel_rules)),
        );
    }

    items.push(
        CommandItem::new("action:run-diagnostics", "Run Diagnostics")
            .group("Actions")
            .keywords(["diagnostics", "diagnose", "check", "health", "report"]),
    );
    items.push(
        CommandItem::new("action:open-guide", "Open Full Disk Access Guide")
            .group("Actions")
            .keywords([
                "full disk access",
                "fda",
                "privacy",
                "permission",
                "welcome",
                "guide",
                "help",
            ]),
    );

    for instance in db_instances {
        let title = instance_title(&instance.kind);
        let status = databases::status(&instance.id);
        let label = format!("{} ({} · :{})", instance.id, title, instance.port);
        items.push(
            CommandItem::new(format!("db:{}", instance.id), label)
                .group("Databases")
                .keywords([
                    instance.id.clone(),
                    instance.kind.clone(),
                    title,
                    instance.port.to_string(),
                    status,
                ]),
        );
    }

    items
}

/// Runs the command identified by `id`.
pub(crate) fn execute(id: &str) {
    match id {
        "nav:services" => store::select_section(Section::Services),
        "nav:development" => store::select_section(Section::Development),
        "nav:databases" => store::select_section(Section::Databases),
        "nav:https-proxy" => store::select_section(Section::HttpsProxy),
        "nav:diagnostics" => store::select_section(Section::Diagnostics),
        "nav:settings" => store::select_section(Section::Settings),
        "action:toggle-logs" => dev_logs::toggle(),
        "action:new-job" => editor::open_new(Source::UserAgent),
        "action:new-database" => {
            store::select_section(Section::Databases);
            databases::open_new();
        }
        "action:stop-all-databases" => databases::stop_all_running(),
        "action:refresh" => shell::refresh_active_section(),
        "action:toggle-proxy" => proxy::set_proxy_enabled(!proxy::proxy_on()),
        "action:toggle-tunnel" => proxy::set_tunnel_enabled(!proxy::tunnel_on()),
        "action:run-diagnostics" => {
            store::select_section(Section::Diagnostics);
            system::run_diagnostics();
        }
        "action:open-guide" => guide::open(),
        other => {
            if let Some(instance_id) = other.strip_prefix("db:") {
                store::select_section(Section::Databases);
                databases::open_instance(instance_id);
            }
        }
    }
}

fn render_group_header(group: &CommandGroup) -> AnyElement {
    text(group.heading.clone())
        .text_size(px(tokens::TEXT_CAPTION))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(move || theme().muted_foreground)
        .into_element()
}

fn render_command_entry(entry: &CommandEntry) -> AnyElement {
    let label = entry.label.clone();
    let shortcut = entry.shortcut_label.clone();
    let checked = entry.checked;
    view()
        .test_id(&format!("command-item-{}", entry.id))
        .w_full()
        .flex_row()
        .items_center()
        .justify_between()
        .gap(tokens::space(2.5))
        .child(
            view()
                .flex_1()
                .min_w(px(0.0))
                .flex_row()
                .items_center()
                .gap(tokens::space(2.0))
                .child(
                    text(label)
                        .flex_1()
                        .min_w(px(0.0))
                        .text_size(px(tokens::TEXT_BODY))
                        .text_color(move || theme().foreground)
                        .truncate(),
                )
                .child(when(checked, || {
                    view()
                        .w(px(6.0))
                        .h(px(6.0))
                        .flex_none()
                        .rounded_full()
                        .bg(move || theme().primary)
                })),
        )
        .child(when(shortcut.is_some(), move || {
            let hint = shortcut.clone().unwrap_or_default();
            view()
                .flex_none()
                .px(px(5.0))
                .py(px(1.0))
                .rounded(px(4.0))
                .bg(tokens::inset)
                .border_1()
                .border_color(tokens::hairline)
                .child(
                    text(hint)
                        .text_size(px(10.0))
                        .tabular_nums()
                        .text_color(move || theme().muted_foreground),
                )
        }))
        .into_element()
}

fn palette_card() -> impl IntoElement {
    view()
        .test_id("command-palette")
        .w(px(560.0))
        .flex_col()
        .rounded(tokens::panel_radius)
        .bg(tokens::panel)
        .border_1()
        .border_color(tokens::hairline)
        .shadow_xl()
        .overflow_hidden()
        .child(
            command()
                .items(build_items)
                .placeholder("Type a command or search…")
                .auto_focus(true)
                .separator(true)
                .accessibility_label("Command Palette")
                .search_accessibility_label("Search commands")
                .search_prefix(
                    icons::search()
                        .size(px(14.0))
                        .flex_none()
                        .text_color(move || theme().muted_foreground),
                )
                .search_suffix(
                    view()
                        .flex_none()
                        .px(px(5.0))
                        .py(px(1.5))
                        .rounded(px(4.0))
                        .bg(tokens::inset)
                        .border_1()
                        .border_color(tokens::hairline)
                        .child(
                            text("ESC")
                                .text_size(px(10.0))
                                .text_color(move || theme().muted_foreground),
                        ),
                )
                .search_bar_style(|s| {
                    s.w_full()
                        .flex_row()
                        .items_center()
                        .gap(tokens::space(2.5))
                        .px(tokens::space(3.5))
                        .py(tokens::space(2.5))
                        .border_b_1()
                        .border_color(tokens::hairline)
                })
                .search_style(|s| {
                    s.flex_1()
                        .min_w(px(0.0))
                        .h(px(24.0))
                        .text_size(px(14.0))
                        .text_color(move || theme().foreground)
                })
                .list_style(|s| s.w_full().max_h(px(360.0)).p(tokens::space(1.5)))
                .group_style(|s| {
                    s.w_full()
                        .px(tokens::space(2.5))
                        .pt(tokens::space(2.0))
                        .pb(tokens::space(1.0))
                })
                .separator_style(|s| {
                    s.w_full()
                        .h(px(1.0))
                        .my(tokens::space(1.0))
                        .bg(tokens::hairline)
                })
                .item_class("focus:bg-sidebar-accent hover:bg-accent disabled:opacity-45")
                .item_style(|s| {
                    s.w_full()
                        .px(tokens::space(2.5))
                        .py(tokens::space(2.0))
                        .rounded(move || theme().radius)
                })
                .render_group(render_group_header)
                .render_item(render_command_entry)
                .empty(
                    view()
                        .test_id("command-palette-empty")
                        .w_full()
                        .py(px(28.0))
                        .items_center()
                        .justify_center()
                        .child(
                            text("No matching commands.")
                                .text_size(px(tokens::TEXT_SMALL))
                                .text_color(move || theme().muted_foreground),
                        ),
                )
                .footer(
                    view()
                        .w_full()
                        .flex_row()
                        .items_center()
                        .justify_between()
                        .px(tokens::space(3.5))
                        .py(tokens::space(2.0))
                        .border_t_1()
                        .border_color(tokens::hairline)
                        .bg(tokens::inset)
                        .child(
                            text("↑↓ Navigate · ↵ Run · Esc Close")
                                .text_size(px(tokens::TEXT_CAPTION))
                                .text_color(move || theme().muted_foreground),
                        )
                        .child(
                            text("⌘K")
                                .text_size(px(tokens::TEXT_CAPTION))
                                .tabular_nums()
                                .text_color(move || theme().muted_foreground),
                        ),
                )
                .on_select(|id: &SharedString| {
                    close();
                    execute(id.as_str());
                })
                .on_cancel(close),
        )
}

/// Modal dialog overlay hosting the Command Palette while [`is_open`].
pub fn overlay() -> impl IntoElement {
    dialog().label("Command Palette").on_cancel(close).child(
        view()
            .size_full()
            .relative()
            .flex_col()
            .items_center()
            .justify_start()
            .pt(px(84.0))
            .bg(AdaptiveColor::rgb(0x000000, 0x000000).opacity(0.35))
            .child(
                view()
                    .test_id("command-palette-backdrop")
                    .absolute()
                    .top(px(0.0))
                    .right(px(0.0))
                    .bottom(px(0.0))
                    .left(px(0.0))
                    .on_press(|_: &()| close()),
            )
            .child(palette_card()),
    )
}
