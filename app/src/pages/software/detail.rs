//! The software detail pane: the versions installed of the software chosen,
//! their file checks, the catalog's versions with a review before installing,
//! and the download cache.

use devcore::software::details::PackageDetails;
use nsh_base::theme;
use nsh_core::prelude::*;

use crate::{
    components::{
        controls::{self as ui, TagTone, Tone, action_button, card, hairline, tag},
        icons, note,
    },
    store::software::{self as sw, SoftwareKind},
    theme as tokens,
};

pub(super) fn detail_pane() -> impl IntoElement {
    view()
        .test_id("software-detail")
        .flex_1()
        .min_w(px(0.0))
        .flex_col()
        .child(header())
        .child(notes())
        .child(
            scroll().flex_1().min_h(px(0.0)).child(
                view()
                    .flex_col()
                    .gap(tokens::space(4.0))
                    .px(tokens::space(5.0))
                    .pt(tokens::space(4.0))
                    .pb(tokens::space(5.0))
                    .child(installed_card())
                    .child(install_card())
                    .child(cache_card()),
            ),
        )
}

/// The software's name, what is installed, and a fresh read.
fn header() -> impl IntoElement {
    ui::page_header(
        move || sw::active_software().title().to_string(),
        Some(move || meta(sw::active_software())),
        view()
            .flex_none()
            .flex_row()
            .items_center()
            .gap(tokens::space(2.0))
            .child(when(sw::loading, || {
                text("Loading")
                    .text_size(px(tokens::TEXT_CAPTION))
                    .text_color(move || theme().muted_foreground)
            }))
            .child(ui::toolbar_icon_button(
                icons::refresh(),
                "Refresh software",
                "software-refresh",
                sw::refresh,
            )),
    )
}

fn meta(kind: SoftwareKind) -> String {
    let count = sw::installed_count(kind);
    let mut text = if count == 0 {
        "Not installed".to_string()
    } else {
        format!("{} installed", sw::plural(count, "version"))
    };
    if let Some(default) = sw::default_version(kind) {
        text.push_str(&format!(" · default {default}"));
    }
    text
}

/// The read error and an operation's error.
fn notes() -> impl IntoElement {
    view()
        .flex_col()
        .flex_none()
        .child(dynamic(move || {
            sw::load_error().map(|message| note(icons::warn(), message, true))
        }))
        .child(dynamic(move || {
            sw::action_error().map(|message| note(icons::warn(), message, true))
        }))
}

// --- Installed ---------------------------------------------------------------

fn installed_card() -> impl IntoElement {
    card(
        "Installed",
        dynamic(|| Some(installed_body().into_element())),
    )
}

fn installed_body() -> impl IntoElement {
    let packages = sw::installed(sw::active_software());
    let empty = packages.is_empty();
    let mut rows: Vec<AnyElement> = Vec::new();
    for (index, package) in packages.into_iter().enumerate() {
        if !rows.is_empty() {
            rows.push(hairline().into_element());
        }
        rows.push(installed_row(package, index).into_element());
    }
    view()
        .flex_col()
        .child(when(empty, || {
            text("No versions installed.")
                .text_size(px(tokens::TEXT_SMALL))
                .text_color(move || theme().muted_foreground)
        }))
        .child(rows)
}

