//! The Runtimes tab: Node.js, Java and Python (uv), what is installed, which
//! version is the default, the release list the Go "Refresh available
//! versions" action read, and the way into the runtime terminal.

use nsh_base::theme;
use nsh_core::prelude::*;

use crate::{
    components::{
        controls::{self as ui, TagTone, Tone, action_button, card, field_label, hairline, tag},
        icons,
    },
    store::development as dev,
    theme as tokens,
};

pub(super) fn runtimes() -> impl IntoElement {
    dev::load();
    dev::load_releases();
    dev::load_java_catalog();
    scroll().flex_1().min_h(px(0.0)).child(
        view()
            .flex_col()
            .gap(tokens::space(4.0))
            .px(tokens::space(5.0))
            .py(tokens::space(4.0))
            .child(node_card())
            .child(java_card())
            .child(python_card()),
    )
}

// --- Node.js -----------------------------------------------------------------

fn node_card() -> impl IntoElement {
    card("Node.js", dynamic(|| Some(node_body().into_element())))
}

fn node_body() -> impl IntoElement {
    view()
        .flex_col()
        .gap(tokens::space(3.0))
        .child(
            view()
                .flex_row()
                .items_center()
                .flex_wrap()
                .gap(tokens::space(2.0))
                .child(
                    text(node_summary)
                        .flex_1()
                        .min_w(px(160.0))
                        .text_size(px(tokens::TEXT_SMALL))
                        .tabular_nums()
                        .text_color(move || theme().foreground),
                )
                .child(action_button(
                    "Refresh available versions",
                    Tone::Default,
                    "dev-refresh-releases",
                    dev::releases_loading(),
                    dev::refresh_releases,
                ))
                .child(action_button(
                    "Open terminal",
                    Tone::Primary,
                    "dev-open-terminal",
                    false,
                    dev::open_terminal,
                )),
        )
        .child(installed_nodes())
        .child(releases_block())
}

fn node_summary() -> String {
    let version = dev::node_default_version();
    if version.is_empty() {
        "No default Node.js version".to_string()
    } else {
        format!("Default Node.js {version}")
    }
}

fn installed_nodes() -> impl IntoElement {
    let nodes = dev::node_versions();
    let empty = nodes.is_empty();
    let mut rows: Vec<AnyElement> = Vec::new();
    for node in nodes {
        if !rows.is_empty() {
            rows.push(hairline().into_element());
        }
        rows.push(node_row(node).into_element());
    }
    view()
        .test_id("node-installed")
        .flex_col()
        .child(field_label("Installed versions"))
        .child(when(empty, || {
            text("No Node.js versions installed. Install one in Software.")
                .text_size(px(tokens::TEXT_SMALL))
                .text_color(move || theme().muted_foreground)
        }))
        .child(rows)
}

fn node_row(node: dev::NodeVersion) -> impl IntoElement {
    let id = node.id.clone();
    let key = format!("node-{}", node.version);
    let default = node.default;
    let version = node.version.clone();
    let lts = node.lts.clone();
    let copy_id = id.clone();
    let default_id = id.clone();
    view()
        .test_id(&key)
        .flex_col()
        .gap(tokens::space(1.5))
        .py(tokens::space(2.0))
        .child(
            view()
                .flex_row()
                .items_center()
                .flex_wrap()
                .gap(tokens::space(1.5))
                .child(
                    text(format!("Node.js · {version}"))
                        .text_size(px(tokens::TEXT_BODY))
                        .font_weight(FontWeight::MEDIUM)
                        .tabular_nums()
                        .text_color(move || theme().foreground)
                        .truncate(),
                )
                .child(default.then(|| tag("Default", TagTone::Success)))
                .child((!lts.is_empty()).then(|| tag(format!("LTS {lts}"), TagTone::Neutral))),
        )
        .child(
            view()
                .flex_row()
                .items_center()
                .flex_wrap()
                .gap(tokens::space(1.5))
                .child(action_button(
                    "Set default",
                    Tone::Default,
                    format!("node-default-{copy_id}"),
                    default || dev::is_busy("node-default"),
                    move || dev::set_node_default(&default_id),
                ))
                .child(action_button(
                    "Copy executable path",
                    Tone::Default,
                    format!("node-copy-{id}"),
                    false,
                    move || dev::copy_node_binary(&copy_id),
                )),
        )
}

