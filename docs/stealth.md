# OpenWrt Stealth client

Purpose: replace the travel router's Proton WireGuard connection with Stealth
while retaining the existing ZeroTier connection to the home router and the
same traffic-selection policy. Nothing in this package configures ZeroTier.

The `protun` APK includes the library, `protun-openwrt`, and an OpenWrt `procd`
service. The service is disabled by default. No installation script rewrites
`/etc/config/network`, firewall, DNS, PBR, or ZeroTier configuration.

## Verified upstream interfaces and remaining uncertainty

- [Protun 3.0.5 Stealth test](https://github.com/ProtonVPN/protun/blob/v3.0.5/src/api/tests/real_connection_tests.rs#L68-L98):
  empty UDP/plain-TCP candidate lists, TLS port 443, and a `Protocol::Stealth`
  assertion. Our client uses that selection and rejects any other protocol.
- [Unix constructor and socket protection](https://github.com/ProtonVPN/protun/blob/v3.0.5/src/api/connection_unix.rs):
  the caller supplies a TUN descriptor and a callback that protects transport
  sockets. We use `SO_BINDTODEVICE` and an optional existing-policy socket mark;
  failure returns `false`, not an unprotected connection.
- [Connection modes](https://github.com/ProtonVPN/protun/blob/v3.0.5/src/api/connection.rs):
  `NoLocalAgent` accepts a WireGuard private key. The upstream tests exercise
  Stealth and NoLocalAgent separately. Their combination with your exported
  credentials and chosen server **requires a live handshake test**. A successful
  build, config check, or library load does not establish this.
- [OpenWrt 25.12.6 procd implementation](https://github.com/openwrt/openwrt/blob/v25.12.6/package/system/procd/files/procd.sh):
  service supervision, bounded respawn, file tracking and uplink restart triggers
  use OpenWrt's existing API.

This is a custom OpenWrt integration, not an official Proton-supported router
client. No router throughput or authenticated VPN result is claimed by this
source change. It has no account login, certificate renewal or Local Agent
settings interface. The separate `local-agent-rs` library is not required by the
new command. Its existing APK is not a complete runnable Local Agent client.

## Configure and probe before switching traffic

1. Keep a backup of the current network, firewall, PBR and DNS configuration,
   and keep the working WireGuard setup available for rollback. Leave ZeroTier
   and the home router unchanged.
2. Install the newly built `protun` APK. Copy
   `/etc/protun/client.json.example` to `/etc/protun/client.json` and replace the
   placeholders. The example endpoint `192.0.2.1` is documentation-only.
3. Put the **Proton WireGuard private key**, as one base64 line, in
   `/etc/protun/private.key`; do not use the account password or commit secrets.
   On the router both files must be root-owned. Use mode `0600` for the key and
   configuration. Symlinked files and group/world-writable configuration are
   rejected; keys readable by other users are rejected.
4. Set `server_ip`, `server_public_key` and `tls_port` from a confirmed Proton
   Stealth endpoint. Do not assume that an arbitrary WireGuard server accepts
   TLS merely because its endpoint port is changed to 443. Set `uplink_device`
   to the actual physical uplink device (such as the current WAN's `l3_device`),
   not a logical UCI name or the ZeroTier device. `tun_name` must be unused;
   the client refuses to take over an existing device.
5. Run:

   ```sh
   protun-openwrt --check-config /etc/protun/client.json
   protun-openwrt --probe /etc/protun/client.json
   ```

   `--check-config` validates input and key-file access without contacting a
   server. `--probe` creates no TUN interface and changes no network policy;
   it attempts a real Stealth/WireGuard handshake, reports
   `connected protocol=Stealth`, then disconnects. A timeout, protocol mismatch
   or interruption fails. A probe does not test Internet forwarding or DNS.
   If it fails, retain WireGuard and investigate credentials, server capability,
   uplink selection and existing policy before migrating.

Optional JSON field `socket_mark` is a numeric mark for the **outer** transport
socket. Omit it unless an existing policy requires one. Binding a socket to a
device does not override every policy-routing rule; ensure the Proton endpoint
still has a reachable path through the physical uplink using the existing WAN
route or a narrowly scoped endpoint route. The client does not invent routing
tables, priorities or marks.

## Replace only the Proton network interface

After the handshake succeeds, adapt the existing Proton logical interface to
use the new TUN device with OpenWrt's `static` protocol. Preserve its logical
interface name so existing firewall-zone and PBR references can continue to use
it. Copy its actual assigned tunnel addresses and appropriate MTU from the
current Proton configuration; a TUN device's name alone does not assign them.

WireGuard-specific peer options, including `allowed_ips` and `route_allowed_ips`,
are not processed by `static`. Reproduce the existing effective routes and
traffic-selection rules explicitly with netifd/PBR, including IPv6 if used.
Do not create a new blanket default route or change ZeroTier's managed routes.
Keep the existing DNS selection, forwarding/NAT and kill-switch policy attached
to the Proton interface. The client itself does not provide a firewall kill
switch; verify the existing rules also cover its new device and the absence of
that device. Do not assume a former WireGuard-device rule automatically follows.

The daemon creates a nonpersistent TUN descriptor; netifd owns addressing, link
state, MTU and routing when the device appears. It removes that new device when
the process closes. It cannot attach to an existing WireGuard or ZeroTier device.

Set `/etc/config/protun`'s `uplink_interface` to the current WAN's logical UCI
interface (default `wan`) so network changes restart the client. This is distinct
from JSON `uplink_device`, which is a kernel device name. Then enable the client
and service:

```sh
uci set protun.main.enabled='1'
uci commit protun
/etc/init.d/protun enable
/etc/init.d/protun start
logread -e protun
```

Supervision restarts failures with a delay and stops rapid restart loops.
Transport reconnects use Protun's own connection logic. If the uplink's kernel
device name changes, update the JSON before restarting. Only apply the network
migration through a management path that remains available for rollback.

## Acceptance checks on the travel router

- Logs report `protocol=Stealth`, and a real LAN client can reach the Internet
  through the intended Proton exit. Capture the outer connection if needed to
  verify the configured endpoint/port; the log alone does not prove forwarding.
- ZeroTier peer status and bidirectional access to the home LAN match the
  baseline. Check its actual transport path too: the client leaves policy to
  OpenWrt, so preserving that path depends on retaining the existing exceptions.
- LAN DNS and IPv6 follow the intended existing policy. Stop the client and
  check that protected traffic cannot fall back to WAN under the existing
  firewall rules, while the previously permitted management path still works.
- Test WAN reconnects, router reboot, service stop/start, MTU-sensitive traffic
  and extended operation. Compare measured throughput, CPU and memory with
  WireGuard; userspace Stealth is not assumed to be faster or hardware-offloaded.

Rollback: disable/stop `protun`, restore the saved Proton network configuration,
and re-enable the previous WireGuard interface. Do not modify ZeroTier or the
home router as part of rollback.