fn installed_row(package: devcore::model::Package, index: usize) -> impl IntoElement {
    let id = package.id();
    let key = format!("{}-{}", package.name, package.version);
    let title = sw::title_of(&package.name);
    let version = package.version.clone();
    let license = package.license.clone();
    let formula = package.formula.clone();
    let homebrew = !formula.is_empty();
    let is_database = SoftwareKind::from_key(&package.name).is_some_and(SoftwareKind::is_database);
    let create_instance_id = if index == 0 {
        "software-create-instance".to_string()
    } else {
        format!("software-create-instance-{key}")
    };
    let default = sw::is_default(&package);
    let unused = sw::is_unused(&id);
    let (status, checked) = sw::integrity(&id);
    let needs_repair = status == "Needs repair";
    let busy = sw::is_busy(&id);
    let disabled = busy || sw::operation_busy();
    let status_word = status.to_lowercase();
    let status_icon = status.clone();
    let status_tint = status.clone();
    let check_id = id.clone();
    let repair_id = id.clone();
    let uninstall_id = id.clone();
    let checked_summary = checked
        .as_ref()
        .map(|result| (result.files, result.issues.clone()));

    view()
        .test_id(&format!("installed-{key}"))
        .flex_col()
        .gap(tokens::space(2.0))
        .py(tokens::space(2.0))
        .child(
            view()
                .flex_row()
                .items_center()
                .flex_wrap()
                .gap(tokens::space(1.5))
                .child(
                    text(format!("{title} · {version}"))
                        .text_size(px(tokens::TEXT_BODY))
                        .font_weight(FontWeight::MEDIUM)
                        .tabular_nums()
                        .text_color(move || theme().foreground)
                        .truncate(),
                )
                .child(default.then(|| tag("Default", TagTone::Success)))
                .child(homebrew.then(|| tag("Homebrew", TagTone::Neutral)))
                .child(unused.then(|| tag("Unused", TagTone::Neutral))),
        )
        .child(
            view()
                .flex_row()
                .items_center()
                .gap(tokens::space(1.5))
                .min_w(px(0.0))
                .child(
                    icons::check()
                        .size(px(12.0))
                        .flex_none()
                        .text_color(move || integrity_color(&status_icon)),
                )
                .child(
                    text(format!("Files {status_word}"))
                        .text_size(px(tokens::TEXT_SMALL))
                        .text_color(move || integrity_color(&status_tint))
                        .flex_none(),
                )
                .child(when(!license.is_empty(), move || {
                    view()
                        .flex_row()
                        .items_center()
                        .gap(tokens::space(1.0))
                        .min_w(px(0.0))
                        .child(
                            text("·")
                                .text_size(px(tokens::TEXT_SMALL))
                                .text_color(move || theme().muted_foreground),
                        )
                        .child(
                            text(license.clone())
                                .text_size(px(tokens::TEXT_CAPTION))
                                .text_color(move || theme().muted_foreground)
                                .truncate(),
                        )
                })),
        )
        .child(checked_summary.map(|(files, issues)| {
            view()
                .flex_col()
                .gap(tokens::space(0.5))
                .child(
                    text(format!("{files} files checked"))
                        .text_size(px(tokens::TEXT_CAPTION))
                        .tabular_nums()
                        .text_color(move || theme().muted_foreground),
                )
                .child(issues.into_iter().map(|issue| {
                    text(issue)
                        .text_size(px(tokens::TEXT_CAPTION))
                        .text_color(move || theme().destructive)
                        .selectable(true)
                }))
        }))
        .child(
            view()
                .flex_row()
                .items_center()
                .flex_wrap()
                .gap(tokens::space(1.5))
                .child(is_database.then(|| {
                    action_button(
                        "Create database instance",
                        Tone::Primary,
                        create_instance_id,
                        busy,
                        || {
                            crate::store::select_section(crate::store::Section::Databases);
                            crate::store::databases::open_new();
                        },
                    )
                }))
                .child(action_button(
                    "Check files",
                    Tone::Default,
                    format!("check-{key}"),
                    disabled,
                    move || sw::check_files(&check_id),
                ))
                .child((!homebrew).then(|| {
                    action_button(
                        "Repair",
                        if needs_repair {
                            Tone::Primary
                        } else {
                            Tone::Default
                        },
                        format!("repair-{key}"),
                        disabled,
                        move || sw::repair(&repair_id),
                    )
                }))
                .child(view().flex_1())
                .child(action_button(
                    if homebrew {
                        "Remove from workspace"
                    } else {
                        "Uninstall"
                    },
                    Tone::Danger,
                    format!("uninstall-{key}"),
                    disabled,
                    move || sw::request_uninstall(&uninstall_id),
                )),
        )
        .child(dynamic({
            let id = id.clone();
            move || {
                sw::progress(&id).map(|line| {
                    text(line)
                        .text_size(px(tokens::TEXT_CAPTION))
                        .text_color(tokens::success)
                        .selectable(true)
                })
            }
        }))
        .child(when(homebrew, move || {
            let command = format!("brew reinstall {formula}");
            let copy_command = command.clone();
            view()
                .flex_row()
                .items_center()
                .gap(tokens::space(1.5))
                .child(
                    text(command)
                        .font_family("monospace")
                        .text_size(px(tokens::TEXT_CAPTION))
                        .text_color(move || theme().muted_foreground)
                        .selectable(true),
                )
                .child(ui::row_icon_button(
                    icons::copy(),
                    "Copy Homebrew reinstall command",
                    format!("copy-brew-{key}"),
                    false,
                    move || sw::copy(&copy_command),
                ))
        }))
}

fn integrity_color(status: &str) -> AdaptiveColor {
    match status {
        "Verified" => tokens::success(),
        "Needs repair" => theme().destructive,
        _ => theme().muted_foreground,
    }
}

// --- Install and review ------------------------------------------------------

fn install_card() -> impl IntoElement {
    card(
        "Install a version",
        view()
            .flex_col()
            .gap(tokens::space(3.0))
            .child(dynamic(|| Some(catalog_versions().into_element())))
            .child(version_field())
            .child(dynamic(|| Some(install_actions().into_element())))
            .child(dynamic(|| sw::preview().map(preview_inner))),
    )
}

