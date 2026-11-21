//! Single-binary entry point for Devbase (`cargo run` and packaged `Devbase.app`).
//!
//! Before AppKit starts, `argv` is dispatched through [`launch::run`]:
//! - `--dev …` runs the shipped internal commands (`serve`, `tunnel`, `maintain`,
//!   `database-worker`, `node`, `helper`);
//! - a public-CLI-shaped argv runs the `devbase` CLI contract;
//! - an empty or GUI-shaped argv (including `--background` from the login
//!   LaunchAgent) starts the macOS desktop app with the theme, capabilities,
//!   main-window options, and unified logging from `nsh.json` and `theme.json`.

#[cfg(not(target_os = "macos"))]
compile_error!("Devbase targets Apple Silicon macOS 26 and later.");

#[cfg(target_os = "macos")]
mod macos {
    use std::{
        io::Write,
        path::{Path, PathBuf},
        process::ExitCode,
    };

    use nsh_app_devbase::launch::{self, Seams};
    use nsh_core::desktop::MainWindowOptions;
    use nsh_platform_macos::{
        ActivationPolicy, AppOptions, AppSource, Appearance, AssetBase, Limits, Root,
    };

    const NSH_JSON: &str = include_str!("../nsh.json");
    const THEME_JSON: &str = include_str!("../theme.json");

    struct Manifest {
        name: String,
        id: String,
        appearance: Appearance,
        activation_policy: ActivationPolicy,
        capabilities_json: Option<String>,
        main_window: MainWindowOptions,
    }

    impl Manifest {
        fn load() -> Self {
            let value: serde_json::Value =
                serde_json::from_str(NSH_JSON).expect("app/nsh.json must be valid JSON");
            let name = value
                .get("name")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("Devbase")
                .to_owned();
            let id = value
                .get("id")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("xyz.jyeu.devbase")
                .to_owned();
            let appearance = match value.get("appearance").and_then(serde_json::Value::as_str) {
                Some("light") => Appearance::Light,
                Some("dark") => Appearance::Dark,
                _ => Appearance::System,
            };
            let activation_policy = match value
                .get("macos")
                .and_then(|macos| macos.get("activationPolicy"))
                .and_then(serde_json::Value::as_str)
            {
                Some("accessory") => ActivationPolicy::Accessory,
                _ => ActivationPolicy::Regular,
            };
            let capabilities_json = value
                .get("capabilities")
                .map(|caps| serde_json::to_string(caps).expect("capabilities JSON"));
            let main_window = value
                .get("window")
                .map(|window| {
                    MainWindowOptions::from_json(window)
                        .expect("app/nsh.json window config must be valid")
                })
                .unwrap_or_default();
            Self {
                name,
                id,
                appearance,
                activation_policy,
                capabilities_json,
                main_window,
            }
        }
    }

    fn build_root(manifest: &Manifest, background: bool) -> Root {
        let tokens = nsh_base::Theme::from_json(THEME_JSON)
            .expect("app/theme.json must be valid")
            .into_tokens();
        let mut main_window = manifest.main_window.clone();
        if background {
            // `--background` (login LaunchAgent / `devbase gui --background`)
            // opens the app hidden in the menu bar while keeping regular
            // activation so opening the window from the tray or Dock works.
            main_window.default_visible = Some(false);
        }
        let mut root = Root::of::<nsh_app_devbase::App>()
            .tokens(tokens)
            .app_id(manifest.id.clone())
            .main_window(main_window);
        if manifest.appearance != Appearance::System {
            root = root.appearance(manifest.appearance);
        }
        if let Some(capabilities) = &manifest.capabilities_json {
            root = root.capabilities_json(capabilities);
        }
        root
    }

    fn bundle_resources_dir() -> (Option<PathBuf>, bool, bool) {
        if let Some(dir) = std::env::var_os("NSH_APP_DIR") {
            return (Some(PathBuf::from(dir)), false, true);
        }
        let contents = std::env::current_exe()
            .and_then(|exe| exe.canonicalize())
            .ok()
            .and_then(|exe| Some(exe.parent()?.parent()?.to_path_buf()));
        if let Some(contents) = contents
            && contents.join("Info.plist").is_file()
        {
            return (Some(contents.join("Resources/nsh")), true, false);
        }
        (None, false, true)
    }

    fn dev_icon_path() -> Option<PathBuf> {
        let candidate = Path::new(env!("CARGO_MANIFEST_DIR")).join("../resources/icon.png");
        candidate.is_file().then_some(candidate)
    }

