//! `https`, `tunnel`, and `ca` (ported from `cmd_https.go`) over
//! `devproxy`'s state types and services.
//!
//! Rule edits are transactional state writes with the same revision guard;
//! apply/stop/restart use `ProxyService`. Live HTTP probes and Cloudflare
//! discovery/sync are not part of this slice and report an explicit
//! `unavailable` operation error.

use std::collections::BTreeMap;
use std::io::Write;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::args::{FlagSpecs, ParseOutcome, has_help_flag, parse_flags, read_json_stdin};
use crate::commands::require_yes;
use crate::env::CliEnv;
use crate::error::{CliError, CliResult};
use crate::output::go_rfc3339_nano;

use chrono::{DateTime, SecondsFormat, Utc};
use devcore::model::{ProxyRule, State, local_domain, normalize_proxy_rules};
use devcore::state::Store;
use devproxy::ca::material::AuthorityInfo;
use devproxy::ca::service::{AlwaysStopped, AuthorityService, AuthorityStatus};
use devproxy::ca::store::AuthorityStore;
use devproxy::ca::trust::SecurityTrust;
use devproxy::proxy::lifecycle::ProxyService;
use devproxy::proxy::rules::publication_state;
use devproxy::tunnel::config::read_status;
use devproxy::tunnel::service::TunnelService;

fn store(env: &CliEnv) -> Store {
    Store::new(env.root.clone())
}

fn read_state(env: &CliEnv) -> CliResult<State> {
    Ok(store(env).read()?)
}

fn unavailable(feature: &str) -> CliError {
    CliError::operation(format!(
        "{feature} is unavailable in this build (the live probe/Cloudflare slice has not landed)"
    ))
}

fn authority_service(env: &CliEnv) -> AuthorityService {
    let runner = Arc::new(devcore::launchd::SystemRunner::new());
    AuthorityService::new(
        AuthorityStore::new(env.root.clone()),
        Arc::new(SecurityTrust::new(runner)),
        Arc::new(AlwaysStopped),
    )
}

fn proxy_service(env: &CliEnv) -> ProxyService {
    ProxyService::system(store(env), Arc::new(authority_service(env)))
}

fn proxy_status(env: &CliEnv, state: &State) -> (String, String) {
    let publication = publication_state(&store(env))
        .map(|value| value.to_string())
        .unwrap_or_else(|_| "Unknown".into());
    let status = format!("{} rules", state.proxy_rules.len());
    (status, publication)
}

fn replace_rules(
    store: &Store,
    listener_port: i64,
    rules: Vec<ProxyRule>,
    expected_revision: u64,
) -> CliResult<State> {
    store.with_state(|state| {
        if state.proxy_rules_revision != expected_revision {
            return Err(devcore::state::StateError::invalid(
                "proxy rules changed; reload and retry",
            ));
        }
        let normalized = normalize_proxy_rules(listener_port, &rules)?;
        state.proxy_rules = normalized;
        state.proxy_listener_port = listener_port;
        state.proxy_rules_revision += 1;
        Ok(())
    })?;
    Ok(store.read()?)
}

fn apply_if_requested(env: &CliEnv, apply: bool) -> CliResult<()> {
    if apply {
        proxy_service(env).apply()?;
    }
    Ok(())
}