fn install_actions() -> impl IntoElement {
    let preview = sw::preview();
    let details = preview.as_ref().and_then(|preview| preview.details.clone());
    let installed = details.as_ref().is_some_and(|details| details.installed);
    let formula = details
        .as_ref()
        .is_some_and(|details| !details.package.formula.is_empty());
    let label = if installed {
        "Already installed"
    } else if formula {
        "Install with Homebrew"
    } else {
        "Download and install"
    };
    let review_tone = if details.is_some() {
        Tone::Default
    } else {
        Tone::Primary
    };
    let install_tone = if details.is_some() {
        Tone::Primary
    } else {
        Tone::Default
    };

    view()
        .flex_row()
        .items_center()
        .flex_wrap()
        .gap(tokens::space(2.0))
        .child(action_button(
            "Review package",
            review_tone,
            "software-review",
            || sw::operation_busy() || sw::version().trim().is_empty(),
            sw::review,
        ))
        .child(action_button(
            label,
            install_tone,
            "software-install",
            || !sw::can_install(),
            sw::install,
        ))
}

fn catalog_versions() -> impl IntoElement {
    let kind = sw::active_software();
    let versions = sw::versions(kind);
    view()
        .flex_col()
        .gap(tokens::space(1.5))
        .child(ui::field_label("Catalog versions"))
        .child(
            view()
                .flex_row()
                .items_center()
                .flex_wrap()
                .gap(tokens::space(1.5))
                .child(
                    versions
                        .into_iter()
                        .map(move |version| version_chip(kind, version)),
                ),
        )
}

fn version_chip(kind: SoftwareKind, version: String) -> impl IntoElement {
    let pressed = version.clone();
    let selected_version = version.clone();
    ui::chip(
        version.clone(),
        move || sw::version() == selected_version,
        format!("version-{}-{}", kind.key(), version),
        false,
        move || sw::set_version(pressed.clone()),
    )
}

fn version_field() -> impl IntoElement {
    ui::field(
        "Exact version",
        ui::text_field(
            "software-version-exact",
            "For example 22.14.0",
            sw::version,
            sw::set_version,
        ),
    )
}

fn preview_inner(preview: sw::Preview) -> impl IntoElement {
    let error = preview.error.clone();
    let loading = preview.loading;
    let details = preview.details.clone();
    view()
        .test_id("software-preview")
        .flex_col()
        .gap(tokens::space(2.0))
        .child(
            view()
                .flex_col()
                .gap(tokens::space(2.0))
                .p(tokens::space(3.0))
                .rounded(move || theme().radius)
                .bg(tokens::inset)
                .child(when(loading, || {
                    text("Reviewing…")
                        .text_size(px(tokens::TEXT_SMALL))
                        .text_color(move || theme().muted_foreground)
                }))
                .child(error.map(|message| {
                    text(message)
                        .text_size(px(tokens::TEXT_SMALL))
                        .text_color(move || theme().destructive)
                        .selectable(true)
                }))
                .child(details.map(preview_details)),
        )
}

