# devproxy

Phase 3 of the nsh migration: the three process-level subsystems that used to
live in Go's `internal/devplatform`.

| Module | Go source | Responsibility |
|---|---|---|
| `proxy::rules` | `proxy_rules.go` | rule wrappers, `proxy/rules.json` schema 1, publication state, fingerprints |
| `proxy::routes` | `proxy_routes.go` | live routing table watched by SHA-256, last-good-on-corruption |
| `proxy::forward` | `proxy_routes.go` | reverse forwarding, `101` upgrade tunneling, immediate streaming, X-Forwarded policy, failure pages |
| `proxy::server` | `proxy.go`, `network.go` | per-port rustls listeners, SNI resolver, Host/SNI `421`, socket-activation FFI |
| `proxy::lifecycle` | `proxy.go`, `network.go`, `ports.go` | port leases/preflight, launchd job, TLS verification, rollback, stop/restart |
| `proxy::activation` | `proxy_activation.go` | revision/fingerprint-checked HTTPS and tunnel observations |
| `ca::material` | `authority.go`, `proxy.go` | ECDSA P-256 CA/leaf generation, parsing, status classification, digests |
| `ca::store` | `authority.go` | `ca-generations/<id>/`, pointer symlinks, public aliases, `ca.lock` |
| `ca::trust` | `authority.go` | `/usr/bin/security` CLI (`add-trusted-cert`, `verify-cert`, `remove-trusted-cert`, `delete-certificate -Z`) |
| `ca::service` | `authority.go` | prepare/trust/activate/discard/restore, revision checks, proxy-stop gate |
| `secrets` | `tunnel_config.go` | `cloudflare.key`, `tunnel.key`, `tunnel-access.json`, 0600 atomic writes, access-map lock |
| `tunnel::config` | `tunnel_config.go` | `tunnel.json` schema 1, status snapshot, route validation, sync detection |
| `tunnel::proto` | `tunnel_proto.go` | 9-byte frames, JSON payloads, HMAC-SHA256 handshake |
| `tunnel::ws` | `ws.go` | manual RFC 6455 client: upgrade, masking, fragments, ping/pong, close |
| `tunnel::session` | `tunnel_session.go` | 256-stream HTTP/WS multiplexer, body buffer, forwarded headers |
| `tunnel::client` | `tunnel_client.go` | supervisor, backoff, status aggregation, hot reload |
| `tunnel::cloudflare` | `cloudflare_api.go`, `tunnel_worker_template.go` | API client, Worker multipart deploy, DO migration, byte-frozen `worker.mjs` |
| `tunnel::service` | `cloudflare_api.go` | settings save/remove, metadata refresh, `SyncTunnels` |
| `tunnel::lifecycle` | `tunnel_client.go` | tunnel launchd job, apply/start/stop/restart |
| `helpers` | `network.go`, `helper_bundle.go` | private `helpers/<name>` executable snapshots |
| `bin/devproxy-svc` | hidden modes | `serve <root>` and `tunnel <root>` |

## Seam traits (everything OS-facing is injected)

| Trait | Production | Used by |
|---|---|---|
| `devcore::launchd::ProcessRunner` | `SystemRunner` | CA trust CLI, launchctl reads, tunnel status |
| `ca::TrustStore` | `SecurityTrust` | authority lifecycle tests assert exact `security` argv |
| `ca::ProxyStopCheck` | job-state check in the caller | activate/restore |
| `proxy::lifecycle::PortProbe` | `SystemPortProbe` | preflight |
| `proxy::lifecycle::HealthProbe` | `SystemHealthProbe` | readiness |
| `proxy::lifecycle::TlsVerifier` / `TlsVerifierFactory` | `SystemTlsVerifier` | post-start CA-only handshake |
| `proxy::lifecycle::ShutdownVerifier` | `EngineShutdownVerifier` | empty-rule and authority gates |
| `proxy::lifecycle::SocketInstaller` | `Phase4SocketInstaller` (unsupported) | privileged ports |
| `proxy::lifecycle::ExecutableSource` | `CurrentExecutable` | helper snapshot source |
| `proxy::server::SocketActivation` | `SystemSocketActivation` (`launch_activate_socket`) | launchd-bound listeners |
| `proxy::server::CertificateAuthority` | `AuthorityService` | SNI leaf signing |
| `tunnel::service::TunnelJobControl` | `tunnel::lifecycle::TunnelJobs` | remove settings |

HTTP with Cloudflare is isolated by the injected API base URL; tests point it
at a local fake server. TLS probes use the system root store (never the app
CA), so an untrusted local CA still surfaces as `TLS`.

## Phase 4 must wire

1. **Privileged socket daemon.** `Phase4SocketInstaller` stops at
   `Unsupported`; Phase 4 installs `/Library/LaunchDaemons/<label>.plist`
   through the privileged helper (`proxy_socket/install`), with the same
   `Sockets` dictionary (`HTTPS-<port>`, 127.0.0.1, IPv4, stream/TCP) and
   `MSM_DEV_SOCKET_ACTIVATION=1`. Stop/kickstart also go through the helper;
   `SystemSocketActivation` already consumes the fds.
2. **Packaged helper bundles.** `helpers::helper_executable` writes the flat
   `helpers/<name>` copy; Phase 4 adds the digest-addressed
   `helpers/bundles/<sha256>/…` layout used inside signed app bundles.
3. **Log maintenance and diagnostics.** The UI/CLI calls
   `Store::init`, `TunnelService`, and `devcore::diagnostics` status keys; the
   sweep/rotation job is Phase 4.
4. **CLI command surface.** `devproxy-svc` intentionally implements only the
   hidden `serve`/`tunnel` modes. The public verbs (`apply-proxy`,
   `apply-tunnel`, authority actions, Cloudflare settings) belong to the app
   integration.

## Leaf certificate policy

A fresh P-256 leaf is generated for every TLS handshake from the current
`ca-current` generation, exactly like Go's `GetCertificate`. Nothing is cached
across generations, so activating a replacement CA takes effect on the next
connection; the only cost is one ECDSA signature per handshake.

## Invariants

- `/etc/hosts` is never touched; rule domains are `<name>.devbase.localhost`.
- The Cloudflare token lives only in `proxy/cloudflare.key` (0600) and is
  never serialized into `state.json` or a snapshot.
- The Worker module `tunnel/worker.mjs` is byte-frozen: its digest includes
  `2026-04-07` and is stored in `TunnelWorkerState::script_digest`.
- Tests never use the network, keychain, or `launchctl`: fakes record argv and
  local fake HTTP servers serve Cloudflare envelopes.
