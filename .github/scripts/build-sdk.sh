#!/usr/bin/env bash
set -Eeuo pipefail
shopt -s nullglob
packages=()

phase=startup
on_error() {
    printf 'Failed in phase %s at line %s (exit %s)\n' "$phase" "$2" "$1" >&2
    exit "$1"
}
trap 'on_error "$?" "$LINENO"' ERR

run_phase() {
    phase=$1
    shift
    local started=$SECONDS
    echo "::group::$phase"
    "$@"
    echo "$phase completed in $((SECONDS - started))s"
    echo '::endgroup::'
}

preflight() {
    : "${SDK_ARCH:?Missing SDK architecture and release}"
    : "${PACKAGES_COMMIT:?Missing packages feed commit}"
    : "${TOOLS_CARGO_TARGET_DIR:?Missing Cargo cache directory}"
    : "${TOOLS_RUST_ROOT:?Missing prebuilt Rust toolchain}"
    : "${RUST_VERSION:?Missing expected Rust version}"
    : "${BUILD_JOBS:=$(nproc)}"
    : "${TOOLS_BUILD_MODE:=runtime}"

    if [[ ! $PACKAGES_COMMIT =~ ^[0-9a-f]{40}$ ]]; then
        echo 'Invalid packages feed commit' >&2
        exit 1
    fi
    if [[ ! $BUILD_JOBS =~ ^[1-9][0-9]*$ ]]; then
        echo 'BUILD_JOBS must be a positive integer' >&2
        exit 1
    fi
    case "$TOOLS_BUILD_MODE" in
        runtime) echo 'Tools runtime mode: opt-level=3, LTO=thin, codegen-units=16' ;;
        fast) echo 'Tools fast mode: opt-level=1, LTO=off, codegen-units=16' ;;
        *)
            echo 'TOOLS_BUILD_MODE must be runtime or fast' >&2
            exit 1
            ;;
    esac
    if [[ ! $SDK_ARCH =~ ^aarch64_cortex-a53-[0-9]+\.[0-9]+\.[0-9]+$ ]] ||
       [[ ${VERSION_PATH:-} != "releases/${SDK_ARCH##*-}" ]]; then
        echo "SDK does not match requested target: $SDK_ARCH" >&2
        exit 1
    fi

    local recipe package
    for recipe in /feed/*/Makefile; do
        package=${recipe%/Makefile}
        packages+=("${package##*/}")
    done
    if (( ${#packages[@]} == 0 )); then
        echo 'No package recipes found in /feed' >&2
        exit 1
    fi

    # Call the mounted compiler directly: runner rustup shims are not in the SDK.
    # Let OpenWrt add staging tools after checking the original host PATH.
    export PATH="$TOOLS_RUST_ROOT/bin:$PATH"
    "$TOOLS_RUST_ROOT/bin/rustc" --version | grep -F "rustc $RUST_VERSION "
    "$TOOLS_RUST_ROOT/bin/cargo" --version
    target_libdir="$("$TOOLS_RUST_ROOT/bin/rustc" --print target-libdir --target aarch64-unknown-linux-musl)"
    target_std=("$target_libdir"/libstd-*.rlib)
    if (( ${#target_std[@]} != 1 )); then
        echo 'Missing prebuilt ARM64 musl standard library' >&2
        exit 1
    fi
    sdk_make=(make "TOOLS_RUST_ROOT=$TOOLS_RUST_ROOT" "TOOLS_BUILD_MODE=$TOOLS_BUILD_MODE")
}

configure_sdk() {
    # Release images contain the SDK; also handle images with a setup script.
    if [[ ! -f rules.mk ]]; then
        bash ./setup.sh
    fi
    # AWS-LC in the new libraries uses the SDK's existing host CMake.
    test -x staging_dir/host/bin/cmake

    # Keep the SDK's Rust build flags and linker helpers at the reviewed revision.
    printf 'src-git packages https://github.com/openwrt/packages.git^%s\nsrc-link action /feed\n' \
        "$PACKAGES_COMMIT" > feeds.conf
    ./scripts/feeds update -a
    test "$(git -C feeds/packages rev-parse HEAD)" = "$PACKAGES_COMMIT"
    "${sdk_make[@]}" defconfig
    local package package_info
    ./scripts/feeds install -f -p action "${packages[@]}"
    for package in "${packages[@]}"; do
        sed -i "/^CONFIG_PACKAGE_${package}=/d; /^# CONFIG_PACKAGE_${package} is not set$/d" .config
        printf '\nCONFIG_PACKAGE_%s=m\n' "$package" >> .config
    done
    "${sdk_make[@]}" defconfig
    for package in "${packages[@]}"; do
        if ! grep -Eq "^CONFIG_PACKAGE_${package}=[my]$" .config; then
            echo "$package was not selected; refusing a skipped build" >&2
            exit 1
        fi
        package_info="$("${sdk_make[@]}" --no-print-directory -s -C "package/feeds/action/$package" TOPDIR=/builder DUMP=1)"
        grep -qx "Package: $package" <<< "$package_info"
        if grep -Eq '^Build-Depends[^:]*:.*rust/host' <<< "$package_info"; then
            echo "$package still requires compiling Rust/LLVM; refusing to continue" >&2
            exit 1
        fi
    done
}

compile_packages() {
    local package
    for package in "${packages[@]}"; do
        "${sdk_make[@]}" "package/$package/download" V=s
        "${sdk_make[@]}" "package/$package/check" V=s
        "${sdk_make[@]}" -j "$BUILD_JOBS" "package/$package/compile" V=s CONFIG_SIGNED_PACKAGES= CONFIG_AUTOREMOVE=
    done
}

verify_libraries() {
    # Run the libraries using the SDK's ARM64 musl loader and QEMU binfmt support.
    compilers=(staging_dir/toolchain-*/bin/aarch64-openwrt-linux-musl-gcc)
    if (( ${#compilers[@]} != 1 )); then
        echo 'Expected one ARM64 SDK compiler' >&2
        exit 1
    fi
    # OpenWrt's relocatable SDK keeps libc beside the toolchain, not necessarily
    # under GCC's reported sysroot. Make exports these staging paths only to its
    # own children; the standalone check compiler needs them explicitly as well.
    toolchain_dir="$(realpath "$(dirname "${compilers[0]}")/..")"
    target_dirs=(staging_dir/target-*)
    if (( ${#target_dirs[@]} != 1 )) || [[ ! -d ${target_dirs[0]} ]]; then
        echo 'Expected one SDK target staging directory' >&2
        exit 1
    fi
    STAGING_DIR="$(realpath "${target_dirs[0]}")"
    export STAGING_DIR
    export STAGING_DIR_HOST=/builder/staging_dir/host
    loader="$toolchain_dir/lib/ld-musl-aarch64.so.1"
    if [[ ! -x $loader ]]; then
        echo "Missing executable SDK musl loader: $loader" >&2
        exit 1
    fi
    echo "Checking library loading with SDK musl: $loader"
    "${compilers[0]}" -Wall -Wextra -Werror -O2 /ci/load-library.c -o /tmp/tools-load-test -ldl

    profile=release
    if grep -qx 'CONFIG_DEBUG=y' .config; then
        profile=debug
    fi
    local package basename symbol unpacked needed dependency library
    readelf="${compilers[0]%-gcc}-readelf"
    local -a staged
    for package in "${packages[@]}"; do
        case "$package" in
            protun-openwrt|proton-stealth-integration) continue ;; # Executable / shell integration checks below.
            tailscale-rs) basename=libtailscalers.so; symbol=ts_init ;;
            local-agent-rs) basename=liblocal_agent_rs.so; symbol=ffi_local_agent_rs_uniffi_contract_version ;;
            *) echo "Missing library check for $package" >&2; exit 1 ;;
        esac
        unpacked="$TOOLS_CARGO_TARGET_DIR/aarch64-unknown-linux-musl/$profile/$basename"
        if [[ ! -s $unpacked ]]; then
            echo "Missing built library: $unpacked" >&2
            exit 1
        fi

        # Check the built library's dependencies independently of SDK package scanning.
        needed="$("$readelf" -d "$unpacked" | sed -n 's/.*(NEEDED).*\[\([^]]*\)\].*/\1/p')"
        if [[ -z $needed ]]; then
            echo "No shared-library dependencies found in: $unpacked" >&2
            exit 1
        fi
        while IFS= read -r dependency; do
            case "$dependency" in
                libc.so|libgcc_s.so.1) ;;
                *)
                    echo "Undeclared runtime dependency: $dependency; update Package/$package/DEPENDS" >&2
                    exit 1
                    ;;
            esac
        done <<< "$needed"

        mapfile -t staged < <(find build_dir -type f -path "*/ipkg-*/$package/usr/lib/$basename")
        if (( ${#staged[@]} != 1 )); then
            echo "Expected one staged $package shared library" >&2
            exit 1
        fi
        for library in "$unpacked" "${staged[0]}"; do
            "$loader" --library-path "$toolchain_dir/lib:$STAGING_DIR/lib:$STAGING_DIR/usr/lib" \
                /tmp/tools-load-test "$(realpath "$library")" "$symbol"
        done
    done
}

export_apks() {
    local package
    local -a matches exports=()
    for package in "${packages[@]}"; do
        mapfile -t matches < <(find bin/packages -type f -name "$package-*.apk")
        if (( ${#matches[@]} != 1 )); then
            echo "Expected exactly one $package APK" >&2
            exit 1
        fi
        exports+=("${matches[0]}")
    done
    cp -- "${exports[@]}" /out/
    printf 'Exported %s\n' "${exports[@]}"
}

verify_client() {
    local -a clients
    mapfile -t clients < <(find build_dir -type f -path '*/ipkg-*/protun-openwrt/usr/sbin/protun-openwrt')
    if (( ${#clients[@]} != 1 )); then
        echo 'Expected one staged protocol client' >&2
        exit 1
    fi
    local client fixture key needed dependency
    client="$(realpath "${clients[0]}")"
    if find "${client%/usr/sbin/protun-openwrt}" -type f -name libprotun.so | grep -q .; then
        echo 'Client-only package unexpectedly contains libprotun.so' >&2
        exit 1
    fi
    needed="$("$readelf" -d "$client" | sed -n 's/.*(NEEDED).*\[\([^]]*\)\].*/\1/p')"
    if [[ -z $needed ]]; then
        echo 'Expected the client to use the SDK shared runtime' >&2
        exit 1
    fi
    while IFS= read -r dependency; do
        case "$dependency" in
            libc.so|libgcc_s.so.1) ;;
            *) echo "Undeclared client dependency: $dependency" >&2; exit 1 ;;
        esac
    done <<< "$needed"
    fixture=$(mktemp -d)
    key='AQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQE='
    printf '%s\n' "$key" > "$fixture/private.key"
    chmod 600 "$fixture/private.key"
    printf '{"tun_name":"protun0","uplink_device":"eth0","server_ip":"192.0.2.1","server_public_key":"%s","private_key_file":"%s/private.key","tls_port":443,"connect_timeout_seconds":60}\n' \
        "$key" "$fixture" > "$fixture/client.json"
    chmod 600 "$fixture/client.json"
    local -a run_client=("$loader" --library-path "$toolchain_dir/lib:$STAGING_DIR/lib:$STAGING_DIR/usr/lib" "$client")
    "${run_client[@]}" --help
    "${run_client[@]}" --capabilities
    "${run_client[@]}" --check-config "$fixture/client.json"
    local mode expected summary test_binary
    for mode in stealth wireguard-udp wireguard-tcp auto; do
        sed "s/}$/,\"protocol\":\"$mode\",\"udp_ports\":[51820],\"tcp_ports\":[443]}/" "$fixture/client.json" > "$fixture/mode.json"
        summary="$("${run_client[@]}" --check-config "$fixture/mode.json")"
        case "$mode" in
            stealth) expected='protocol=Stealth peers=1 udp=0 tcp=0 tls=1' ;;
            wireguard-udp) expected='protocol=WireguardUdp peers=1 udp=1 tcp=0 tls=0' ;;
            wireguard-tcp) expected='protocol=WireguardTcp peers=1 udp=0 tcp=1 tls=0' ;;
            auto) expected='protocol=Auto peers=1 udp=1 tcp=1 tls=1' ;;
        esac
        grep -Fq "$expected" <<< "$summary"
    done
    sed 's/}$/,"unsupported":true}/' "$fixture/client.json" > "$fixture/unknown.json"
    if "${run_client[@]}" --check-config "$fixture/unknown.json"; then
        echo 'Client accepted an unknown configuration field' >&2
        exit 1
    fi
    ln -s "$fixture/private.key" "$fixture/linked.key"
    sed "s|$fixture/private.key|$fixture/linked.key|" "$fixture/client.json" > "$fixture/symlink.json"
    if "${run_client[@]}" --check-config "$fixture/symlink.json"; then
        echo 'Client accepted a symlinked private key' >&2
        exit 1
    fi
    chmod 644 "$fixture/private.key"
    if "${run_client[@]}" --check-config "$fixture/client.json"; then
        echo 'Client accepted an exposed private key' >&2
        exit 1
    fi
    chmod 600 "$fixture/private.key"
    printf '%s\n' 'invalid-key' > "$fixture/private.key"
    if "${run_client[@]}" --check-config "$fixture/client.json"; then
        echo 'Client accepted an invalid private key' >&2
        exit 1
    fi
    rm -- "$fixture/client.json" "$fixture/private.key" "$fixture/unknown.json" "$fixture/symlink.json" "$fixture/linked.key" "$fixture/mode.json"
    rmdir -- "$fixture"
    test_binary="$(python3 - <<'PY'
import json
from pathlib import Path
records = list(Path('build_dir').rglob('openwrt-tests.jsonl'))
if len(records) != 1:
    raise SystemExit('Expected one client unit-test manifest')
tests = [r['executable'] for line in records[0].read_text().splitlines()
         if (r := json.loads(line)).get('reason') == 'compiler-artifact'
         and r.get('profile', {}).get('test') and r.get('executable')
         and r.get('target', {}).get('name') == 'protun-openwrt']
if len(tests) != 1:
    raise SystemExit('Expected one client unit-test executable')
print(tests[0])
PY
    )"
    "$loader" --library-path "$toolchain_dir/lib:$STAGING_DIR/lib:$STAGING_DIR/usr/lib" "$test_binary"
    echo 'ARM64 client starts and accepts/rejects configuration as expected; no network session tested'
}

verify_integration() {
    local -a roots
    local root script
    mapfile -t roots < <(find build_dir -type d -path '*/ipkg-*/proton-stealth-integration')
    if (( ${#roots[@]} != 1 )); then
        echo 'Expected one staged Stealth integration package' >&2; exit 1
    fi
    root=${roots[0]}
    for script in usr/lib/exitnode/backend.sh usr/libexec/proton-tunnel usr/libexec/proton-stealth usr/libexec/proton-transport-metadata etc/init.d/proton-stealth; do
        test -s "$root/$script"
        bash -n "$root/$script"
    done
    echo 'Integration shell syntax checked; no router policy applied'
}

cd /builder
run_phase 'Preflight' preflight
run_phase 'SDK feeds and configuration' configure_sdk
run_phase 'Compile and package' compile_packages
run_phase 'ARM64 library checks' verify_libraries
run_phase 'ARM64 protocol client checks' verify_client
run_phase 'Stealth integration checks' verify_integration
run_phase 'Export APKs' export_apks
