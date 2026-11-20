//! The tunnel supervisor and per-Worker runner.
//!
//! Ported from `tunnel_client.go`. One outbound WebSocket per Worker; a 500 ms
//! exponential backoff capped at 15 s; auth failures wait 15 s and show
//! `AuthFailed`; hot reload of `tunnel.json` and `tunnel.key` restarts runners;
//! live state is published only through `proxy/tunnel-status.json`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::sync::{mpsc, watch};

use super::config::{
    TunnelConflict, TunnelPublishedConfig, TunnelRoute, TunnelStatusSnapshot,
    read_published_config_with_digest, write_status,
};
use super::proto::{
    FLAG_FIN, FRAME_REGISTER, FRAME_REGISTER_ACK, RegisterAckPayload, RegisterPayload,
    decode_frame, encode_json_frame, sign_handshake,
};
use super::session::TunnelSession;
use super::ws::{WsError, dial};
use crate::secrets::SecretStore;

const KEEPALIVE: Duration = Duration::from_secs(30);
const READ_TIMEOUT: Duration = Duration::from_secs(75);
const BACKOFF_INITIAL: Duration = Duration::from_millis(500);
const BACKOFF_MAX: Duration = Duration::from_secs(15);
const AUTH_WAIT: Duration = Duration::from_secs(15);
const STABLE_SESSION: Duration = Duration::from_secs(15);

