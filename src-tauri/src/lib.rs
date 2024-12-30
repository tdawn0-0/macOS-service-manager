mod brew_command;
mod open_file_command;

use brew_command::{get_brew_service_info, get_brew_services, manage_brew_service};
use open_file_command::open_log_in_console;
use specta_typescript::Typescript;
use tauri_specta::{collect_commands, Builder};

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let builder = Builder::<tauri::Wry>::new()
        // Then register them (separated by a comma)
        .commands(collect_commands![
            get_brew_services,
            manage_brew_service,
            get_brew_service_info,
            open_log_in_console,
        ]);

    #[cfg(debug_assertions)] // <- Only export on non-release builds
    builder
        .export(Typescript::default(), "../src/ipc/tauri/bindings.ts")
        .expect("Failed to export typescript bindings");

    tauri::Builder::default()
        // and finally tell Tauri how to invoke them
        .invoke_handler(builder.invoke_handler())
        .setup(move |app| {
            // This is also required if you want to use events
            builder.mount_events(app);
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
