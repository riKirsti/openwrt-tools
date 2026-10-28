# Optional controller integration

`protun-openwrt` provides one optional controller-managed tunnel lifecycle for
Stealth, WireGuard UDP and WireGuard TCP. It introduces no additional package,
protocol dropdown, watchdog or automatic protocol fallback. Existing C++ helpers
and the account/session implementation are unchanged.

The host supplies `/usr/lib/exit-node-common`, transactions, firewall and routing
policy, DNS, preferences and credential ownership. Host scripts and private
configuration are not distributed in this package.

## Compatibility contract

- Existing `proton_backend=stealth` selects Stealth; `wireguard` selects Protun
  WireGuard UDP. `wireguard-tcp` is also supported by the adapter. An absent
  backend defaults to Stealth. An existing `proton_protocol` value is respected;
  invalid values fail closed rather than falling back.
- `vpn_context` exports backend `protun`, interface `pt-proton` and the requested
  protocol. Host reconciliation, worker health, health-cache identity and overlay
  binding must use this context. Existing backend commands must clear a conflicting
  protocol override transactionally. Existing VPN on/off preferences are retained.
- `proton-tunnel` starts, stops and tests the shared `proton-client` procd service.
  Compatibility `proton-stealth` entry points forward to it. Service boot does
  nothing; only the controller requests a tunnel. The separate standalone `protun`
  service remains disabled. Do not enable two owners of the same tunnel.
- Install fail-closed policy before preparing a tunnel. Pass only the controller's
  `active.conf` or `pending.conf`. Preserve the existing account and certificate
  ownership contract. This adapter never generates or revokes credentials itself.
- Read readiness and the negotiated protocol from the client's fresh, matching
  process/boot/instance status. Port 443 does not distinguish TCP and Stealth.
  Recheck real IPv4/IPv6 HTTPS before publishing health.

## Network lifecycle

Retain table 620, physical socket mark 0x621, policy mark 0x620, MTU 1280 and the
existing dual-stack address contract. The client owns its nonpersistent TUN;
netifd owns its addresses. Wait for the client before adding the dynamic logical
interface, and restore addresses after supervised restarts. Do not restart WAN
or the network manager to change a VPN transport.

The public `/vpn/v2/clientconfig` response supplies UDP, TCP and TLS port
candidates. Schema version 2 validates all three lists before atomic replacement;
failed requests retain the last valid cache. These are global candidates, not
per-server capability claims. Actual authenticated connections establish support.

Before migrating a host, retain its package/configuration privately and test all
existing modes, protocols, server preferences, dual-stack forwarding, DNS,
failure protection, service restarts and recovery. Compare native UDP against
kernel WireGuard before removing the old controller helper. General WireGuard
packages may remain installed; both the tunnel and overlay still need `kmod-tun`.

## Sources

- [Proton Unix connection API](https://github.com/ProtonVPN/protun/blob/v3.0.5/src/api/connection_unix.rs)
- [Proton real connection examples](https://github.com/ProtonVPN/protun/blob/v3.0.5/src/api/tests/real_connection_tests.rs)
- [Proton connection status](https://github.com/ProtonVPN/protun/blob/v3.0.5/src/api/state.rs)
- [OpenWrt process supervision](https://github.com/openwrt/procd/blob/master/service/instance.c)
- [OpenWrt dynamic interfaces](https://github.com/openwrt/netifd/blob/master/ubus.c)