/// A supervisor failure that ends `serve`.
#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Config(#[from] super::config::TunnelConfigError),
    #[error(transparent)]
    Secret(#[from] crate::secrets::SecretError),
    #[error(transparent)]
    Ws(#[from] WsError),
    #[error(transparent)]
    Proto(#[from] super::proto::TunnelProtoError),
    #[error("{0}")]
    Invalid(String),
}

#[derive(Debug, Clone, Default)]
struct WorkerStatus {
    phase: String,
    conflicts: Vec<TunnelConflict>,
}

struct BoardInner {
    pid: i64,
    runners: BTreeSet<String>,
    statuses: BTreeMap<String, WorkerStatus>,
}

/// Shared live status for every runner, written atomically after each change.
#[derive(Clone)]
pub struct StatusBoard {
    root: PathBuf,
    inner: Arc<Mutex<BoardInner>>,
}

impl StatusBoard {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        StatusBoard {
            root: root.into(),
            inner: Arc::new(Mutex::new(BoardInner {
                pid: std::process::id() as i64,
                runners: BTreeSet::new(),
                statuses: BTreeMap::new(),
            })),
        }
    }

    fn register(&self, worker: &str) {
        let mut inner = self.inner.lock().unwrap();
        inner.runners.insert(worker.to_string());
        inner.statuses.insert(
            worker.to_string(),
            WorkerStatus {
                phase: "Connecting".into(),
                conflicts: Vec::new(),
            },
        );
        self.write_locked(&inner);
    }

    fn unregister(&self, worker: &str) {
        let mut inner = self.inner.lock().unwrap();
        inner.runners.remove(worker);
        inner.statuses.remove(worker);
        self.write_locked(&inner);
    }

    fn update(&self, worker: &str, status: WorkerStatus) {
        let mut inner = self.inner.lock().unwrap();
        if !inner.runners.contains(worker) {
            return;
        }
        inner.statuses.insert(worker.to_string(), status);
        self.write_locked(&inner);
    }

    fn write_locked(&self, inner: &BoardInner) {
        let _ = write_status(&self.root, snapshot(inner));
    }

    /// The snapshot the GUI would read, for tests.
    pub fn snapshot(&self) -> TunnelStatusSnapshot {
        let inner = self.inner.lock().unwrap();
        snapshot(&inner)
    }
}

fn snapshot(inner: &BoardInner) -> TunnelStatusSnapshot {
    let total = inner.runners.len() as i64;
    if total == 0 {
        return TunnelStatusSnapshot {
            pid: inner.pid,
            phase: "Stopped".into(),
            connected: 0,
            total: 0,
            conflicts: Vec::new(),
            worker_states: BTreeMap::new(),
            updated_at: chrono::Utc::now(),
        };
    }
    let mut connected = 0;
    let mut connecting = 0;
    let mut auth_failed = 0;
    let mut conflicts = Vec::new();
    let mut worker_states = BTreeMap::new();
    for worker in &inner.runners {
        let status = inner.statuses.get(worker).cloned().unwrap_or_default();
        match status.phase.as_str() {
            "Connected" => {
                if status.conflicts.is_empty() {
                    connected += 1;
                    worker_states.insert(worker.clone(), "Connected".into());
                } else {
                    conflicts.extend(status.conflicts.clone());
                    worker_states.insert(worker.clone(), "Conflict".into());
                }
            }
            "AuthFailed" => {
                auth_failed += 1;
                worker_states.insert(worker.clone(), "Reconnecting".into());
            }
            "Connecting" => {
                connecting += 1;
                worker_states.insert(worker.clone(), "Reconnecting".into());
            }
            _ => {
                worker_states.insert(worker.clone(), "Reconnecting".into());
            }
        }
    }
    let phase = if connected > 0 || !conflicts.is_empty() {
        "Connected"
    } else if auth_failed > 0 {
        "AuthFailed"
    } else if connecting == total {
        "Connecting"
    } else {
        "Reconnecting"
    };
    TunnelStatusSnapshot {
        pid: inner.pid,
        phase: phase.into(),
        connected,
        total,
        conflicts,
        worker_states,
        updated_at: chrono::Utc::now(),
    }
}

struct Runner {
    route: TunnelRoute,
    cancel: watch::Sender<bool>,
    handle: tokio::task::JoinHandle<()>,
}

/// Run the tunnel until `shutdown` turns true.
pub async fn serve(
    root: PathBuf,
    event: Arc<dyn Fn(&str) + Send + Sync>,
    mut shutdown: watch::Receiver<bool>,
) -> Result<(), ClientError> {
    let board = StatusBoard::new(&root);
    let secrets = SecretStore::new(&root);
    let mut runners: BTreeMap<String, Runner> = BTreeMap::new();

    let mut cfg = read_published_config_with_digest(&root)?;
    let (mut key_digest, mut secret) = read_secret_digest(&secrets)?;
    reconcile(
        &board,
        &mut runners,
        &cfg.0,
        &secret,
        shutdown.clone(),
        event.clone(),
    )
    .await;
    let _ = write_status(&root, board.snapshot());

    let mut tick = tokio::time::interval(Duration::from_secs(1));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            _ = shutdown.changed() => break,
            _ = tick.tick() => {}
        }
        let new_cfg = read_published_config_with_digest(&root);
        let new_key = read_secret_digest(&secrets);
        let mut restart_all = false;
        let mut cfg_changed = false;
        if let Ok((config, digest)) = &new_cfg
            && *digest != cfg.1
        {
            if config.client_id != cfg.0.client_id {
                restart_all = true;
            } else {
                cfg_changed = true;
            }
            cfg = (config.clone(), *digest);
        }
        if let Ok((digest, value)) = &new_key
            && *digest != key_digest
        {
            secret = value.clone();
            key_digest = *digest;
            restart_all = true;
        }
        if restart_all {
            stop_all(&mut runners).await;
            reconcile(
                &board,
                &mut runners,
                &cfg.0,
                &secret,
                shutdown.clone(),
                event.clone(),
            )
            .await;
        } else if cfg_changed {
            reconcile(
                &board,
                &mut runners,
                &cfg.0,
                &secret,
                shutdown.clone(),
                event.clone(),
            )
            .await;
        }
    }

    stop_all(&mut runners).await;
    let _ = write_status(&root, TunnelStatusSnapshot::default());
    Ok(())
}

fn read_secret_digest(secrets: &SecretStore) -> Result<([u8; 32], String), ClientError> {
    use sha2::{Digest, Sha256};
    let bytes = std::fs::read(secrets.tunnel_key_path())?;
    let secret = crate::secrets::validate_tunnel_secret(&String::from_utf8_lossy(&bytes))?;
    Ok((Sha256::digest(&bytes).into(), secret))
}

async fn reconcile(
    board: &StatusBoard,
    runners: &mut BTreeMap<String, Runner>,
    cfg: &TunnelPublishedConfig,
    secret: &str,
    shutdown: watch::Receiver<bool>,
    event: Arc<dyn Fn(&str) + Send + Sync>,
) {
    let desired: BTreeMap<String, TunnelRoute> = cfg
        .routes
        .iter()
        .map(|route| (route.worker_name.clone(), route.clone()))
        .collect();
    let mut to_stop = Vec::new();
    for (name, runner) in runners.iter() {
        match desired.get(name) {
            Some(route) if route == &runner.route => {}
            _ => to_stop.push(name.clone()),
        }
    }
    for name in to_stop {
        if let Some(runner) = runners.remove(&name) {
            let _ = runner.cancel.send(true);
            let _ = runner.handle.await;
        }
        board.unregister(&name);
    }

    for (name, route) in desired {
        if runners.contains_key(&name) {
            continue;
        }
        board.register(&name);
        let (cancel, cancel_rx) = watch::channel(false);
        let board = board.clone();
        let secret = secret.to_string();
        let client_id = cfg.client_id.clone();
        let shutdown = shutdown.clone();
        let event = event.clone();
        let route_for_task = route.clone();
        let handle = tokio::spawn(async move {
            run_runner(
                board,
                route_for_task,
                client_id,
                secret,
                cancel_rx,
                shutdown,
                event,
            )
            .await;
        });
        runners.insert(
            name,
            Runner {
                route,
                cancel,
                handle,
            },
        );
    }
}

