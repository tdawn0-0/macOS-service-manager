//! The inspector, headless: each launchd source's jobs, the search, the
//! detail groups, the provider's action matrix, the confirmation path and the
//! busy state while an action runs. A fake provider stands in for launchd:
//! no test reads the real LaunchAgents directories or runs launchctl.

use std::{
    path::PathBuf,
    sync::{Arc, Mutex, MutexGuard},
    thread,
    time::Duration,
};

use devcore::service::{
    Action, Autostart, BoxError, Detail, Item, LogFile, Provider, Row, Source, State,
};
use nsh_core::prelude::*;
use nsh_engine::{EngineConfig, Root};
use nsh_testing::{DialogAnswer, Headless};

use nsh_app_devbase::testing::{install_provider, use_fake_providers};

/// The service tests share one fake-provider registry, so they take turns.
static SERIAL: Mutex<()> = Mutex::new(());

fn serial() -> MutexGuard<'static, ()> {
    SERIAL
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Behaviour {
    Normal,
    /// Starting the job asks first.
    ConfirmStart,
    /// Every action takes this long, so the busy state can be seen.
    Slow,
    /// An action on an admin item fails like the missing helper does.
    HelperMissing,
}

struct FakeProvider {
    source: Source,
    items: Mutex<Vec<Item>>,
    behaviour: Behaviour,
}

impl FakeProvider {
    fn new(source: Source, items: Vec<Item>, behaviour: Behaviour) -> Arc<Self> {
        Arc::new(FakeProvider {
            source,
            items: Mutex::new(items),
            behaviour,
        })
    }
}

impl Provider for FakeProvider {
    fn source(&self) -> Source {
        self.source
    }

    fn list(&self) -> Result<Vec<Item>, BoxError> {
        Ok(self
            .items
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone())
    }

    fn detail(&self, item: &Item) -> Result<Detail, BoxError> {
        Ok(Detail {
            rows: vec![
                Row {
                    group: "Status".to_string(),
                    key: "Label".to_string(),
                    value: item.name.clone(),
                },
                Row {
                    group: "Status".to_string(),
                    key: "Domain".to_string(),
                    value: item.owner.clone(),
                },
                Row {
                    group: "Program".to_string(),
                    key: "Command".to_string(),
                    value: format!("/usr/bin/{}", item.name),
                },
                Row {
                    group: "Files".to_string(),
                    key: "Property list".to_string(),
                    value: item.path.display().to_string(),
                },
            ],
            logs: Vec::<LogFile>::new(),
        })
    }

    fn actions(&self, item: &Item) -> Vec<Action> {
        let mut actions = match item.state {
            State::Running if item.keep_alive => vec![Action::Stop, Action::Restart],
            State::Running => vec![Action::Stop, Action::Restart, Action::Unload],
            State::Loaded | State::Scheduled | State::Failed => {
                vec![Action::Start, Action::Unload]
            }
            _ => vec![Action::Load],
        };
        if item.autostart == Autostart::Off {
            actions.push(Action::Enable);
        } else {
            actions.push(Action::Disable);
        }
        actions.push(Action::Remove);
        actions
    }

    fn confirm(&self, item: &Item, action: Action) -> Option<String> {
        match (self.behaviour, action) {
            (Behaviour::ConfirmStart, Action::Start) => Some(format!("Start {} now?", item.name)),
            (_, Action::Remove) => Some(format!("Remove {}?", item.name)),
            _ => None,
        }
    }

    fn do_action(&self, item: &Item, action: Action) -> Result<(), BoxError> {
        if self.behaviour == Behaviour::Slow {
            thread::sleep(Duration::from_millis(500));
        }
        if self.behaviour == Behaviour::HelperMissing && item.admin {
            return Err("the privileged helper is not installed".into());
        }
        let mut items = self
            .items
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        match action {
            Action::Remove => items.retain(|held| held.id != item.id),
            Action::Start | Action::Load | Action::Restart => {
                if let Some(held) = items.iter_mut().find(|held| held.id == item.id) {
                    held.state = State::Running;
                    held.pid = 900;
                }
            }
            Action::Stop | Action::Unload => {
                if let Some(held) = items.iter_mut().find(|held| held.id == item.id) {
                    held.state = if action == Action::Unload {
                        State::Stopped
                    } else {
                        State::Loaded
                    };
                    held.pid = 0;
                }
            }
            Action::Enable => {
                if let Some(held) = items.iter_mut().find(|held| held.id == item.id) {
                    held.autostart = Autostart::On;
                }
            }
            Action::Disable => {
                if let Some(held) = items.iter_mut().find(|held| held.id == item.id) {
                    held.autostart = Autostart::Off;
                }
            }
        }
        Ok(())
    }
}

