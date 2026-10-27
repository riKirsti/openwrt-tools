# OpenWrt Proton VPN client

`protun-openwrt` exposes Protun 3.0.5's WireGuard UDP, WireGuard TCP and Stealth
transports, automatic candidate selection, multiple servers, and integrated
Local Agent settings. Stealth is the default selection, not a build restriction.
The APK installs one executable plus the OpenWrt service and configuration. It
does not install `libprotun.so` or require the separate `local-agent-rs` APK.

The service is disabled by default. No installation script rewrites network,
DNS, firewall, PBR, or ZeroTier settings. The existing travel/home ZeroTier
connection retains its role and configuration.

## Protocol selection

Use `protocol` in `/etc/protun/client.json`:

| Value | Candidate transports supplied to the engine |
| --- | --- |
| `stealth` (default) | TLS ports only |
| `wireguard-udp` | UDP ports only |
| `wireguard-tcp` | Plain TCP ports only |
| `auto` | All confirmed ports you configured |

Configure `udp_ports`, `tcp_ports`, and `tls_ports` as arrays of actual server
ports. The old single `tls_port` field remains supported for existing r2/r3
configurations. No UDP/TCP ports are guessed. A selected mode without usable
ports fails validation. Automatic mode permits ordinary WireGuard: select
Stealth explicitly when obfuscation is required. Selection/failover order is
owned by Protun; this client does not claim that auto always prefers Stealth.

For multiple servers, use a `peers` array instead of the single-peer fields:

```json
"peers": [
  {
    "id": "primary",
    "server_ip": "192.0.2.1",
    "server_public_key": "REPLACE_WITH_ACTUAL_KEY",
    "udp_ports": [],
    "tcp_ports": [],
    "tls_ports": [443],
    "priority": 0,
    "server_name": "REPLACE_WITH_ACTUAL_SERVER_CERTIFICATE_NAME"
  }
]
```

The addresses and keys in examples are placeholders, not working endpoints.
Confirm each protocol's availability and ports using the server's real metadata.
Peers require unique IDs; their priorities pass through to Protun unchanged.
Optional `exit_label` also passes through. `sni_strategy` accepts `random`
(default) or `top`, matching the upstream API. Inputs are bounded to 32 peers,
64 total port entries per peer, and 16 KiB per configuration file.

## Authentication choices

**Existing private key:** start from `/etc/protun/client.json.example`. Provide
the exported Proton WireGuard private key as one base64 line in
`/etc/protun/private.key`. This mode does not establish a Local Agent session.
It does not require the Proton account password.

**Account and Local Agent:** start from `/etc/protun/client-agent.json.example`.
Remove `private_key_file`; the engine manages its certificate and private key
through the private session cache. Each peer requires the correct `server_name`
for Local Agent TLS verification. The cache's parent directory must exist.

`app_version` and `login_app_version` are qualified Proton API app-version
identifiers. The example `linux-vpn-openwrt@3.0.5` identifies this custom client,
not an official Proton app. Local format validation does not prove backend
acceptance; an actual login is required to establish that Proton accepts the
configured identifier and session-fork flow. Do not invent a successful result
from a build or substitute an official app identity without understanding it.

With the service stopped, run:

```sh
/etc/init.d/protun stop
protun-openwrt --check-config /etc/protun/client.json
protun-openwrt --login /etc/protun/client.json
protun-openwrt --probe /etc/protun/client.json
```

`--login` prompts on the controlling terminal for the Proton account username,
password, and an authenticator code if TOTP is required. Password/code input is
hidden and is not accepted in command-line arguments or stored in configuration.
It uses Muon's production SRP/TOTP and session-fork APIs, with production API TLS
verification/pins. The development `test_utils` login helper is not used.

Direct hardware-security-key/FIDO2 interaction and CAPTCHA UI are not implemented.
For an existing authenticated session, an integration can supply a session fork:

```sh
protun-openwrt --import-session /etc/protun/client.json /root/fork.json
```

The private input file must contain `{"selector":"...","cookies":[{"name":"...","value":"..."}]}`.
The selector must be a real, unconsumed Proton session fork for the configured
VPN app; this is not an arbitrary access token or a WireGuard private key. The
import interface does not itself implement a browser/FIDO2 authentication flow.
Treat the import file as a credential and remove your copy when no longer needed.

The cache directory is owned by the running user (root on the router), mode
0700. Cached credentials are regular, nonsymlinked 0600 files, written atomically
and synced. A fork is consumed once. Protun persists its API session, certificate
and private key, and handles refresh through its integrated Local Agent mode.
Storage errors, invalid refresh tokens or fatal certificate errors are reported;
the command does not silently retry passwords or replace credentials with guesses.
Stop the service before login, import or cache clearing to avoid concurrent writers.

```sh
protun-openwrt --clear-session /etc/protun/client.json
```

This removes local cached credentials only. Revoke server-side sessions through
your Proton account. The account password is not needed for routine reconnects
while the cached session remains valid. The router needs a correct system clock
and working DNS for API TLS connections; certificate verification is not disabled.

## Local Agent settings

