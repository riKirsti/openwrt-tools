# Optional controller integration

`protun-openwrt` includes optional scripts for compatible exit-node controllers.
There is no separate integration package. The standard client supports WireGuard
UDP, WireGuard TCP, Stealth, and automatic selection; `protocol` selects the
transport, with Stealth as its default. The optional controller adapter supports
explicit selection of WireGuard UDP, WireGuard TCP, and Stealth with the ProtunNext
client.
It is independent of router brand and deployment role. It is not a standalone
network manager: the host controller must provide `/usr/lib/exit-node-common`,
its locking and transaction functions, the expected runtime state, and its
credential lifecycle. Installation does not rewrite policies or select a backend. Existing controller
commands and runtime paths are retained for compatibility. The integration service
is controller-requested only and its boot handler does nothing; it does not replace
the normal, disabled-by-default `protun` service. Enable only one owner of a given
tunnel: use either the standard service or the controller-managed service.
Device-specific controller patches, configuration snapshots, and source fingerprints
are intentionally excluded from this repository and package. Integrators must
review their controller's compatibility before enabling this adapter.

The optional helpers require the controller's existing HTTPS-capable `curl`,
`ip-full`, `flock`, `ubus`, and libubox JSON shell functions. They do not add these
controller-only prerequisites to the standalone client's dependency set. Verify
those tools before activating integration; keep HTTPS certificate checking enabled.

The legacy `wireguard` preference field remains the compatible VPN on/off switch.
Set `exitnode.main.proton_backend=protun` to use the client for all three transports.
A controller dedicated to ProtunNext may set `PROTON_CONTROLLER_BACKEND=protun` before
sourcing the adapter. Both methods read the same `exitnode.main.proton_protocol`
preference. It accepts `stealth` (default), `wireguard-udp`, and `wireguard-tcp`.
The runtime status reports the negotiated protocol. The adapter supplies only the
selected transport and preserves the controller's fail-closed policy. The existing
`wireguard` and `stealth` backends remain available for compatible deployments.

The adapter retains the existing controller's locks, transaction/credential ownership,
tables 600/610/620, physical transport mark 0x621, DNS selection and ZeroTier binding.
Procd supervises the new process with 30-second unlimited retries; the existing
controller reconciles policy after TUN recreation. Transport health checks also
restore the dynamic netifd interface, addresses, and transport routes when procd
recreates a TUN. This repair does not change LAN selectors or the selected Internet
mode. HTTPS retries have an explicit total retry-time limit. The new service does not start a
VPN at boot: the existing Direct-on-boot behavior remains authoritative.

The wrapper creates the exclusive nonpersistent TUN, applies `tun_mtu`, and brings
the link up before passing it to the protocol engine. The managed `protun` backend applies the tunnel
addresses and link state directly on `pt-proton` after the worker creates it. The
legacy Stealth backend uses netifd's dynamic static interface. Use `pt-proton` as
the firewall zone device when migrating forwarding from kernel WireGuard.

Optional `status_file` and `status_instance` enable a private atomic heartbeat. Status
has PID, instance, boot ID, monotonic age, actual protocol and TUN state; it contains
no account/key data. Host readiness also checks the live process, interface and real
IPv4/IPv6 HTTPS. A Connected event is not proof of internet/DNS forwarding.

Transport ports come from Proton's public `/vpn/v2/clientconfig` endpoint, using the
same working API app-version identifier as the existing account helper. No account
credentials are read/sent for this request. HTTPS certificate verification remains
enabled. Last validated port metadata is retained when the endpoint is unavailable;
the actual handshake must still authenticate the configured peer. The package's
`protun-openwrt --account` commands let controllers reuse the same account client
for server discovery and credential operations.

Before activation, take private backups and validate the source hashes, shell syntax,
all generated policies, and the binary on the actual device. Start from Direct,
then validate each protocol through the controller, including credential refresh
and restoration after a failed transition. For an existing controller migration,
keep its last working package and configuration available for rollback.

Official references reviewed:

- [Proton's Unix TUN and socket-protection API](https://github.com/ProtonVPN/protun/blob/v3.0.5/src/api/connection_unix.rs)
- [Proton connection states and actual protocol](https://github.com/ProtonVPN/protun/blob/v3.0.5/src/api/state.rs)
- [Proton real-connection examples](https://github.com/ProtonVPN/protun/blob/v3.0.5/src/api/tests/real_connection_tests.rs)
- [OpenWrt dynamic-interface implementation](https://github.com/openwrt/netifd/blob/06fc904585606ed5a1dfca233ecaa6be9a5ca599/ubus.c)
- [OpenWrt supervision/retry implementation](https://github.com/openwrt/procd/blob/a9afc12dcd4e1f86ef08eb6744b9a6359bd78ed8/service/instance.c)
- [Linux TUN lifecycle](https://docs.kernel.org/networking/tuntap.html)

This integration manages VPN software and leaves firmware management to the system administrator.
