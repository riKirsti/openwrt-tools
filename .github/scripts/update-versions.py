#!/usr/bin/env python3
"""Update stable tool releases, checked lockfiles and the OpenWrt release SDK."""

import argparse
import hashlib
import io
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import tarfile
import tempfile
import tomllib
import urllib.error
import urllib.request


VERSION = r"(\d+\.\d+\.\d+)"
WORKFLOW = Path(".github/workflows/build.yml")


def fetch(url, token=None, accept=None):
    headers = {"User-Agent": "openwrt-tools-version-updater"}
    if token:
        headers["Authorization"] = "Bearer " + token
    if accept:
        headers["Accept"] = accept
    with urllib.request.urlopen(urllib.request.Request(url, headers=headers), timeout=60) as response:
        return response.read()


def version(value):
    if not isinstance(value, str) or not re.fullmatch(VERSION, value):
        raise ValueError(f"Not a stable release version: {value!r}")
    return tuple(map(int, value.split(".")))


def match_one(pattern, text):
    matches = list(re.finditer(pattern, text, re.MULTILINE))
    if len(matches) != 1:
        raise ValueError(f"Expected one match for {pattern!r}, found {len(matches)}")
    return matches[0]


def latest_tag(repository):
    token = os.environ.get("GH_TOKEN")
    try:
        release = json.loads(fetch(f"https://api.github.com/repos/{repository}/releases/latest", token=token))
    except urllib.error.HTTPError as error:
        if error.code != 404:
            raise
    else:
        tag = release["tag_name"]
        if release.get("draft") or release.get("prerelease") or not re.fullmatch("v?" + VERSION, tag):
            raise ValueError(f"Unexpected stable release for {repository}: {tag!r}")
        return tag
    # Some upstreams publish stable tags without creating GitHub Releases.
    tags = []
    page = 1
    while True:
        batch = json.loads(fetch(f"https://api.github.com/repos/{repository}/tags?per_page=100&page={page}", token=token))
        tags.extend(t["name"] for t in batch if re.fullmatch("v?" + VERSION, t["name"]))
        if len(batch) < 100:
            break
        page += 1
    if not tags:
        raise ValueError(f"No stable releases or version tags found for {repository}")
    return max(tags, key=lambda tag: version(tag.removeprefix("v")))


def latest_openwrt(current):
    # Follow patch releases for the router's existing release series.
    index = fetch("https://downloads.openwrt.org/releases/").decode()
    candidates = re.findall(r'href=["\']' + VERSION + r'/["\']', index)
    candidates = [v for v in candidates if version(v)[:2] == version(current)[:2]]
    return max([current, *candidates], key=version)


def sdk_available(architecture, release):
    auth = json.loads(fetch("https://ghcr.io/token?service=ghcr.io&scope=repository:openwrt/sdk:pull"))
    try:
        fetch(f"https://ghcr.io/v2/openwrt/sdk/manifests/{architecture}-{release}",
              token=auth["token"],
              accept="application/vnd.oci.image.index.v1+json, application/vnd.docker.distribution.manifest.list.v2+json, application/vnd.oci.image.manifest.v1+json")
    except urllib.error.HTTPError as error:
        if error.code == 404:
            return False
        raise
    return True


# Repository tags and the actual Rust build targets are not always the same version.
PROJECTS = {
    "protun-openwrt": {"repository": "ProtonVPN/protun", "crate": "protun",
               "manifest": "Cargo.toml", "release_manifest": "Cargo.toml",
               "features": ["linux", "openwrt-client"]},
    "tailscale-rs": {"repository": "tailscale/tailscale-rs", "crate": "ts_ffi",
                     "manifest": "Cargo.toml", "release_manifest": "Cargo.toml", "features": []},
    "local-agent-rs": {"repository": "ProtonVPN/local-agent-rs", "crate": "local-agent-rs",
                       "manifest": "local_agent_rs/Cargo.toml",
                       "release_manifest": "python-proton-vpn-local-agent/Cargo.toml", "features": ["uniffi"]},
}


def manifest_version(source, path):
    manifest = tomllib.loads((source / path).read_text(encoding="utf-8"))
    value = manifest.get("package", {}).get("version")
    if not isinstance(value, str):
        value = manifest.get("workspace", {}).get("package", {}).get("version")
    version(value)
    return value