pub fn run_https_command(env: &mut CliEnv, out: &mut dyn Write, args: &[String]) -> CliResult<()> {
    if has_help_flag(args) && args.len() == 1 {
        crate::schema::print_topic_help(out, "https")?;
        return Ok(());
    }
    let mut rest = args;
    let mut sub = "status";
    if let Some(first) = rest.first() {
        sub = first;
        rest = &rest[1..];
    }
    match sub {
        "status" | "rules" | "list" => {
            if let Some(extra) = rest.first() {
                return Err(CliError::usage(format!(
                    "unexpected argument {extra:?} for https {sub}"
                )));
            }
            let state = read_state(env)?;
            let (status, publication) = proxy_status(env, &state);
            let mut payload = BTreeMap::new();
            payload.insert("status", serde_json::Value::String(status.clone()));
            payload.insert(
                "publicationState",
                serde_json::Value::String(publication.clone()),
            );
            payload.insert(
                "listenerPort",
                serde_json::Value::from(state.proxy_listener_port),
            );
            payload.insert(
                "revision",
                serde_json::Value::from(state.proxy_rules_revision),
            );
            payload.insert("rules", serde_json::to_value(&state.proxy_rules).unwrap());
            crate::commands::write_result(env, out, &payload, |w| {
                writeln!(
                    w,
                    "HTTPS Proxy: {status} (listener :{}, revision {}, {publication})",
                    state.proxy_listener_port, state.proxy_rules_revision
                )?;
                if state.proxy_rules.is_empty() {
                    writeln!(w, "No HTTPS proxy rules configured.")?;
                    return Ok(());
                }
                for rule in &state.proxy_rules {
                    let upstream = if rule.upstream_host.is_empty() {
                        "127.0.0.1"
                    } else {
                        &rule.upstream_host
                    };
                    let mut line = format!(
                        "  - https://{} -> {}:{}",
                        rule.domain, upstream, rule.local_port
                    );
                    if rule.tunnel {
                        line.push_str(" [tunnel");
                        if !rule.tunnel_url.is_empty() {
                            line.push_str(": ");
                            line.push_str(&rule.tunnel_url);
                        }
                        line.push(']');
                    }
                    writeln!(w, "{line}")?;
                }
                Ok(())
            })
        }
        "add" => run_add(env, out, rest),
        "remove" => run_remove(env, out, rest),
        "set-port" => run_set_port(env, out, rest),
        "replace" => run_replace(env, out, rest),
        "apply" | "start" => {
            if let Some(extra) = rest.first() {
                return Err(CliError::usage(format!(
                    "unexpected argument {extra:?} for https {sub}"
                )));
            }
            proxy_service(env).apply()?;
            let publication = publication_state(&store(env))
                .map(|value| value.to_string())
                .unwrap_or_else(|_| "Unknown".into());
            #[derive(Serialize)]
            struct Payload {
                applied: bool,
                #[serde(rename = "publicationState")]
                publication_state: String,
            }
            let payload = Payload {
                applied: true,
                publication_state: publication,
            };
            crate::commands::write_result(env, out, &payload, |w| {
                writeln!(w, "HTTPS proxy applied ({}).", payload.publication_state)
            })
        }
        "stop" => {
            proxy_service(env).stop()?;
            #[derive(Serialize)]
            struct Payload {
                stopped: bool,
            }
            crate::commands::write_result(env, out, &Payload { stopped: true }, |w| {
                writeln!(w, "HTTPS proxy stopped.")
            })
        }
        "restart" => {
            proxy_service(env).restart()?;
            #[derive(Serialize)]
            struct Payload {
                restarted: bool,
            }
            crate::commands::write_result(env, out, &Payload { restarted: true }, |w| {
                writeln!(w, "HTTPS proxy restarted.")
            })
        }
        "check" => {
            if rest.len() != 1 {
                return Err(CliError::usage(
                    "usage: devbase https check <name-or-domain>",
                ));
            }
            Err(unavailable("live TLS+HTTP proxy checks"))
        }
        other => Err(CliError::usage(format!(
            "unknown https subcommand {other:?}"
        ))),
    }
}

fn parse_listener(parsed: &crate::args::Parsed) -> CliResult<i64> {
    let port = parsed.integer("listener-port")?.unwrap_or(0);
    if !(0..=65535).contains(&port) {
        return Err(CliError::usage("--listener-port must be 1–65535"));
    }
    Ok(port)
}

