# OpenWrt tools

Build a Proton VPN client and an embedded Tailscale Rust library for an ARM64 Cortex-A53 router.

| Package | Installed program or library |
| --- | --- |
| `protun-openwrt` | `/usr/sbin/protun-openwrt` |
| `tailscale-rs` | `/usr/lib/libtailscalers.so` |

The `protun-openwrt` APK supports WireGuard UDP, WireGuard TCP, Stealth, automatic
protocol selection, and integrated Local Agent/account-session interfaces.
It builds the managed connection engine from a stable ProtunNext release,
pinned to the release commit and source archive SHA-256.
Stealth remains the default. The OpenWrt service is disabled until configured.
See [Stealth setup and verification](docs/stealth.md). Installing the package
leaves existing network interfaces, routing, DNS, and firewall rules in place.

## Tailscale Rust library

`tailscale-rs` packages the official upstream C ABI for OpenWrt 25.12 ARM64.
It installs `/usr/lib/libtailscalers.so` and its generated header,
`/usr/include/tailscale.h`. Both are also available in the SDK staging directory
to packages that depend on `tailscale-rs`.

Compile [the C smoke example](package/tailscale-rs/example.c) with the OpenWrt
SDK compiler and link it with `-ltailscalers`. It initializes logging without
tailnet enrollment. An application can use the upstream C API to configure
and enroll an embedded device, obtain its addresses, and use tailnet TCP and UDP
sockets. See the [upstream examples and feature status](https://github.com/tailscale/tailscale-rs/tree/v0.7.1#status)
for the Rust API, separate language bindings, and supported networking features.

The APK contains the C binding. It includes no router daemon, admin CLI, or
system routing integration. The SDK check compiles the example with the packaged
header and runs it with the packaged library under ARM64 emulation.
Live tailnet traffic and OpenWrt installations require separate device testing.

## Documentation

- [Development and upstream updates](docs/development.md)
- [Build and package](docs/building.md)
- [Deploy the Proton VPN client](docs/stealth.md)
- [Deploy with an exit-node controller](docs/controller-integration.md)