fn item(source: Source, name: &str, state: State, autostart: Autostart, admin: bool) -> Item {
    Item {
        id: format!("{}:{}", source.as_str(), name),
        source,
        name: name.to_string(),
        state,
        pid: if state == State::Running { 4242 } else { 0 },
        autostart,
        owner: match source {
            Source::Daemon => "system".to_string(),
            _ => "gui/501".to_string(),
        },
        path: PathBuf::from(format!("/tmp/devbase-tests/{name}.plist")),
        kind: None,
        admin,
        keep_alive: false,
    }
}

fn user_agents(behaviour: Behaviour) -> Arc<FakeProvider> {
    FakeProvider::new(
        Source::UserAgent,
        vec![
            item(
                Source::UserAgent,
                "com.example.alpha",
                State::Running,
                Autostart::On,
                false,
            ),
            item(
                Source::UserAgent,
                "com.example.bravo",
                State::Loaded,
                Autostart::On,
                false,
            ),
            item(
                Source::UserAgent,
                "com.example.charlie",
                State::Stopped,
                Autostart::Off,
                false,
            ),
        ],
        behaviour,
    )
}

fn system_agents() -> Arc<FakeProvider> {
    FakeProvider::new(
        Source::SystemAgent,
        vec![item(
            Source::SystemAgent,
            "com.example.beta",
            State::Loaded,
            Autostart::On,
            false,
        )],
        Behaviour::Normal,
    )
}

fn daemons(behaviour: Behaviour) -> Arc<FakeProvider> {
    FakeProvider::new(
        Source::Daemon,
        vec![item(
            Source::Daemon,
            "com.example.gamma",
            State::Running,
            Autostart::On,
            true,
        )],
        behaviour,
    )
}

/// Installs one fake per source and starts the surface, holding the tests'
/// turn for as long as the returned guard lives.
fn start(
    user: Arc<FakeProvider>,
    system: Arc<FakeProvider>,
    daemon: Arc<FakeProvider>,
) -> (MutexGuard<'static, ()>, Headless) {
    let turn = serial();
    use_fake_providers();
    install_provider(Source::UserAgent, user);
    install_provider(Source::SystemAgent, system);
    install_provider(Source::Daemon, daemon);
    let mut app = Headless::start(EngineConfig::new(Root::of::<nsh_app_devbase::App>())).unwrap();
    app.set_viewport(Size::new(px(1100.0), px(720.0))).unwrap();
    app.wait_for("Services").unwrap();
    (turn, app)
}

fn normal_app() -> (MutexGuard<'static, ()>, Headless) {
    start(
        user_agents(Behaviour::Normal),
        system_agents(),
        daemons(Behaviour::Normal),
    )
}

#[test]
fn switching_source_shows_that_sources_items() {
    let (_turn, mut app) = normal_app();
    app.wait_for("com.example.alpha").unwrap();

    app.press("id:source-system-agent").unwrap();
    app.wait_for("com.example.beta").unwrap();
    assert!(!app.screen().shows("com.example.alpha"));

    app.press("id:source-daemon").unwrap();
    app.wait_for("com.example.gamma").unwrap();
    assert!(!app.screen().shows("com.example.beta"));

    app.press("id:source-user-agent").unwrap();
    app.wait_for("com.example.alpha").unwrap();
}

#[test]
fn search_filters_the_list_by_name() {
    let (_turn, mut app) = normal_app();
    app.wait_for("com.example.alpha").unwrap();
    app.wait_for("com.example.bravo").unwrap();

    app.type_text("id:services-search", "alpha").unwrap();
    app.wait_for_gone("com.example.bravo", Duration::from_secs(2))
        .unwrap();
    assert!(app.screen().shows("com.example.alpha"));

    // Clearing the field restores the whole list.
    app.clear("id:services-search").unwrap();
    app.wait_for("com.example.bravo").unwrap();
}