fn run_add(env: &mut CliEnv, out: &mut dyn Write, args: &[String]) -> CliResult<()> {
    let specs = FlagSpecs::new()
        .value("upstream-host")
        .bool("tunnel")
        .bool("tunnel-auth")
        .value("tunnel-worker")
        .value("tunnel-domain")
        .value("listener-port")
        .bool("apply");
    let ParseOutcome::Parsed(parsed) = parse_flags(args, &specs)? else {
        return Ok(());
    };
    if parsed.positionals.len() != 2 {
        return Err(CliError::usage(
            "usage: devbase https add <name-or-domain> <local-port> [--upstream-host <host>] [--tunnel] [--tunnel-auth] [--tunnel-worker <name>] [--tunnel-domain <host>] [--listener-port <port>] [--apply]",
        ));
    }
    let domain = local_domain(&parsed.positionals[0]);
    if domain.is_empty() {
        return Err(CliError::usage("domain cannot be empty"));
    }
    let local_port: i64 = parsed.positionals[1]
        .parse()
        .ok()
        .filter(|port| (1..=65535).contains(port))
        .ok_or_else(|| CliError::usage("local-port must be 1–65535"))?;
    let listener = parse_listener(&parsed)?;
    let state = read_state(env)?;
    let port = if listener > 0 {
        listener
    } else if state.proxy_listener_port == 0 {
        443
    } else {
        state.proxy_listener_port
    };
    let mut rule = ProxyRule {
        domain: domain.clone(),
        local_port,
        upstream_host: parsed.string("upstream-host").trim().to_string(),
        tunnel: parsed.flag("tunnel"),
        tunnel_auth: parsed.flag("tunnel-auth"),
        tunnel_worker: parsed.string("tunnel-worker").trim().to_string(),
        tunnel_domain: parsed.string("tunnel-domain").trim().to_string(),
        tunnel_url: String::new(),
    };
    let mut rules = Vec::with_capacity(state.proxy_rules.len() + 1);
    let mut replaced = false;
    for existing in &state.proxy_rules {
        if existing.domain == domain {
            rule.tunnel_url = existing.tunnel_url.clone();
            rules.push(rule.clone());
            replaced = true;
        } else {
            rules.push(existing.clone());
        }
    }
    if !replaced {
        rules.push(rule.clone());
    }
    let saved = replace_rules(&store(env), port, rules, state.proxy_rules_revision)?;
    apply_if_requested(env, parsed.flag("apply"))?;
    let saved = read_state(env).unwrap_or(saved);
    let saved_rule = saved
        .proxy_rules
        .iter()
        .find(|candidate| candidate.domain == domain)
        .cloned()
        .unwrap_or(rule);
    let payload = serde_json::json!({
        "rule": saved_rule,
        "listenerPort": saved.proxy_listener_port,
        "revision": saved.proxy_rules_revision,
        "rules": saved.proxy_rules,
        "applied": parsed.flag("apply"),
    });
    crate::commands::write_result(env, out, &payload, |w| {
        writeln!(
            w,
            "Saved HTTPS rule https://{} -> port {} (revision {}).",
            payload["rule"]["domain"].as_str().unwrap_or_default(),
            payload["rule"]["localPort"].as_i64().unwrap_or_default(),
            saved.proxy_rules_revision
        )
    })
}

fn run_remove(env: &mut CliEnv, out: &mut dyn Write, args: &[String]) -> CliResult<()> {
    let specs = FlagSpecs::new().bool("apply");
    let ParseOutcome::Parsed(parsed) = parse_flags(args, &specs)? else {
        return Ok(());
    };
    if parsed.positionals.len() != 1 {
        return Err(CliError::usage(
            "usage: devbase https remove <name-or-domain> [--apply]",
        ));
    }
    let domain = local_domain(&parsed.positionals[0]);
    let state = read_state(env)?;
    let rules: Vec<ProxyRule> = state
        .proxy_rules
        .iter()
        .filter(|rule| rule.domain != domain)
        .cloned()
        .collect();
    let removed = rules.len() != state.proxy_rules.len();
    let saved = replace_rules(
        &store(env),
        state.proxy_listener_port,
        rules,
        state.proxy_rules_revision,
    )?;
    apply_if_requested(env, parsed.flag("apply"))?;
    let saved = read_state(env).unwrap_or(saved);
    let mut payload = BTreeMap::new();
    payload.insert("removed", serde_json::Value::Bool(removed));
    payload.insert(
        "revision",
        serde_json::Value::from(saved.proxy_rules_revision),
    );
    payload.insert("rules", serde_json::to_value(&saved.proxy_rules).unwrap());
    payload.insert("applied", serde_json::Value::Bool(parsed.flag("apply")));
    crate::commands::write_result(env, out, &payload, |w| {
        writeln!(
            w,
            "Removed HTTPS rule https://{domain} (revision {}).",
            saved.proxy_rules_revision
        )
    })
}