async fn stop_all(runners: &mut BTreeMap<String, Runner>) {
    let runners = std::mem::take(runners);
    for (_, runner) in runners {
        let _ = runner.cancel.send(true);
        let _ = runner.handle.await;
    }
}

struct RunFailure {
    message: String,
    auth_failed: bool,
}

async fn run_runner(
    board: StatusBoard,
    route: TunnelRoute,
    client_id: String,
    secret: String,
    mut cancel: watch::Receiver<bool>,
    mut shutdown: watch::Receiver<bool>,
    event: Arc<dyn Fn(&str) + Send + Sync>,
) {
    let mut backoff = BACKOFF_INITIAL;
    loop {
        if *cancel.borrow() || *shutdown.borrow() {
            return;
        }
        let started = Instant::now();
        let result = run_once(
            &board,
            &route,
            &client_id,
            &secret,
            &mut cancel,
            &mut shutdown,
        )
        .await;
        if *cancel.borrow() || *shutdown.borrow() {
            return;
        }
        let auth_failed = matches!(&result, Err(failure) if failure.auth_failed);
        if started.elapsed() >= STABLE_SESSION {
            backoff = BACKOFF_INITIAL;
        }
        let wait = if auth_failed {
            board.update(
                &route.worker_name,
                WorkerStatus {
                    phase: "AuthFailed".into(),
                    conflicts: Vec::new(),
                },
            );
            AUTH_WAIT
        } else {
            board.update(
                &route.worker_name,
                WorkerStatus {
                    phase: "Reconnecting".into(),
                    conflicts: Vec::new(),
                },
            );
            let current = backoff;
            backoff = (backoff * 2).min(BACKOFF_MAX);
            current
        };
        if let Err(failure) = result {
            (event)(&format!(
                "Tunnel ({}) disconnected: {}",
                route.worker_name, failure.message
            ));
        }
        tokio::select! {
            _ = cancel.changed() => return,
            _ = shutdown.changed() => return,
            _ = tokio::time::sleep(wait) => {}
        }
    }
}