#[test]
fn selecting_an_item_shows_its_detail_groups() {
    let (_turn, mut app) = normal_app();
    app.wait_for("com.example.alpha").unwrap();

    app.press("id:item-com.example.alpha").unwrap();
    app.wait_for("Program").unwrap();
    assert!(app.screen().shows("Files"));
    assert!(app.screen().shows("Property list"));
    assert!(
        app.screen()
            .shows("/tmp/devbase-tests/com.example.alpha.plist")
    );
    assert!(app.screen().shows("Running · PID 4242"));
}

#[test]
fn actions_follow_the_state_matrix() {
    let (_turn, mut app) = normal_app();
    app.wait_for("com.example.alpha").unwrap();

    app.press("id:item-com.example.alpha").unwrap();
    app.wait_for("/tmp/devbase-tests/com.example.alpha.plist")
        .unwrap();
    assert!(
        app.screen()
            .matches("id:inspector >> role:button[Stop]")
            .unwrap()
    );
    assert!(
        app.screen()
            .matches("id:inspector >> role:button[Restart]")
            .unwrap()
    );

    // Idle: Start is the main action, and Restart does not apply.
    app.press("id:item-com.example.bravo").unwrap();
    app.wait_for("/tmp/devbase-tests/com.example.bravo.plist")
        .unwrap();
    assert!(
        app.screen()
            .matches("id:inspector >> role:button[Start]")
            .unwrap()
    );
    assert!(
        !app.screen()
            .matches("id:inspector >> role:button[Restart]")
            .unwrap()
    );

    // Stopped and not autostarted: Load.
    app.press("id:item-com.example.charlie").unwrap();
    app.wait_for("/tmp/devbase-tests/com.example.charlie.plist")
        .unwrap();
    assert!(
        app.screen()
            .matches("id:inspector >> role:button[Load]")
            .unwrap()
    );
    assert!(
        !app.screen()
            .matches("id:inspector >> role:button[Stop]")
            .unwrap()
    );
}

#[test]
fn a_confirming_action_waits_for_the_dialog() {
    let (turn, mut app) = start(
        user_agents(Behaviour::ConfirmStart),
        system_agents(),
        daemons(Behaviour::Normal),
    );
    app.on_dialog(|_| DialogAnswer::Press("Start".to_string()));
    app.wait_for("com.example.bravo").unwrap();

    app.press("id:item-com.example.bravo").unwrap();
    app.wait_for("Start").unwrap();
    app.press("id:action-com.example.bravo").unwrap();
    app.wait_for("Running · PID 900").unwrap();

    drop(app);
    drop(turn);
}

#[test]
fn cancelling_a_confirming_action_changes_nothing() {
    let (turn, mut app) = start(
        user_agents(Behaviour::ConfirmStart),
        system_agents(),
        daemons(Behaviour::Normal),
    );
    app.wait_for("com.example.bravo").unwrap();
    app.press("id:item-com.example.bravo").unwrap();
    app.wait_for("Start").unwrap();

    app.press("id:action-com.example.bravo").unwrap();
    // The default answer presses Cancel: the action must not run.
    app.wait(Duration::from_millis(800)).unwrap();
    assert!(!app.screen().shows("Running · PID 900"));

    drop(app);
    drop(turn);
}

#[test]
fn an_action_shows_the_busy_state_while_it_runs() {
    let (_turn, mut app) = start(
        user_agents(Behaviour::Slow),
        system_agents(),
        daemons(Behaviour::Normal),
    );
    app.wait_for("com.example.alpha").unwrap();
    app.press("id:item-com.example.alpha").unwrap();
    app.wait_for("Stop").unwrap();

    app.press("id:action-com.example.alpha").unwrap();
    app.wait_for("Working").unwrap();
    app.wait_for("Idle").unwrap();
    app.wait_for_gone("Working", Duration::from_secs(2))
        .unwrap();
}

#[test]
fn a_privileged_action_reports_the_missing_helper() {
    let (_turn, mut app) = start(
        user_agents(Behaviour::Normal),
        system_agents(),
        daemons(Behaviour::HelperMissing),
    );
    app.wait_for("com.example.alpha").unwrap();
    app.press("id:source-daemon").unwrap();
    app.wait_for("com.example.gamma").unwrap();
    app.press("id:item-com.example.gamma").unwrap();
    app.wait_for("Stop").unwrap();

    app.press("id:action-com.example.gamma").unwrap();
    app.wait_for("com.example.gamma: the privileged helper is not installed")
        .unwrap();
}