fn run_set_port(env: &mut CliEnv, out: &mut dyn Write, args: &[String]) -> CliResult<()> {
    let specs = FlagSpecs::new().bool("apply");
    let ParseOutcome::Parsed(parsed) = parse_flags(args, &specs)? else {
        return Ok(());
    };
    if parsed.positionals.len() != 1 {
        return Err(CliError::usage(
            "usage: devbase https set-port <listener-port> [--apply]",
        ));
    }
    let port: i64 = parsed.positionals[0]
        .parse()
        .ok()
        .filter(|port| (1..=65535).contains(port))
        .ok_or_else(|| CliError::usage("listener-port must be 1–65535"))?;
    let state = read_state(env)?;
    let saved = replace_rules(
        &store(env),
        port,
        state.proxy_rules.clone(),
        state.proxy_rules_revision,
    )?;
    apply_if_requested(env, parsed.flag("apply"))?;
    let saved = read_state(env).unwrap_or(saved);
    let payload = serde_json::json!({
        "listenerPort": saved.proxy_listener_port,
        "revision": saved.proxy_rules_revision,
        "applied": parsed.flag("apply"),
    });
    crate::commands::write_result(env, out, &payload, |w| {
        writeln!(
            w,
            "HTTPS proxy listener port set to {port} (revision {}).",
            saved.proxy_rules_revision
        )
    })
}

#[derive(Deserialize)]
struct ReplaceObject {
    #[serde(default, rename = "listenerPort")]
    listener_port: i64,
    rules: Vec<ProxyRule>,
}

fn run_replace(env: &mut CliEnv, out: &mut dyn Write, args: &[String]) -> CliResult<()> {
    let specs = FlagSpecs::new()
        .bool("json-stdin")
        .value("listener-port")
        .bool("apply");
    let ParseOutcome::Parsed(parsed) = parse_flags(args, &specs)? else {
        return Ok(());
    };
    if let Some(extra) = parsed.positionals.first() {
        return Err(CliError::usage(format!(
            "unexpected argument {extra:?} for https replace"
        )));
    }
    if !parsed.flag("json-stdin") {
        return Err(CliError::usage(
            "usage: devbase https replace --json-stdin [--listener-port <port>] [--apply]",
        ));
    }
    let raw = read_json_stdin(&mut *env.input, 1 << 20)?;
    let mut listener = parse_listener(&parsed)?;
    let rules: Vec<ProxyRule> = match serde_json::from_slice::<serde_json::Value>(&raw) {
        Ok(serde_json::Value::Array(_)) => serde_json::from_slice(&raw)
            .map_err(|error| CliError::usage(format!("invalid rule JSON: {error}")))?,
        Ok(serde_json::Value::Object(_)) => {
            let object: ReplaceObject = serde_json::from_slice(&raw)
                .map_err(|error| CliError::usage(format!("invalid rule JSON: {error}")))?;
            if listener == 0 {
                listener = object.listener_port;
            }
            object.rules
        }
        _ => {
            return Err(CliError::usage(
                "JSON stdin must be an array of rules or {listenerPort, rules}",
            ));
        }
    };
    let state = read_state(env)?;
    let port = if listener > 0 {
        listener
    } else if state.proxy_listener_port == 0 {
        443
    } else {
        state.proxy_listener_port
    };
    let saved = replace_rules(&store(env), port, rules, state.proxy_rules_revision)?;
    apply_if_requested(env, parsed.flag("apply"))?;
    let saved = read_state(env).unwrap_or(saved);
    let payload = serde_json::json!({
        "listenerPort": saved.proxy_listener_port,
        "revision": saved.proxy_rules_revision,
        "rules": saved.proxy_rules,
        "applied": parsed.flag("apply"),
    });
    crate::commands::write_result(env, out, &payload, |w| {
        writeln!(
            w,
            "Replaced {} proxy rules (revision {}).",
            saved.proxy_rules.len(),
            saved.proxy_rules_revision
        )
    })
}

// ---- tunnel -------------------------------------------------------------

#[derive(Serialize)]
struct TunnelRuleView {
    domain: String,
    #[serde(rename = "localPort")]
    local_port: i64,
    #[serde(rename = "upstreamHost", skip_serializing_if = "String::is_empty")]
    upstream_host: String,
    tunnel: bool,
    #[serde(rename = "tunnelAuth", skip_serializing_if = "is_false")]
    tunnel_auth: bool,
    #[serde(rename = "tunnelWorker")]
    tunnel_worker: String,
    #[serde(rename = "tunnelDomain", skip_serializing_if = "String::is_empty")]
    tunnel_domain: String,
    #[serde(rename = "tunnelUrl", skip_serializing_if = "String::is_empty")]
    tunnel_url: String,
    #[serde(rename = "workerState", skip_serializing_if = "String::is_empty")]
    worker_state: String,
}

