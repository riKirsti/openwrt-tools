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
            protun) basename=libprotun.so; symbol=ffi_protun_uniffi_contract_version ;;
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

cd /builder
run_phase 'Preflight' preflight
run_phase 'SDK feeds and configuration' configure_sdk
run_phase 'Compile and package' compile_packages
run_phase 'ARM64 library checks' verify_libraries
run_phase 'Export APKs' export_apks