fn releases_block() -> impl IntoElement {
    let list = dev::releases();
    let loading = dev::releases_loading();
    let error = dev::releases_error();
    view()
        .flex_col()
        .gap(tokens::space(1.5))
        .child(field_label("Available releases"))
        .child(when(loading, || {
            text("Loading releases…")
                .text_size(px(tokens::TEXT_CAPTION))
                .text_color(move || theme().muted_foreground)
        }))
        .child(error.map(|message| {
            text(message)
                .text_size(px(tokens::TEXT_CAPTION))
                .text_color(move || theme().destructive)
                .selectable(true)
        }))
        .child(list.map(|list| {
            view()
                .flex_col()
                .gap(tokens::space(1.5))
                .child(when(list.stale, || {
                    text("nodejs.org could not be reached. Showing the last release list.")
                        .text_size(px(tokens::TEXT_CAPTION))
                        .text_color(tokens::warning)
                }))
                .child(
                    text(format!(
                        "{} tracked · install a version in Software",
                        list.releases.len()
                    ))
                    .text_size(px(tokens::TEXT_CAPTION))
                    .tabular_nums()
                    .text_color(move || theme().muted_foreground),
                )
                .child(
                    view()
                        .flex_row()
                        .items_center()
                        .flex_wrap()
                        .gap(tokens::space(1.0))
                        .child(list.releases.into_iter().take(24).map(|release| {
                            tag(
                                if release.lts.is_empty() {
                                    release.version
                                } else {
                                    format!("{} LTS {}", release.version, release.lts)
                                },
                                TagTone::Neutral,
                            )
                        })),
                )
        }))
}

// --- Java --------------------------------------------------------------------

fn java_card() -> impl IntoElement {
    card("Java", dynamic(|| Some(java_body().into_element())))
}

fn java_body() -> impl IntoElement {
    let java = dev::java_versions();
    let empty = java.is_empty();
    let mut rows: Vec<AnyElement> = Vec::new();
    for jdk in java {
        if !rows.is_empty() {
            rows.push(hairline().into_element());
        }
        rows.push(java_row(jdk).into_element());
    }
    let summary = {
        let version = dev::java_default_version();
        if version.is_empty() {
            "No default JDK".to_string()
        } else {
            format!("Default JDK {version}")
        }
    };
    view()
        .flex_col()
        .gap(tokens::space(3.0))
        .child(
            text(summary)
                .text_size(px(tokens::TEXT_SMALL))
                .tabular_nums()
                .text_color(move || theme().foreground),
        )
        .child(field_label("Installed JDKs"))
        .child(when(empty, || {
            text("No JDKs installed. Install one from the Software tab.")
                .text_size(px(tokens::TEXT_SMALL))
                .text_color(move || theme().muted_foreground)
        }))
        .child(rows)
        .child(
            text("Java follows .java-version and .sdkmanrc in each folder once terminal integration is enabled.")
                .text_size(px(tokens::TEXT_CAPTION))
                .text_color(move || theme().muted_foreground),
        )
        .child(hairline())
        .child(java_catalog_block())
}

