# OpenWrt tools

Build a Stealth client and two Rust library APKs for an ARM64 Cortex-A53 router:

| Package | Installed program or library |
| --- | --- |
| `protun-openwrt` | `/usr/sbin/protun-openwrt` |
| `tailscale-rs` | `/usr/lib/libtailscalers.so` |
| `local-agent-rs` | `/usr/lib/liblocal_agent_rs.so` |

Applications use these libraries through their upstream interfaces. The `protun-openwrt`
APK includes the experimental Stealth client and a disabled OpenWrt service.
See [Stealth setup and verification](docs/stealth.md). Installing the package
does not migrate a WireGuard interface or change ZeroTier, routing, DNS or firewall
configuration. The other two APKs contain libraries, not runnable VPN services.

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

**Update upstream versions** checks every two days; manual runs check immediately.
It follows stable package releases and OpenWrt patch releases within the existing
release series. Package versions, source hashes, and dependencies live in each
package's `Makefile` and `files/Cargo.lock`. Updates are committed only after source
and dependency checks succeed. A source or compiler incompatibility stops the
update for review.

## Layout

- `package/*/Makefile`: package sources, versions, library names, and features.
- `package/rust-library.mk`: shared Rust compilation and installation rules.
- `.github/scripts/build-sdk.sh`: SDK setup, compilation, and ARM64 load checks.
- `.github/scripts/publish-apks.sh`: latest-only APK publishing.
- `.github/scripts/update-versions.py`: upstream checks and version updates.

Library load checks verify linking and exported symbols. Router functionality and
VPN connectivity require device testing.