async fn run_once(
    board: &StatusBoard,
    route: &TunnelRoute,
    client_id: &str,
    secret: &str,
    cancel: &mut watch::Receiver<bool>,
    shutdown: &mut watch::Receiver<bool>,
) -> Result<(), RunFailure> {
    let ws_url = route.web_socket_url().map_err(|error| RunFailure {
        message: error.to_string(),
        auth_failed: false,
    })?;
    let timestamp = chrono::Utc::now().timestamp().to_string();
    let nonce: String = hex::encode(rand::random::<[u8; 16]>());
    let signature = sign_handshake(secret, client_id, &timestamp, &nonce);
    let headers = vec![
        ("X-Devbase-Client".to_string(), client_id.to_string()),
        ("X-Devbase-Timestamp".to_string(), timestamp),
        ("X-Devbase-Nonce".to_string(), nonce),
        ("X-Devbase-Signature".to_string(), signature),
    ];
    let (ws, _response) = dial(&ws_url, &headers, &[], None).await.map_err(|error| {
        let auth_failed = matches!(
            &error,
            WsError::Handshake(handshake)
                if handshake.status_code == 401 || handshake.status_code == 403
        );
        RunFailure {
            message: error.to_string(),
            auth_failed,
        }
    })?;

    let (outgoing, mut outgoing_rx) = mpsc::unbounded_channel::<Vec<u8>>();
    let sender = ws.sender();
    let writer = tokio::spawn(async move {
        while let Some(frame) = outgoing_rx.recv().await {
            if sender.write_binary(&frame).await.is_err() {
                break;
            }
        }
    });

    let session = TunnelSession::new(route.clone(), outgoing, shutdown.clone());
    let register = encode_json_frame(
        FRAME_REGISTER,
        FLAG_FIN,
        0,
        0,
        &RegisterPayload {
            client_id: client_id.to_string(),
        },
    )
    .map_err(|error| RunFailure {
        message: error.to_string(),
        auth_failed: false,
    })?;
    if let Err(error) = ws.write_binary(&register).await {
        writer.abort();
        return Err(RunFailure {
            message: error.to_string(),
            auth_failed: false,
        });
    }

    let mut ping = tokio::time::interval(KEEPALIVE);
    ping.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut ws = ws;
    let mut cancel_for_ping = cancel.clone();
    let ping_sender = ws.sender();
    let ping_task = tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = ping.tick() => {
                    if ping_sender.write_ping(&[]).await.is_err() {
                        break;
                    }
                }
                _ = cancel_for_ping.changed() => break,
            }
        }
    });

    let result = loop {
        let message = tokio::select! {
            changed = cancel.changed() => {
                let _ = changed;
                break Ok(());
            }
            changed = shutdown.changed() => {
                let _ = changed;
                break Ok(());
            }
            message = tokio::time::timeout(READ_TIMEOUT, ws.read_message()) => message,
        };
        let message = match message {
            Ok(Ok(message)) => message,
            Ok(Err(WsError::Close(_close))) => {
                break Ok(());
            }
            Ok(Err(error)) => {
                break Err(RunFailure {
                    message: error.to_string(),
                    auth_failed: false,
                });
            }
            Err(_) => {
                break Err(RunFailure {
                    message: "read timeout".into(),
                    auth_failed: false,
                });
            }
        };
        let (opcode, data) = message;
        if opcode != crate::tunnel::ws::OP_BINARY {
            continue;
        }
        if let Ok(frame) = decode_frame(&data)
            && frame.frame_type == FRAME_REGISTER_ACK
        {
            if let Ok(ack) = serde_json::from_slice::<RegisterAckPayload>(frame.payload) {
                let conflicts = if !ack.ok && !ack.conflict_owner.is_empty() {
                    vec![TunnelConflict {
                        host: route.public_host(),
                        owner: ack.conflict_owner,
                    }]
                } else {
                    Vec::new()
                };
                board.update(
                    &route.worker_name,
                    WorkerStatus {
                        phase: "Connected".into(),
                        conflicts,
                    },
                );
            }
            continue;
        }
        session.dispatch(&data);
    };
    session.close_all();
    let _ = ws.write_close(1000, "shutdown").await;
    ping_task.abort();
    drop(session);
    writer.abort();
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn board_with(entries: &[(&str, &str)]) -> StatusBoard {
        let board = StatusBoard::new("/nonexistent");
        for (name, phase) in entries {
            board.register(name);
            board.update(
                name,
                WorkerStatus {
                    phase: (*phase).to_string(),
                    conflicts: Vec::new(),
                },
            );
        }
        board
    }

    #[test]
    fn phases_aggregate_like_the_go_status() {
        let board = board_with(&[("devbase-a", "Connected"), ("devbase-b", "Connecting")]);
        let status = board.snapshot();
        assert_eq!(status.phase, "Connected");
        assert_eq!(status.connected, 1);
        assert_eq!(status.total, 2);
        assert_eq!(status.worker_states["devbase-b"], "Reconnecting");

        let board = board_with(&[("devbase-a", "Connecting"), ("devbase-b", "Connecting")]);
        assert_eq!(board.snapshot().phase, "Connecting");

        let board = board_with(&[("devbase-a", "AuthFailed"), ("devbase-b", "Connecting")]);
        assert_eq!(board.snapshot().phase, "AuthFailed");

        let board = board_with(&[("devbase-a", "Reconnecting")]);
        assert_eq!(board.snapshot().phase, "Reconnecting");
        assert_eq!(board.snapshot().total, 1);
    }

    #[test]
    fn conflicts_are_reported_per_worker() {
        let board = StatusBoard::new("/nonexistent");
        board.register("devbase-a");
        board.update(
            "devbase-a",
            WorkerStatus {
                phase: "Connected".into(),
                conflicts: vec![TunnelConflict {
                    host: "a.example.com".into(),
                    owner: "other-laptop".into(),
                }],
            },
        );
        let status = board.snapshot();
        assert_eq!(status.phase, "Connected");
        assert_eq!(status.connected, 0);
        assert_eq!(status.worker_states["devbase-a"], "Conflict");
        assert_eq!(status.conflicts[0].owner, "other-laptop");
    }

    #[test]
    fn empty_board_is_stopped() {
        let board = StatusBoard::new("/nonexistent");
        assert_eq!(board.snapshot().phase, "Stopped");
        assert_eq!(board.snapshot().total, 0);
    }
}