def updated_source(root, name, tag, cargo, rust_version):
    project = PROJECTS[name]
    release = tag.removeprefix("v")
    url = f"https://codeload.github.com/{project['repository']}/tar.gz/{tag}"
    archive = fetch(url)
    digest = hashlib.sha256(archive).hexdigest()
    with tempfile.TemporaryDirectory(prefix=f"{name}-update-") as temporary:
        work = Path(temporary)
        with tarfile.open(fileobj=io.BytesIO(archive)) as source_tar:
            source_tar.extractall(work, filter="data")
        directories = list(work.iterdir())
        if len(directories) != 1 or not directories[0].is_dir():
            raise ValueError("Expected one source directory")
        source = directories[0]
        if manifest_version(source, project["release_manifest"]) != release:
            raise ValueError(f"Source manifest does not match {name} release {release}")
        for patch in sorted((root / "package" / name / "patches").glob("*.patch")):
            subprocess.run(["git", "apply", "--check", str(patch)], cwd=source, check=True)
            subprocess.run(["git", "apply", str(patch)], cwd=source, check=True)
        if name == "protun-openwrt":
            for client_source in (root / "package/protun-openwrt/files").glob("openwrt-*.rs"):
                shutil.copyfile(client_source, source / client_source.name)
        build_source = (source / project["manifest"]).parent
        shutil.copyfile(root / "package" / name / "files/Cargo.lock", build_source / "Cargo.lock")
        # Retain existing dependencies wherever the new manifest permits them.
        subprocess.run([cargo, "update", "--workspace"], cwd=build_source, check=True)
        features = ["--features", ",".join(project["features"])] if project["features"] else []
        result = subprocess.run([cargo, "metadata", "--locked", "--format-version", "1",
                                 "--filter-platform", "aarch64-unknown-linux-musl", *features],
                                cwd=build_source, check=True, capture_output=True, text=True, encoding="utf-8")
        metadata = json.loads(result.stdout)
        packages = {p["id"]: p for p in metadata["packages"]}
        nodes = {n["id"]: n for n in metadata["resolve"]["nodes"]}
        targets = [p for p in metadata["packages"] if p["name"] == project["crate"] and p["source"] is None]
        if len(targets) != 1:
            raise ValueError(f"Expected one local build target: {project['crate']}")
        pending = [targets[0]["id"]]
        seen = set()
        while pending:
            identifier = pending.pop()
            if identifier in seen:
                continue
            seen.add(identifier)
            package = packages[identifier]
            minimum = package.get("rust_version")
            if minimum:
                required = tuple(map(int, minimum.split(".")))
                required = (*required, *([0] * (3 - len(required))))
                if required > version(rust_version):
                    raise ValueError(f"{package['name']} requires Rust {minimum}; pinned Rust is {rust_version}")
            for dependency in nodes[identifier]["deps"]:
                if any(kind["kind"] != "dev" for kind in dependency["dep_kinds"]):
                    pending.append(dependency["pkg"])
        lock = (build_source / "Cargo.lock").read_bytes()
        lock_data = tomllib.loads(lock.decode())
        if not any(p["name"] == project["crate"] and p["version"] == targets[0]["version"]
                   for p in lock_data["package"]):
            raise ValueError(f"Updated lockfile does not match {project['crate']}")
    return digest, lock


def update(root, cargo="cargo"):
    root = root.resolve()
    workflow = (root / WORKFLOW).read_text(encoding="utf-8")
    recipes = {path.parent.name for path in root.glob("package/*/Makefile")}
    if recipes != PROJECTS.keys():
        raise ValueError("Package recipes and updater targets do not match")
    sdk = match_one(r"^\s+SDK_ARCH: ([a-z0-9_-]+)-" + VERSION + "$", workflow)
    architecture, current_openwrt = sdk.group(1), sdk.group(2)
    rust_version = match_one(r"^\s+RUST_VERSION: " + VERSION + "$", workflow).group(1)
    new_openwrt = latest_openwrt(current_openwrt)
    sdk_changed = version(new_openwrt) > version(current_openwrt)
    if sdk_changed and not sdk_available(architecture, new_openwrt):
        print(f"OpenWrt {new_openwrt} SDK is not published yet; retaining {current_openwrt}")
        sdk_changed = False
    changes = {}
    summary = []
    for name, project in PROJECTS.items():
        make_path = Path("package") / name / "Makefile"
        makefile = (root / make_path).read_text(encoding="utf-8")
        original = makefile
        if match_one(r"^PKG_NAME:=(.+)$", makefile).group(1) != name:
            raise ValueError(f"Package name does not match its folder: {name}")
        current = match_one(r"^PKG_VERSION:=" + VERSION + "$", makefile).group(1)
        revision = int(match_one(r"^PKG_RELEASE:=(\d+)$", makefile).group(1))
        match_one(r"^PKG_HASH:=[0-9a-f]{64}$", makefile)
        tag = latest_tag(project["repository"])
        release = tag.removeprefix("v")
        if version(release) > version(current):
            source_url = match_one(r"^PKG_SOURCE_URL:=(https://\S+)\?$", makefile).group(1)
            expected_url = f"https://codeload.github.com/{project['repository']}/tar.gz/{tag}"
            if source_url.replace("$(PKG_VERSION)", release) != expected_url:
                raise ValueError(f"Release tag does not match {name}'s source URL: {tag}")
            digest, lock = updated_source(root, name, tag, cargo, rust_version)
            makefile = re.sub(r"^PKG_VERSION:=" + VERSION + "$", "PKG_VERSION:=" + release, makefile, flags=re.MULTILINE)
            makefile = re.sub(r"^PKG_HASH:=[0-9a-f]{64}$", "PKG_HASH:=" + digest, makefile, flags=re.MULTILINE)
            makefile = re.sub(r"^PKG_RELEASE:=\d+$", "PKG_RELEASE:=1", makefile, flags=re.MULTILINE)
            changes[Path("package") / name / "files/Cargo.lock"] = lock
            summary.append(f"{name} {current} -> {release}")
        elif sdk_changed:
            makefile = re.sub(r"^PKG_RELEASE:=\d+$", f"PKG_RELEASE:={revision + 1}", makefile, flags=re.MULTILINE)
        if makefile != original:
            changes[make_path] = makefile.encode()
    if sdk_changed:
        replacement = f"{architecture}-{new_openwrt}"
        start, end = sdk.span(1)[0], sdk.span(2)[1]
        changes[WORKFLOW] = (workflow[:start] + replacement + workflow[end:]).encode()
        summary.append(f"OpenWrt {current_openwrt} -> {new_openwrt}")
    # No repository writes until all downloads, patches and resolutions succeed.
    for path, data in changes.items():
        (root / path).write_bytes(data.replace(b"\r\n", b"\n"))
    print("; ".join(summary) if summary else "No newer supported stable releases; no files changed")
    return summary


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, default=Path(__file__).resolve().parents[2])
    parser.add_argument("--cargo", default="cargo")
    args = parser.parse_args()
    update(args.root, args.cargo)
