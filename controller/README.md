# Proton server selection helpers

These are the existing controller's catalog/credential and server-selection programs. They are compiled into the existing controller executables; they do not add a router package or replace `protun-openwrt`.

## Build and test

With Go 1.27.2, run `go test ./...` separately in `catalog` and `node-state`. Build for OpenWrt ARM64 with `CGO_ENABLED=0 GOOS=linux GOARCH=arm64 go build -trimpath -ldflags='-s -w'` in each directory. Install the catalog executable as `/usr/libexec/protonvpn-wg-confgen` and the selection executable as `/usr/libexec/proton-node-state`. Deployment must use the controller's existing transaction mutex and preserve authorization, preferences and rollback copies.

## Protocol eligibility

`--protocol-details` enriches the existing v1 catalog from `/vpn/v2/logicals?WithEntriesForProtocols=WireGuardUDP,WireGuardTCP,WireGuardTLS&WithState=true`. Physical server IDs join the results without replacing v1 scores or loads. Optional metadata failure retains the legacy catalog.

`PROTON_PROTOCOL=stealth` selects `WireGuardTLS`; `wireguard-udp` and `wireguard-tcp` map to `WireGuardUDP` and `WireGuardTCP`. The default remains `wireguard-udp` for compatibility. A protocol-specific address overrides `EntryIP`; missing protocol entries use `EntryIP`, following Proton's client behavior. Protocol-specific ports override defaults independently of address fallback. Online status, a valid address and a 32-byte X25519 public key establish eligibility. The existing Free-tier and IPv6 filters remain in force. The current TCP/TLS router transport requires an IPv4 outer endpoint.

The catalog's `--transport` option uses the selected protocol when generating credentials. Generated files carry `TransportProtocol` and optional `TransportPorts` comments; machine-readable credential receipts also identify the protocol. A controller must keep `pending.protocol`/`active.protocol` with its existing credential metadata and include them in transaction backup/restore. A credential generated for one transport must not silently be reused as another transport's endpoint. `protun-openwrt` r10 reads explicit Stealth ports, or uses the existing validated global TLS-port metadata when none are supplied.

## Connection observations

The optional integration helper `integration/proton-server-observe` records success only after the existing authenticated Stealth connection and dual-stack HTTPS checks pass for the same live engine instance. It never infers success from latency. Install it as `/usr/libexec/proton-server-observe` in controllers providing `/usr/lib/exit-node-common` and the existing runtime contract. The worker may call its `health` command instead of `proton-tunnel health`; network checks occur before acquiring the transaction mutex. Transaction callers already holding that mutex call `success` or `failure` with network ID, server name and prepared credential path. Failures are recorded only after an actual connection test fails with host tunnel addressing and routes present. API/credential failures do not establish a server failure.

Observations live in `/tmp/proton-runtime/server-observations.json`, restricted to root, bounded to 256 records, and scoped to network ID, protocol, boot and endpoint address/public key. A successful connection remains verified for six hours. A failed connection lowers automatic priority for five minutes; it never removes a manual option. A successful retry clears the cooldown immediately. No observation causes a protocol downgrade or a background bulk connection scan.

The UI should show **eligible**, **verified**, or **temporarily unreachable** separately from measured latency. `protocols` describes eligible transports; `protocol_state` concerns the selected transport. These fields indicate metadata eligibility and recent observations, not a guarantee that every server will connect from every network.

## Official source basis

Proton Android commit [`deb11fef4092a1ecfcf51a7d40bdece9eb4c3e5e`](https://github.com/ProtonVPN/android-app/tree/deb11fef4092a1ecfcf51a7d40bdece9eb4c3e5e): `ConnectingDomain.kt` defines protocol-specific addresses/ports and address fallback; `SupportsProtocol.kt` checks resolved addresses and keys; `ProtocolSelection.kt` maps Stealth to WireGuard TLS; `ProtonVPNRetrofit.kt` requests protocol entries. These rules establish eligibility. Live authenticated connection checks establish verification.

Tests use synthetic servers and loopback fixtures. Real API/socket diagnostics remain explicitly opt-in.
