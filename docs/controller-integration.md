# Optional controller integration

`protun-openwrt` includes optional scripts for compatible exit-node controllers.
There is no separate integration package. The standard client supports WireGuard
UDP, WireGuard TCP, Stealth, and automatic selection; `protocol` selects the
transport, with Stealth as its default. The optional controller adapter currently
selects either the existing kernel WireGuard backend or the Stealth client backend.
It does not yet expose client TCP/automatic selection in the controller UI.
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
The new `exitnode.main.proton_backend` chooses `wireguard` or `stealth`; the runtime
status exposes `backend`, `protocol`, and `vpn_ready` while retaining `wg_ready` for
the current UI. Selecting Stealth supplies TLS candidates only. There is no automatic
fallback to UDP/plain TCP or Direct.

The adapter retains the existing controller's locks, transaction/credential ownership,
tables 600/610/620, physical transport mark 0x621, DNS selection and ZeroTier binding.
Procd supervises the new process with 30-second unlimited retries; the existing
controller reconciles policy after TUN recreation. Transport health checks also
restore the dynamic netifd interface, addresses, and transport routes when procd
recreates a TUN; netifd removes a dynamic interface when its previous device
disappears. This repair does not change LAN selectors or the selected Internet
mode. HTTPS retries have an explicit total retry-time limit. The new service does not start a
VPN at boot: the existing Direct-on-boot behavior remains authoritative.

The wrapper creates the exclusive nonpersistent TUN and applies `tun_mtu` through
Linux SIOCSIFMTU before connecting. Netifd owns addresses and link state through its
`network.add_dynamic` static-interface API. Dynamic logical-interface MTU did not
apply on the tested OpenWrt version, so it is not relied on.

Optional `status_file` and `status_instance` enable a private atomic heartbeat. Status
has PID, instance, boot ID, monotonic age, actual protocol and TUN state; it contains
no account/key data. Host readiness also checks the live process, interface and real
IPv4/IPv6 HTTPS. A Connected event is not proof of internet/DNS forwarding.

Transport ports come from Proton's public `/vpn/v2/clientconfig` endpoint, using the
same working API app-version identifier as the existing account helper. No account
credentials are read/sent for this request. HTTPS certificate verification remains
enabled. Last validated port metadata is retained when the endpoint is unavailable;
the actual Stealth handshake must still authenticate the configured peer. Local Agent
account migration is not part of this change: the example custom app-version value
was rejected by the live API during review.

Before activation, take private backups and validate the source hashes, shell syntax,
all generated policies, and the binary on the actual device. Keep the backend set to
WireGuard while deploying the compatible controller changes. Then use the controller's
`backend stealth` command in Direct and test all five modes. `backend wireguard` is
the deliberate rollback path, subject to the controller's normal transaction checks.

Official references reviewed:

- [Proton's Unix TUN and socket-protection API](https://github.com/ProtonVPN/protun/blob/v3.0.5/src/api/connection_unix.rs)
- [Proton connection states and actual protocol](https://github.com/ProtonVPN/protun/blob/v3.0.5/src/api/state.rs)
- [Proton real-connection examples](https://github.com/ProtonVPN/protun/blob/v3.0.5/src/api/tests/real_connection_tests.rs)
- [OpenWrt dynamic-interface implementation](https://github.com/openwrt/netifd/blob/06fc904585606ed5a1dfca233ecaa6be9a5ca599/ubus.c)
- [OpenWrt supervision/retry implementation](https://github.com/openwrt/procd/blob/a9afc12dcd4e1f86ef08eb6744b9a6359bd78ed8/service/instance.c)
- [Linux TUN lifecycle](https://docs.kernel.org/networking/tuntap.html)

This integration manages VPN software and leaves firmware management to the system administrator.
