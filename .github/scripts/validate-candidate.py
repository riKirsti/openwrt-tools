"""Validate a staged candidate before it reaches the SDK."""

import re
import subprocess
import sys
from pathlib import Path


SDK = ".github/workflows/sdk-build.yml"
PACKAGES = ("protun-openwrt", "tailscale-rs")
UPSTREAM_PATHS = {SDK, *(f"package/{name}/{file}" for name in PACKAGES
                        for file in ("Makefile", "files/Cargo.lock")),
                  *(f"package/protun-openwrt/files/openwrt-{name}.rs"
                    for name in ("client", "config", "cache", "status")),
                  "package/tailscale-rs/example.c", "package/tailscale-rs/files/stage-header.py"}


def git(*args):
    return subprocess.check_output(["git", *args])


def allowed(path, mode):
    if mode == "upstream":
        return path in UPSTREAM_PATHS
    return (path in {"README.md", "package/rust-library.mk"}
            or path.startswith(("docs/", *(f"package/{name}/" for name in PACKAGES))))


def validate(mode):
    paths = list(filter(None, git("diff", "--cached", "--name-only", "--no-renames", "-z").decode().split("\0")))
    if not paths or any(not allowed(path, mode) for path in paths):
        raise ValueError("Candidate has no changes or edits paths outside its scope")
    for path in paths:
        entries = git("ls-files", "-s", "-z", "--", path).split(b"\0")
        if any(entry and entry.split(b" ", 1)[0] not in {b"100644", b"100755"} for entry in entries):
            raise ValueError(f"Unexpected file type in {path}")
        if Path(path).is_file():
            text = Path(path).read_bytes().decode("utf-8")
            if "\0" in text:
                raise ValueError(f"Unexpected binary file in {path}")
    if SDK in paths:
        original = git("show", f"HEAD:{SDK}").decode()
        candidate = Path(SDK).read_text(encoding="utf-8")
        pattern = r"(?m)^      SDK_ARCH: (aarch64_cortex-a53)-[0-9]+\.[0-9]+\.[0-9]+$"
        if (len(re.findall(pattern, candidate)) != 1 or len(re.findall(pattern, original)) != 1
                or re.sub(pattern, "SDK_ARCH", candidate) != re.sub(pattern, "SDK_ARCH", original)):
            raise ValueError("Only the supported SDK version can change in an upstream candidate")
    for name in PACKAGES:
        path = f"package/{name}/Makefile"
        properties = dict(re.findall(r"(?m)^(PKG_[A-Z_]+):=([^\n]+)$", Path(path).read_text(encoding="utf-8")))
        required = {"PKG_NAME": re.escape(name), "PKG_VERSION": r"\d+\.\d+\.\d+",
                    "PKG_RELEASE": r"[1-9]\d*", "PKG_HASH": r"[a-f0-9]{64}",
                    "PKG_SOURCE_COMMIT": r"[a-f0-9]{40}"}
        if any(not re.fullmatch(pattern, properties.get(key, "")) for key, pattern in required.items()):
            raise ValueError(f"Invalid package metadata in {path}")
        if mode == "maintenance" and path in paths:
            original = dict(re.findall(r"(?m)^(PKG_[A-Z_]+):=([^\n]+)$", git("show", f"HEAD:{path}").decode()))
            if any(properties[key] != original[key] for key in required):
                raise ValueError("Maintenance proposals must preserve pinned package release metadata")
    return paths


if __name__ == "__main__":
    if len(sys.argv) != 2 or sys.argv[1] not in {"upstream", "maintenance"}:
        raise SystemExit("Usage: validate-candidate.py upstream|maintenance")
    try:
        validate(sys.argv[1])
    except (ValueError, OSError, subprocess.CalledProcessError) as error:
        raise SystemExit(str(error)) from None