fn is_false(value: &bool) -> bool {
    !*value
}

pub fn run_tunnel_command(env: &mut CliEnv, out: &mut dyn Write, args: &[String]) -> CliResult<()> {
    if has_help_flag(args) && args.len() == 1 {
        crate::schema::print_topic_help(out, "tunnel")?;
        return Ok(());
    }
    let mut rest = args;
    let mut sub = "status";
    if let Some(first) = rest.first() {
        sub = first;
        rest = &rest[1..];
    }
    let service = TunnelService::new(store(env), "https://api.cloudflare.com");
    match sub {
        "status" => {
            if let Some(extra) = rest.first() {
                return Err(CliError::usage(format!(
                    "unexpected argument {extra:?} for tunnel status"
                )));
            }
            let state = read_state(env)?;
            let snapshot = read_status(&env.root);
            let (status, detail, worker_states) = match &snapshot {
                Some(snapshot) => (
                    snapshot.phase.clone(),
                    String::new(),
                    snapshot.worker_states.clone(),
                ),
                None => (String::new(), String::new(), BTreeMap::new()),
            };
            let has_token = service.has_cloudflare_token();
            let rules: Vec<TunnelRuleView> = state
                .proxy_rules
                .iter()
                .filter(|rule| rule.tunnel)
                .map(|rule| {
                    let worker = rule.resolved_tunnel_worker();
                    TunnelRuleView {
                        domain: rule.domain.clone(),
                        local_port: rule.local_port,
                        upstream_host: rule.upstream_host.clone(),
                        tunnel: rule.tunnel,
                        tunnel_auth: rule.tunnel_auth,
                        tunnel_worker: worker.clone(),
                        tunnel_domain: rule.tunnel_domain.clone(),
                        tunnel_url: rule.tunnel_url.clone(),
                        worker_state: worker_states.get(&worker).cloned().unwrap_or_default(),
                    }
                })
                .collect();
            let payload = serde_json::json!({
                "status": status,
                "detail": detail,
                "hasToken": has_token,
                "accountId": state.cloudflare_account_id,
                "subdomain": state.cloudflare_subdomain,
                "clientId": state.tunnel_client_id,
                "zones": state.cloudflare_zones,
                "dns": state.cloudflare_dns,
                "workers": state.tunnel_workers,
                "rules": rules,
            });
            crate::commands::write_result(env, out, &payload, |w| {
                writeln!(
                    w,
                    "Cloudflare Tunnel: {} (token configured: {})",
                    payload["status"].as_str().unwrap_or_default(),
                    payload["hasToken"].as_bool().unwrap_or(false)
                )?;
                let account = payload["accountId"].as_str().unwrap_or_default();
                let subdomain = payload["subdomain"].as_str().unwrap_or_default();
                if !account.is_empty() || !subdomain.is_empty() {
                    writeln!(
                        w,
                        "Account: {} | workers.dev subdomain: {}",
                        account, subdomain
                    )?;
                }
                for rule in &rules {
                    writeln!(
                        w,
                        "  - {} ({}) -> {} [{}]",
                        rule.domain, rule.tunnel_worker, rule.tunnel_url, rule.worker_state
                    )?;
                }
                Ok(())
            })
        }
        "configure" => {
            let specs = FlagSpecs::new()
                .bool("token-stdin")
                .value("account-id")
                .value("client-id");
            let ParseOutcome::Parsed(parsed) = parse_flags(rest, &specs)? else {
                return Ok(());
            };
            if let Some(extra) = parsed.positionals.first() {
                return Err(CliError::usage(format!(
                    "unexpected argument {extra:?} for tunnel configure"
                )));
            }
            let token = if parsed.flag("token-stdin") {
                crate::args::read_secret_stdin(&mut *env.input, "Cloudflare API token")?
            } else {
                service.read_cloudflare_token().map_err(|_| {
                    CliError::usage(
                        "tunnel configure requires --token-stdin when no token is stored",
                    )
                })?
            };
            service
                .save_settings(
                    &token,
                    &parsed.string("account-id"),
                    &parsed.string("client-id"),
                )
                .map_err(|error| CliError::operation(error.to_string()))?;
            let state = read_state(env)?;
            let payload = serde_json::json!({
                "configured": true,
                "hasToken": true,
                "accountId": state.cloudflare_account_id,
                "clientId": state.tunnel_client_id,
            });
            crate::commands::write_result(env, out, &payload, |w| {
                writeln!(w, "Cloudflare tunnel configured.")
            })
        }
        "remove-config" => {
            let specs = FlagSpecs::new().bool("yes").bool("delete-workers");
            let ParseOutcome::Parsed(parsed) = parse_flags(rest, &specs)? else {
                return Ok(());
            };
            require_yes(
                parsed.flag("yes"),
                "removing tunnel configuration requires --yes",
            )?;
            let delete_workers = parsed.flag("delete-workers");
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|error| CliError::operation(error.to_string()))?;
            runtime
                .block_on(service.remove_settings(delete_workers))
                .map_err(|error| CliError::operation(error.to_string()))?;
            let payload = serde_json::json!({
                "removed": true,
                "deletedWorkers": delete_workers,
            });
            crate::commands::write_result(env, out, &payload, |w| {
                writeln!(w, "Removed Cloudflare tunnel configuration.")
            })
        }
        "discover" | "refresh" | "sync" => Err(unavailable("Cloudflare discovery and Worker sync")),
        "apply" | "start" => {
            let jobs = devproxy::tunnel::lifecycle::TunnelJobs::new(
                store(env),
                Arc::new(devcore::launchd::SystemRunner::new()),
            );
            jobs.start()
                .map_err(|error| CliError::operation(error.to_string()))?;
            #[derive(Serialize)]
            struct Payload {
                applied: bool,
            }
            crate::commands::write_result(env, out, &Payload { applied: true }, |w| {
                writeln!(w, "Tunnel agent started.")
            })
        }
        "stop" => {
            let jobs = devproxy::tunnel::lifecycle::TunnelJobs::new(
                store(env),
                Arc::new(devcore::launchd::SystemRunner::new()),
            );
            jobs.stop()
                .map_err(|error| CliError::operation(error.to_string()))?;
            #[derive(Serialize)]
            struct Payload {
                stopped: bool,
            }
            crate::commands::write_result(env, out, &Payload { stopped: true }, |w| {
                writeln!(w, "Tunnel agent stopped.")
            })
        }
        "restart" => {
            let jobs = devproxy::tunnel::lifecycle::TunnelJobs::new(
                store(env),
                Arc::new(devcore::launchd::SystemRunner::new()),
            );
            jobs.restart()
                .map_err(|error| CliError::operation(error.to_string()))?;
            #[derive(Serialize)]
            struct Payload {
                restarted: bool,
            }
            crate::commands::write_result(env, out, &Payload { restarted: true }, |w| {
                writeln!(w, "Tunnel agent restarted.")
            })
        }
        "check" => {
            if rest.len() != 1 {
                return Err(CliError::usage(
                    "usage: devbase tunnel check <name-or-domain>",
                ));
            }
            Err(unavailable("live tunnel reachability checks"))
        }
        "access-token" => {
            let specs = FlagSpecs::new().bool("rotate");
            let ParseOutcome::Parsed(parsed) = parse_flags(rest, &specs)? else {
                return Ok(());
            };
            if parsed.positionals.len() != 1 {
                return Err(CliError::usage(
                    "usage: devbase tunnel access-token <name-or-domain> [--rotate]",
                ));
            }
            let domain = local_domain(&parsed.positionals[0]);
            let state = read_state(env)?;
            let rule = state
                .proxy_rules
                .iter()
                .find(|rule| rule.domain == domain && rule.tunnel)
                .ok_or_else(|| {
                    CliError::operation(format!("tunnel rule {domain} is not configured"))
                })?;
            let worker = rule.resolved_tunnel_worker();
            let secrets = service.secrets();
            let rotate = parsed.flag("rotate");
            let token = if rotate {
                secrets.rotate_tunnel_access_token(&worker)
            } else {
                secrets.ensure_tunnel_access_token(&worker)
            }
            .map_err(|error| CliError::operation(error.to_string()))?;
            let payload = serde_json::json!({
                "domain": domain,
                "token": token,
                "rotated": rotate,
            });
            crate::commands::write_result(env, out, &payload, |w| {
                writeln!(w, "{}", payload["token"].as_str().unwrap_or_default())
            })
        }
        other => Err(CliError::usage(format!(
            "unknown tunnel subcommand {other:?}"
        ))),
    }
}

