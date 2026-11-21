# OpenWrt tools

Build a Proton VPN client and two Rust library APKs for an ARM64 Cortex-A53 router:

| Package | Installed program or library |
| --- | --- |
| `protun-openwrt` | `/usr/sbin/protun-openwrt` |
| `tailscale-rs` | `/usr/lib/libtailscalers.so` |
| `local-agent-rs` | `/usr/lib/liblocal_agent_rs.so` |

Applications use these libraries through their upstream interfaces. The `protun-openwrt`
APK includes a client supporting WireGuard UDP, WireGuard TCP, Stealth, automatic
protocol selection, and integrated Local Agent/account-session interfaces.
It builds the managed connection engine from a stable ProtunNext release,
pinned to the release commit and source archive SHA-256.
Stealth remains the default. The OpenWrt service is disabled until configured.
See [Stealth setup and verification](docs/stealth.md). Installing the package
does not migrate a WireGuard interface or change ZeroTier, routing, DNS or firewall
configuration. The other two APKs contain libraries, not runnable VPN services.

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
system routing integration. The SDK check compiles and runs the example against
the extracted APK under ARM64 emulation. Live tailnet traffic and OpenWrt router
installations are outside that check.

## Build

Commit the source files to `main`, then run **Build OpenWrt tools APKs** in
GitHub Actions. Package and build-script changes also start a build automatically.
Successful builds on `main` commit one latest APK per package to `build/`.
Pull requests build and verify without publishing.

The SDK tag, pinned Rust version, and build mode are in
`.github/workflows/build.yml`. `runtime` mode uses optimization level 3 and
ThinLTO. `fast` mode uses optimization level 1 with LTO disabled.

CI uses prebuilt Rust with the OpenWrt linker and caches downloads and compilation
outputs. Building this feed outside CI requires a compatible Rust toolchain and
the OpenWrt packages feed's Rust helpers.

## Automatic updates

Add the repository Actions secret `UPSTREAM_UPDATE_TOKEN`, using a token with
**Contents** and **Workflows** read/write permissions for this repository.
Install the updater workflow on `main`.

**Update upstream versions** checks every two days. Manual runs check immediately
and default to preview.
It follows stable package releases and OpenWrt patch releases within the existing
release series. Package versions, source hashes, and dependencies live in each
package's `Makefile` and `files/Cargo.lock`. The exact candidate must pass
the OpenWrt SDK build and APK tests before the workflow pushes it to `main`.
A source or compiler incompatibility stops the update for review.

New Tailscale Rust releases are pinned to their release commit and source hash.
The updater qualifies the exact candidate and leaves it as a patch artifact for
a pull request. Review the upstream changelog and C API, update this section if
the exposed features change, and run the SDK checks on that PR.

For future ProtunNext releases, add the OpenCode Zen key as `OPENCODE_API_KEY`
in this repository. The updater can then use the paid OpenCode Console model
`opencode/deepseek-v4.1-flash` to adapt existing client, config, status and cache
code to a new API. The model runs
with a limited copy of public source files. The updater uses the
[locked OpenCode v2 CLI](.github/opencode/package-lock.json) in standalone mode
and verifies the model identity before accepting edits. To test the integration
on a text fixture, dispatch **Update upstream versions** with **preview** and
**opencode_smoke** enabled. The SDK qualification and
publication jobs handle the resulting patch separately.

The locally maintained `proton-account` package has no upstream release to track.
SDK updates bump its package release along with the upstream packages.

Run updater regression tests with
`python3 -m unittest discover -s .github/scripts -p 'test_*.py'`.

## Layout

- `package/*/Makefile`: package sources, versions, library names, and features.
- `package/rust-library.mk`: shared Rust compilation and installation rules.
- `.github/scripts/build-sdk.sh`: SDK setup, compilation, and ARM64 load checks.
- `.github/scripts/publish-apks.sh`: latest-only APK publishing.
- `.github/scripts/update-versions.py`: upstream checks and version updates.

Library load checks verify linking and exported symbols. Router functionality and
VPN connectivity require device testing.
