# Build and package

Commit the source files to `main`, then run **Build OpenWrt tools APKs** in
GitHub Actions. Package and build-script changes also start a build automatically.
Successful builds on `main` commit one latest APK per package to `build/`.
Pull requests build and verify without publishing.

The SDK tag, pinned Rust version, and build mode are in
`.github/workflows/sdk-build.yml`. `runtime` mode uses optimization level 3 and
ThinLTO. `fast` mode uses optimization level 1 with LTO disabled.

CI uses prebuilt Rust with the OpenWrt linker and caches downloads and compilation
outputs. Building this feed outside CI requires a compatible Rust toolchain and
the OpenWrt packages feed's Rust helpers.

CI checks the APKs under ARM64 emulation. See [Proton deployment](stealth.md) for router and VPN checks.