// ---- ca -----------------------------------------------------------------

#[derive(Serialize)]
#[allow(non_snake_case)]
struct AuthorityInfoView {
    ID: String,
    Name: String,
    Status: String,
    Detail: String,
    Fingerprint: String,
    Trust: String,
    CertificatePath: String,
    NotBefore: String,
    NotAfter: String,
    Usable: bool,
}

#[derive(Serialize)]
#[allow(non_snake_case)]
struct AuthorityStatusView {
    CheckedAt: String,
    Active: AuthorityInfoView,
    #[serde(skip_serializing_if = "Option::is_none")]
    Prepared: Option<AuthorityInfoView>,
    #[serde(skip_serializing_if = "Option::is_none")]
    Previous: Option<AuthorityInfoView>,
    Archives: Vec<AuthorityInfoView>,
    PreparedIsActive: bool,
    Revision: String,
}

fn time_from_unix(seconds: i64) -> String {
    DateTime::<Utc>::from_timestamp(seconds, 0)
        .map(|time| time.to_rfc3339_opts(SecondsFormat::Nanos, true))
        .unwrap_or_default()
}

fn info_view(info: &AuthorityInfo) -> AuthorityInfoView {
    AuthorityInfoView {
        ID: info.id.clone(),
        Name: info.name.clone(),
        Status: info.status.clone(),
        Detail: info.detail.clone(),
        Fingerprint: info.fingerprint.clone(),
        Trust: info.trust.as_str().to_string(),
        CertificatePath: info.certificate_path.to_string_lossy().into_owned(),
        NotBefore: time_from_unix(info.not_before),
        NotAfter: time_from_unix(info.not_after),
        Usable: info.usable,
    }
}

