# OpenWrt package development

- Start with [build instructions](docs/building.md) and [upstream update workflow](docs/development.md).
- `package/protun-openwrt/` contains the ProtunNext adapter, CLI configuration, service and controller contracts. Preserve existing config and status formats, explicit protocol selection and Direct-at-boot behavior. Test the ARM64 Proton client in the SDK.
- `package/tailscale-rs/` packages upstream `ts_ffi` as `libtailscalers.so` with `tailscale.h`. Preserve the installed and SDK staging paths and test both with a C consumer. Stage the header from Cargo's recorded build output so a warm cache works with a fresh source checkout. Check new APIs and configuration against the exact pinned [upstream source](https://github.com/tailscale/tailscale-rs).
- `package/rust-library.mk` is shared by both packages. Check both APKs after changes to it.
- Run `python3 -m unittest discover -s .github/scripts -p 'test_*.py'` and `python3 .github/scripts/check_repository.py` before the SDK build.
- Verify findings and code changes against official source, docs, and examples for the pinned revision. Review the final diff and give reproducible evidence with each claim. Identify checks you could not run and leave uncertain claims open.
- Base performance claims on paired measurements. For a new benchmark, include the workload, metric, and reproducible procedure in the proposal. Keep changes focused and preserve supported interfaces.