Configure `local_agent.settings` and restart the service after changing it:

| Field | Values |
| --- | --- |
| `netshield_level` | `0` off, `1` malware, `2` malware/ads/trackers, `3` adds adult filtering |
| `split_tcp` | Boolean or null |
| `soft_jail` | Boolean or null |
| `port_forwarding` | Boolean or null |
| `random_nat` | Boolean or null |
| `circumvention_routing` | Boolean or null |

Null leaves the choice to the server. All six fields pass through to the upstream
settings structure. The server/account policy determines which settings are
accepted; logs show applied settings and report policy refusals. Optional
`certificate_duration_days` requests a positive certificate lifetime; the API
decides whether to accept it. Requesting port forwarding does not configure an
OpenWrt DNAT rule, renew a NAT-PMP lease, or prove an external port is reachable.
NetShield DNS behavior still depends on using the intended Proton DNS route.

## OpenWrt network integration

Before switching traffic, back up the current network, firewall, PBR and DNS
configuration and retain the working WireGuard configuration for rollback.
If upgrading from the old `protun` package name, back up `/etc/protun` and
`/etc/config/protun`, stop the service, and remove the old package before
installing `protun-openwrt` to avoid overlapping ownership. The service and UCI
configuration retain the name `protun`.

The physical kernel uplink name belongs in JSON `uplink_device`. The logical
UCI uplink name belongs in `/etc/config/protun`'s `uplink_interface` (default
`wan`), used for restart triggers. The transport sockets, including direct
login sockets, are bound to that physical uplink. Optional numeric `socket_mark`
can match an existing transport-routing policy. Binding is not a replacement
for reachable WAN routes under every policy rule; inspect existing endpoint
routes and marks rather than adding global rules blindly.

`tun_name` must be unused. The client exclusively creates a nonpersistent TUN
device and cannot attach to an existing WireGuard/ZeroTier device. netifd owns
its addresses, MTU, link state and routing. Replace the existing Proton logical
interface's WireGuard backend with this TUN using netifd's `static` protocol;
preserve the logical interface name and its intended firewall/PBR references.
Use the actual tunnel addresses and MTU from your Proton configuration.

WireGuard's `allowed_ips`/`route_allowed_ips` do not apply to a `static` interface.
Reproduce the existing effective routes explicitly, including the existing
ZeroTier/home exceptions and IPv6 policy. Preserve DNS, forwarding/NAT and
kill-switch policy. This executable does not install its own firewall kill
switch; verify the existing policy covers the new TUN and its disappearance.
No home-router or ZeroTier controller changes are part of this migration.

After the probe and network-policy checks succeed:

```sh
uci set protun.main.enabled='1'
uci commit protun
/etc/init.d/protun enable
/etc/init.d/protun start
logread -e protun
```

Supervision uses delayed, bounded respawns. Uplink changes restart the process;
update the JSON if the physical device name changes. Rollback means stopping
and disabling this service and restoring the prior Proton/WireGuard interface,
without modifying ZeroTier or the home router.

## Verification and limits

`--capabilities` lists implemented interfaces. `--check-config` validates input
and reports the actual filtered candidate counts without connecting or creating
session files. `--probe` creates no TUN device and changes no network policy. It
attempts a real connection and reports the actual selected protocol; in Local
Agent mode, success requires both the VPN and Local Agent connection. It may
consume a fork or refresh/write cached credentials. It does not prove LAN
forwarding, DNS, external port forwarding or ZeroTier reachability.

CI builds and runs ARM64 startup/configuration checks plus unit tests for all
protocol selections, missing ports, Local Agent setting mapping, multiple-peer
validation, private cache storage and one-use fork consumption. These are
offline tests, not authenticated Proton network tests.

On the router, verify the intended exit IP from a LAN client, DNS and IPv6,
bidirectional home access through ZeroTier, the ZeroTier transport path, and
behavior when the client stops. Test WAN changes, reboot, MTU-sensitive traffic
and extended operation. Compare throughput, CPU and memory against WireGuard;
userspace operation does not imply hardware offload or a speed improvement.

This is a custom client integration. It does not add OpenVPN, IKEv2, future
post-quantum protocols, or every feature of Proton's desktop/mobile applications.
Enabling an interface in this build is distinct from verifying your account,
server and router can use it.

## Upstream references

- [Protocol enum](https://github.com/ProtonVPN/protun/blob/v3.0.5/src/api/state.rs).
- [Connection modes, candidate peers and session-fork API](https://github.com/ProtonVPN/protun/blob/v3.0.5/src/api/connection.rs).
- [Unix TUN/socket-protection API](https://github.com/ProtonVPN/protun/blob/v3.0.5/src/api/connection_unix.rs).
- [All Local Agent settings](https://github.com/ProtonVPN/protun/blob/v3.0.5/src/api/local_agent.rs).
- [Real-connection upstream tests](https://github.com/ProtonVPN/protun/blob/v3.0.5/src/api/tests/real_connection_tests.rs).
- [Proton protocol-codebase announcement](https://protonvpn.com/blog/introducing-proton-protocols).