fn status_view(status: &AuthorityStatus) -> AuthorityStatusView {
    AuthorityStatusView {
        CheckedAt: go_rfc3339_nano(&status.checked_at),
        Active: info_view(&status.active),
        Prepared: status.prepared.as_ref().map(info_view),
        Previous: status.previous.as_ref().map(info_view),
        Archives: status.archives.iter().map(info_view).collect(),
        PreparedIsActive: status.prepared_is_active,
        Revision: status.revision.clone(),
    }
}

fn find_info(status: &AuthorityStatus, id: &str) -> Option<AuthorityInfo> {
    if status.active.id == id {
        return Some(status.active.clone());
    }
    if let Some(prepared) = &status.prepared
        && prepared.id == id
    {
        return Some(prepared.clone());
    }
    if let Some(previous) = &status.previous
        && previous.id == id
    {
        return Some(previous.clone());
    }
    status.archives.iter().find(|info| info.id == id).cloned()
}

pub fn run_ca_command(env: &mut CliEnv, out: &mut dyn Write, args: &[String]) -> CliResult<()> {
    if has_help_flag(args) && args.len() == 1 {
        crate::schema::print_topic_help(out, "ca")?;
        return Ok(());
    }
    let mut rest = args;
    let mut sub = "status";
    if let Some(first) = rest.first() {
        sub = first;
        rest = &rest[1..];
    }
    let service = authority_service(env);
    match sub {
        "status" => {
            let status = service.status()?;
            let view = status_view(&status);
            crate::commands::write_result(env, out, &view, |w| {
                writeln!(
                    w,
                    "Local CA: {} ({}), trust: {}, revision {}",
                    view.Active.Name, view.Active.Status, view.Active.Trust, view.Revision
                )
            })
        }
        "trust" => {
            service.configure_active_trust(false)?;
            let status = service.status()?;
            let authority = info_view(&status.active);
            let payload = serde_json::json!({"trusted": true, "authority": authority});
            crate::commands::write_result(env, out, &payload, |w| {
                writeln!(w, "Local CA trust installed.")
            })
        }
        "untrust" => {
            let specs = FlagSpecs::new().bool("yes");
            let ParseOutcome::Parsed(parsed) = parse_flags(rest, &specs)? else {
                return Ok(());
            };
            require_yes(parsed.flag("yes"), "removing CA trust requires --yes")?;
            service.configure_active_trust(true)?;
            let status = service.status()?;
            let authority = info_view(&status.active);
            let payload = serde_json::json!({"untrusted": true, "authority": authority});
            crate::commands::write_result(env, out, &payload, |w| {
                writeln!(w, "Local CA trust removed.")
            })
        }
        "prepare" => {
            service.prepare()?;
            let status = service.status()?;
            let view = status_view(&status);
            crate::commands::write_result(env, out, &view, |w| {
                writeln!(
                    w,
                    "Prepared CA generation {} (revision {}).",
                    view.Prepared
                        .as_ref()
                        .map(|p| p.ID.clone())
                        .unwrap_or_default(),
                    view.Revision
                )
            })
        }
        "trust-prepared" | "activate" | "discard-prepared" => {
            let specs = FlagSpecs::new().value("revision");
            let ParseOutcome::Parsed(parsed) = parse_flags(rest, &specs)? else {
                return Ok(());
            };
            let mut revision = parsed.string("revision");
            if revision.is_empty() {
                revision = service.status()?.revision;
            }
            match sub {
                "trust-prepared" => service.trust_prepared(&revision)?,
                "activate" => service.activate(&revision)?,
                _ => service.discard_prepared(&revision)?,
            }
            let status = service.status()?;
            let authority = info_view(&status.active);
            let payload = serde_json::json!({
                "action": sub,
                "revision": revision,
                "authority": authority,
            });
            crate::commands::write_result(env, out, &payload, |w| {
                writeln!(w, "CA {sub} completed.")
            })
        }
        "restore" => {
            let specs = FlagSpecs::new().value("fingerprint").value("revision");
            let ParseOutcome::Parsed(parsed) = parse_flags(rest, &specs)? else {
                return Ok(());
            };
            let fingerprint = parsed.string("fingerprint");
            if parsed.positionals.len() != 1 || fingerprint.trim().is_empty() {
                return Err(CliError::usage(
                    "usage: devbase ca restore <generation-id> --fingerprint <fp> [--revision <rev>]",
                ));
            }
            let id = parsed.positionals[0].clone();
            let revision = if parsed.string("revision").is_empty() {
                service.status()?.revision
            } else {
                parsed.string("revision")
            };
            service.restore(&id, &fingerprint, &revision)?;
            let status = service.status()?;
            let authority = find_info(&status, &id).map(|info| info_view(&info));
            let payload = serde_json::json!({"restored": true, "authority": authority});
            crate::commands::write_result(env, out, &payload, |w| {
                writeln!(w, "Restored CA generation {id}.")
            })
        }
        "saved-trust" => {
            let specs = FlagSpecs::new().value("fingerprint").bool("remove");
            let ParseOutcome::Parsed(parsed) = parse_flags(rest, &specs)? else {
                return Ok(());
            };
            let fingerprint = parsed.string("fingerprint");
            if parsed.positionals.len() != 1 || fingerprint.trim().is_empty() {
                return Err(CliError::usage(
                    "usage: devbase ca saved-trust <generation-id> --fingerprint <fp> [--remove]",
                ));
            }
            let id = parsed.positionals[0].clone();
            let remove = parsed.flag("remove");
            service.configure_saved_trust(&id, &fingerprint, remove)?;
            let status = service.status()?;
            let authority = find_info(&status, &id).map(|info| info_view(&info));
            let payload = serde_json::json!({
                "id": id,
                "remove": remove,
                "authority": authority,
            });
            crate::commands::write_result(env, out, &payload, |w| {
                writeln!(w, "Updated saved CA trust.")
            })
        }
        "remove-archived-trust" => {
            let specs = FlagSpecs::new().value("fingerprint").bool("yes");
            let ParseOutcome::Parsed(parsed) = parse_flags(rest, &specs)? else {
                return Ok(());
            };
            let fingerprint = parsed.string("fingerprint");
            if parsed.positionals.len() != 1 || fingerprint.trim().is_empty() {
                return Err(CliError::usage(
                    "usage: devbase ca remove-archived-trust <archive-id> --fingerprint <fp> --yes",
                ));
            }
            require_yes(parsed.flag("yes"), "removing archived trust requires --yes")?;
            let id = parsed.positionals[0].clone();
            service.remove_archived_trust(&id, &fingerprint)?;
            let status = service.status()?;
            let authority = find_info(&status, &id).map(|info| info_view(&info));
            let payload = serde_json::json!({"removed": true, "authority": authority});
            crate::commands::write_result(env, out, &payload, |w| {
                writeln!(w, "Removed archived CA trust.")
            })
        }
        other => Err(CliError::usage(format!("unknown ca subcommand {other:?}"))),
    }
}