fn java_catalog_block() -> impl IntoElement {
    let catalog = dev::java_catalog();
    let loading = dev::java_catalog_loading();
    let error = dev::java_catalog_error();
    let choice = dev::java_choice();
    let empty = catalog.is_empty();
    let choices: Vec<_> = catalog.into_iter().take(20).collect();
    view()
        .flex_col()
        .gap(tokens::space(1.5))
        .child(
            view()
                .flex_row()
                .items_center()
                .gap(tokens::space(1.5))
                .child(field_label("Install a JDK from JetBrains"))
                .child(action_button(
                    "Refresh available JDKs",
                    Tone::Default,
                    "java-refresh-catalog",
                    loading,
                    dev::refresh_java_catalog,
                )),
        )
        .child(when(loading, || {
            text("Loading the JetBrains catalog…")
                .text_size(px(tokens::TEXT_CAPTION))
                .text_color(move || theme().muted_foreground)
        }))
        .child(error.map(|message| {
            text(message)
                .text_size(px(tokens::TEXT_CAPTION))
                .text_color(move || theme().destructive)
                .selectable(true)
        }))
        .child(when(empty && !loading, || {
            text("No catalog versions were listed.")
                .text_size(px(tokens::TEXT_CAPTION))
                .text_color(move || theme().muted_foreground)
        }))
        .child(
            view()
                .flex_row()
                .items_center()
                .flex_wrap()
                .gap(tokens::space(1.0))
                .child(choices.into_iter().map(move |package| {
                    let id = package.id();
                    let select_id = id.clone();
                    let label = if package.java_product.is_empty() {
                        format!("Java {}", package.version)
                    } else {
                        format!("{} {}", package.java_product, package.version)
                    };
                    ui::chip(
                        label,
                        choice == id,
                        format!("java-catalog-{id}"),
                        false,
                        move || dev::select_java(&select_id),
                    )
                })),
        )
        .child(
            view()
                .flex_row()
                .items_center()
                .flex_wrap()
                .gap(tokens::space(1.5))
                .child(action_button(
                    "Install JDK",
                    Tone::Primary,
                    "java-install",
                    empty || loading || dev::is_busy("java-install"),
                    dev::install_java,
                ))
                .child(
                    text("Downloads from JetBrains and installs a Devbase-managed JDK.")
                        .text_size(px(tokens::TEXT_CAPTION))
                        .text_color(move || theme().muted_foreground),
                ),
        )
}

fn java_row(jdk: dev::JavaVersion) -> impl IntoElement {
    let id = jdk.id.clone();
    let default = jdk.default;
    let version = jdk.version.clone();
    let product = jdk.product.clone();
    let default_id = id.clone();
    view()
        .test_id(&format!("java-{id}"))
        .flex_col()
        .gap(tokens::space(1.5))
        .py(tokens::space(2.0))
        .child(
            view()
                .flex_row()
                .items_center()
                .flex_wrap()
                .gap(tokens::space(1.5))
                .child(
                    text(if product.is_empty() {
                        format!("Java · {version}")
                    } else {
                        format!("{product} · {version}")
                    })
                    .text_size(px(tokens::TEXT_BODY))
                    .font_weight(FontWeight::MEDIUM)
                    .tabular_nums()
                    .text_color(move || theme().foreground)
                    .truncate(),
                )
                .child(default.then(|| tag("Default", TagTone::Success))),
        )
        .child(action_button(
            "Set default",
            Tone::Default,
            format!("java-default-{id}"),
            default || dev::is_busy("java-default"),
            move || dev::set_java_default(&default_id),
        ))
}

// --- Python (uv) -------------------------------------------------------------

fn python_card() -> impl IntoElement {
    card(
        "Python (uv)",
        view()
            .flex_col()
            .gap(tokens::space(3.0))
            .child(
                text(move || {
                    let description = dev::uv_description();
                    if description.is_empty() {
                        "uv was not found".to_string()
                    } else {
                        description
                    }
                })
                .text_size(px(tokens::TEXT_SMALL))
                .text_color(move || theme().muted_foreground)
                .selectable(true),
            )
            .child(dynamic(move || {
                let error = dev::uv_error();
                (!error.is_empty()).then(|| {
                    text(error)
                        .text_size(px(tokens::TEXT_CAPTION))
                        .text_color(move || theme().destructive)
                        .selectable(true)
                })
            }))
            .child(uv_input())
            .child(dynamic(|| Some(python_body().into_element()))),
    )
}

fn uv_input() -> impl IntoElement {
    view()
        .flex_col()
        .gap(tokens::space(1.0))
        .child(field_label("uv executable"))
        .child(ui::text_field(
            "uv-path-input",
            "/absolute/path/to/uv (optional)",
            dev::uv_path,
            dev::set_uv_path_draft,
        ))
        .child(
            view()
                .flex_row()
                .items_center()
                .gap(tokens::space(1.5))
                .child(action_button(
                    "Use uv executable",
                    Tone::Default,
                    "uv-use-path",
                    false,
                    dev::use_uv_path,
                ))
                .child(action_button(
                    "Refresh Python versions",
                    Tone::Default,
                    "uv-refresh",
                    false,
                    dev::refresh,
                )),
        )
}