fn preview_details(details: PackageDetails) -> impl IntoElement {
    let manifest = details.package.clone();
    let id = manifest.id();
    let title = sw::title_of(&manifest.name);
    let version = manifest.version.clone();
    let source = manifest.url.clone();
    let checksum = manifest.sha256.clone();
    let formula = !manifest.formula.is_empty();
    let archive_bytes = details.archive_bytes;
    let cached = details.cached;
    let installed = details.installed;
    let compatible = details.compatible;
    let compatibility = details.compatibility.clone();
    let references = details.references.clone();
    let needs_repair = details.installed && details.integrity == "Needs repair";
    let integrity_note = details.integrity_note.clone();
    let details_text = package_details_text(&details);
    let progress_id = id.clone();

    view()
        .flex_col()
        .gap(tokens::space(2.0))
        .child(
            view()
                .flex_row()
                .items_center()
                .flex_wrap()
                .gap(tokens::space(1.5))
                .child(
                    icons::package()
                        .size(px(13.0))
                        .flex_none()
                        .text_color(move || theme().muted_foreground),
                )
                .child(
                    text(format!("{title} {version}"))
                        .flex_1()
                        .min_w(px(0.0))
                        .text_size(px(tokens::TEXT_BODY))
                        .font_weight(FontWeight::MEDIUM)
                        .tabular_nums()
                        .text_color(move || theme().foreground)
                        .truncate(),
                )
                .child(installed.then(|| tag("Installed", TagTone::Success)))
                .child(cached.then(|| tag("Cached", TagTone::Neutral))),
        )
        .child(
            view()
                .flex_row()
                .items_center()
                .flex_wrap()
                .gap(tokens::space(1.5))
                .child(action_button(
                    "Copy Source",
                    Tone::Default,
                    "software-copy-source",
                    false,
                    move || sw::copy(&source),
                ))
                .child(action_button(
                    "Copy Checksum",
                    Tone::Default,
                    "software-copy-checksum",
                    false,
                    move || sw::copy(&checksum),
                ))
                .child(when(!formula && archive_bytes > 0, move || {
                    text(format!("Archive size {}", sw::format_bytes(archive_bytes)))
                        .text_size(px(tokens::TEXT_CAPTION))
                        .tabular_nums()
                        .text_color(move || theme().muted_foreground)
                })),
        )
        .child(when(!compatible, move || {
            text(compatibility.clone())
                .text_size(px(tokens::TEXT_SMALL))
                .text_color(move || theme().destructive)
        }))
        .child(
            text(details_text)
                .font_family("monospace")
                .tabular_nums()
                .text_size(px(tokens::TEXT_CAPTION))
                .text_color(move || theme().foreground)
                .selectable(true),
        )
        .child(references.into_iter().map(|reference| {
            text(reference)
                .text_size(px(tokens::TEXT_CAPTION))
                .text_color(move || theme().muted_foreground)
        }))
        .child(when(needs_repair, move || {
            text(integrity_note.clone())
                .text_size(px(tokens::TEXT_CAPTION))
                .text_color(tokens::warning)
        }))
        .child(dynamic(move || {
            sw::progress(&progress_id).map(|line| {
                text(line)
                    .text_size(px(tokens::TEXT_CAPTION))
                    .text_color(tokens::success)
                    .selectable(true)
            })
        }))
}

/// The Go `packageDetailsText`: source, checksum and folder of the package.
fn package_details_text(details: &PackageDetails) -> String {
    let package = &details.package;
    if !package.formula.is_empty() {
        return format!(
            "Homebrew formula: {}\nVersion: {}\nSource: {}\nFormula SHA-256: {}\nArchitecture: {}\nLicense: {}\nCellar folder: {}",
            package.formula,
            package.version,
            package.url,
            package.sha256,
            package.arch,
            package.license,
            details.location.display()
        );
    }
    let minimum = if package.min_macos.is_empty() {
        "See upstream requirements".to_string()
    } else {
        format!("macOS {} or newer", package.min_macos)
    };
    let text = format!(
        "Source: {}\nSHA-256: {}\nArchitecture: {}\nSystem: {}\nLicense: {}\nInstallation folder: {}",
        package.url,
        package.sha256,
        package.arch,
        minimum,
        package.license,
        details.location.display()
    );
    if package.java_product.is_empty() {
        text
    } else {
        format!("Distribution: {}\n{text}", package.java_product)
    }
}

// --- Cache -------------------------------------------------------------------

fn cache_card() -> impl IntoElement {
    card(
        "Download cache",
        dynamic(|| Some(cache_body().into_element())),
    )
}

fn cache_body() -> impl IntoElement {
    let info = sw::cache();
    let empty = info.files == 0;
    let directory = info.directory.display().to_string();
    let has_directory = !directory.is_empty();
    let meta = if empty {
        "Empty".to_string()
    } else {
        format!(
            "{} · {}",
            sw::plural(info.files as usize, "archive"),
            sw::format_bytes(info.bytes)
        )
    };
    let disabled = empty || sw::operation_busy();

    view()
        .flex_col()
        .gap(tokens::space(2.0))
        .child(
            text(meta)
                .text_size(px(tokens::TEXT_SMALL))
                .tabular_nums()
                .text_color(move || theme().foreground),
        )
        .child(when(has_directory, move || {
            view()
                .flex_row()
                .items_center()
                .gap(tokens::space(1.5))
                .min_w(px(0.0))
                .child(
                    icons::folder()
                        .size(px(12.0))
                        .flex_none()
                        .text_color(move || theme().muted_foreground),
                )
                .child(
                    text(directory.clone())
                        .font_family("monospace")
                        .text_size(px(tokens::TEXT_CAPTION))
                        .text_color(move || theme().muted_foreground)
                        .truncate(),
                )
        }))
        .child(
            view()
                .flex_row()
                .items_center()
                .gap(tokens::space(2.0))
                .child(action_button(
                    "Clear download cache",
                    Tone::Default,
                    "clear-cache",
                    disabled,
                    sw::request_clear_cache,
                ))
                .child(when(sw::cache_busy(), || {
                    text("Clearing cached archives…")
                        .text_size(px(tokens::TEXT_CAPTION))
                        .text_color(move || theme().muted_foreground)
                })),
        )
}