    fn install_gui_logger(development: bool, app_id: &str, data_dir: Option<&Path>) {
        let mut unified = nsh_platform_apple::unified_log::Options::default();
        unified.development = development;
        unified.filtered = true;
        let unified = nsh_platform_apple::unified_log::logger(unified);

        let mut logging = nsh_engine::logging::LogOptions::default();
        logging.development = development;
        logging.directory = match data_dir {
            Some(dir) => Some(dir.join("logs")),
            None => nsh_engine::logging::log_dir(app_id),
        };
        logging.output = Some(Box::new(move |record: &log::Record<'_>| {
            unified.log(record)
        }));
        nsh_engine::logging::install(logging);
    }

    fn monitor_in_info_plist() -> bool {
        use nsh_platform_macos::objc2_foundation::{NSBundle, NSNumber, NSString};
        NSBundle::mainBundle()
            .objectForInfoDictionaryKey(&NSString::from_str("NSHPerformanceMonitor"))
            .and_then(|value| value.downcast::<NSNumber>().ok())
            .is_some_and(|value| value.boolValue())
    }

    /// Production dispatch: `devcli` for `--dev` and public CLI, `nsh` for the GUI.
    struct SystemSeams;

    impl Seams for SystemSeams {
        fn internal(&self, args: &[String]) -> ExitCode {
            let _ = env_logger::try_init();
            let mut out = std::io::stdout();
            let mut err = std::io::stderr();
            match devcli::run_internal_with(args, &devcli::SystemInternal, &mut out, &mut err) {
                Ok(()) => ExitCode::SUCCESS,
                Err(error) => {
                    let _ = writeln!(err, "{error}");
                    ExitCode::FAILURE
                }
            }
        }

        fn public_cli(&self, args: &[String]) -> ExitCode {
            let _ = env_logger::try_init();
            let mut env = devcli::CliEnv::default_env(Box::new(std::io::stdin()));
            let mut out = std::io::stdout();
            let mut err = std::io::stderr();
            match devcli::run_cli_with_env(args, &mut env, &mut out, &mut err) {
                Ok(()) => ExitCode::SUCCESS,
                Err(error) => {
                    if env.json {
                        let _ = devcli::output::write_action_error_json(
                            &mut err,
                            error.message(),
                            error.exit_code(),
                        );
                    } else {
                        let _ = devcli::output::write_human_error(&mut err, error.message());
                    }
                    ExitCode::from(u8::try_from(error.exit_code()).unwrap_or(1))
                }
            }
        }

        fn gui(&self, background: bool) -> ExitCode {
            let manifest = Manifest::load();
            let (resources_dir, in_bundle, developing) = bundle_resources_dir();
            let data_dir = resources_dir
                .as_deref()
                .and_then(|dir| std::fs::read_to_string(dir.join("app.json")).ok())
                .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
                .and_then(|info| {
                    info.get("dataDir")
                        .and_then(serde_json::Value::as_str)
                        .and_then(nsh_engine::resolve_data_dir)
                });
            install_gui_logger(developing, &manifest.id, data_dir.as_deref());

            nsh_platform_macos::set_ui_sets(vec![
                nsh_base::registry_of([nsh_base::parts::core as nsh_base::parts::Part]),
                nsh_app_devbase::__nsh_native_module(),
            ]);

            let default_size = manifest
                .main_window
                .default_size
                .map_or((1100, 720), |size| {
                    (size.width.0.round() as i32, size.height.0.round() as i32)
                });
            let options = AppOptions {
                app_id: manifest.id.clone(),
                title: manifest.name.clone(),
                size: default_size,
                appearance: manifest.appearance,
                activation_policy: manifest.activation_policy,
                asset_base: resources_dir.map(|dir| AssetBase::Directory(dir.join("assets"))),
                icon: (!in_bundle).then(dev_icon_path).flatten(),
                limits: Limits::standalone(),
                data_dir,
                login_item: in_bundle && !developing,
                keep_alive_on_close: true,
                fps: monitor_in_info_plist(),
                ..AppOptions::default()
            };

            let root = build_root(&manifest, background);
            nsh_platform_macos::run_app(AppSource::Rust(root), options)
        }
    }

    pub fn entry() -> ExitCode {
        let args: Vec<String> = std::env::args().skip(1).collect();
        launch::run(&args, &SystemSeams)
    }

    #[cfg(test)]
    mod tests {
        use nsh_core::desktop::TitleBarStyle;

        use super::*;

        #[test]
        fn manifest_and_root_match_macos_chrome_contract() {
            let manifest = Manifest::load();
            assert_eq!(manifest.name, "Devbase");
            assert_eq!(manifest.id, "xyz.jyeu.devbase");
            assert!(manifest.main_window.restore_bounds);
            assert_eq!(manifest.main_window.attributes.close_on_press, Some(false));
            assert_eq!(manifest.main_window.attributes.transparent, Some(true));
            let title_bar = manifest
                .main_window
                .attributes
                .title_bar
                .expect("hidden titleBar must be configured");
            assert_eq!(title_bar.style, TitleBarStyle::Hidden);
            assert_eq!(title_bar.height.map(|h| h.0), Some(52.0));

            let foreground = build_root(&manifest, false);
            assert_eq!(foreground.app_id_ref(), Some("xyz.jyeu.devbase"));
            assert!(
                foreground
                    .main_window_ref()
                    .expect("main_window set")
                    .visible_at_launch()
            );
            assert!(
                !foreground
                    .capabilities_ref()
                    .expect("capabilities set")
                    .speech,
                "speech capability must stay disabled so macOS mic/dictation plist keys stay absent"
            );

            let background = build_root(&manifest, true);
            assert!(
                !background
                    .main_window_ref()
                    .expect("main_window set")
                    .visible_at_launch()
            );
        }
    }
}

#[cfg(target_os = "macos")]
fn main() -> std::process::ExitCode {
    macos::entry()
}