fn python_body() -> impl IntoElement {
    let installed = dev::python_versions();
    let available = dev::python_available();
    let pin = dev::python_pin();
    let empty = installed.is_empty();
    let mut rows: Vec<AnyElement> = Vec::new();
    for python in installed {
        if !rows.is_empty() {
            rows.push(hairline().into_element());
        }
        rows.push(python_row(python).into_element());
    }
    let choices: Vec<dev::PythonVersion> = available.into_iter().take(20).collect();
    let no_choices = choices.is_empty();

    view()
        .flex_col()
        .gap(tokens::space(3.0))
        .child(field_label("Installed by uv"))
        .child(when(empty, || {
            text("None installed")
                .text_size(px(tokens::TEXT_SMALL))
                .text_color(move || theme().muted_foreground)
        }))
        .child(rows)
        .child(hairline())
        .child(field_label("Install a version"))
        .child(when(no_choices, || {
            text("No downloadable macOS arm64 versions were listed.")
                .text_size(px(tokens::TEXT_CAPTION))
                .text_color(move || theme().muted_foreground)
        }))
        .child(
            view()
                .flex_row()
                .items_center()
                .flex_wrap()
                .gap(tokens::space(1.0))
                .child(choices.into_iter().map(|python| {
                    let choice = python.key.clone();
                    ui::chip(
                        python.key.clone(),
                        dev::python_choice() == python.key,
                        format!("python-available-{}", python.key),
                        false,
                        move || dev::select_python(&choice),
                    )
                })),
        )
        .child(
            view()
                .flex_row()
                .items_center()
                .gap(tokens::space(1.5))
                .child(action_button(
                    "Install Python with uv",
                    Tone::Primary,
                    "python-install",
                    no_choices || dev::is_busy("python-install"),
                    dev::install_python,
                ))
                .child(when(!pin.is_empty(), move || {
                    text(format!("uv global pin: {pin}"))
                        .text_size(px(tokens::TEXT_CAPTION))
                        .tabular_nums()
                        .text_color(move || theme().muted_foreground)
                })),
        )
}

fn python_row(python: dev::PythonVersion) -> impl IntoElement {
    let key = python.key.clone();
    let pin_id = key.clone();
    let uninstall_id = key.clone();
    let copy_key = key.clone();
    let path = python.path.clone();
    let has_path = !path.is_empty();
    let copy_path = path.clone();
    view()
        .test_id(&format!("python-{}", python.key))
        .flex_col()
        .gap(tokens::space(1.5))
        .py(tokens::space(2.0))
        .child(
            view()
                .flex_row()
                .items_center()
                .flex_wrap()
                .gap(tokens::space(1.5))
                .child(
                    text(format!("Python · {}", python.version))
                        .text_size(px(tokens::TEXT_BODY))
                        .font_weight(FontWeight::MEDIUM)
                        .tabular_nums()
                        .text_color(move || theme().foreground)
                        .truncate(),
                )
                .child(python.pin.then(|| tag("Default", TagTone::Success))),
        )
        .child(when(has_path, move || {
            text(path.clone())
                .font_family("monospace")
                .text_size(px(tokens::TEXT_CAPTION))
                .text_color(move || theme().muted_foreground)
                .selectable(true)
                .truncate()
        }))
        .child(
            view()
                .flex_row()
                .items_center()
                .flex_wrap()
                .gap(tokens::space(1.5))
                .child(action_button(
                    "Set default",
                    Tone::Default,
                    format!("python-pin-{pin_id}"),
                    python.pin || dev::is_busy("python-default"),
                    move || dev::set_python_default(&pin_id),
                ))
                .child(action_button(
                    "Uninstall",
                    Tone::Danger,
                    format!("python-uninstall-{uninstall_id}"),
                    dev::is_busy("python-uninstall"),
                    move || dev::uninstall_python(&uninstall_id),
                ))
                .child(when(has_path, move || {
                    let path = copy_path.clone();
                    ui::row_icon_button(
                        icons::copy(),
                        "Copy Python executable path",
                        format!("python-copy-{copy_key}"),
                        false,
                        move || dev::copy_python_path(&path),
                    )
                })),
        )
}
