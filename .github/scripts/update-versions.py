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

from adapt_protunnext import adapt


VERSION = r"(\d+\.\d+\.\d+)"
WORKFLOW = Path(".github/workflows/sdk-build.yml")
PROTUNNEXT = "riKirsti/ProtunNext"
COMMIT = r"[0-9a-f]{40}"


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
        if error.code != 404 or repository == PROTUNNEXT:
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


def release_commit(repository, tag):
    token = os.environ.get("GH_TOKEN")
    revision = json.loads(fetch(f"https://api.github.com/repos/{repository}/commits/{tag}", token=token))["sha"]
    if not re.fullmatch(COMMIT, revision):
        raise ValueError("Invalid release commit identity")
    return revision


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
    "protun-openwrt": {"repository": PROTUNNEXT, "crate": "protun-next",
               "manifest": "Cargo.toml", "release_manifest": "Cargo.toml",
               "features": ["openwrt-client"]},
    "tailscale-rs": {"repository": "tailscale/tailscale-rs", "crate": "ts_ffi",
                     "manifest": "Cargo.toml", "release_manifest": "Cargo.toml",
                     "features": []},
}


def manifest_version(source, path):
    manifest = tomllib.loads((source / path).read_text(encoding="utf-8"))
    value = manifest.get("package", {}).get("version")
    if not isinstance(value, str):
        value = manifest.get("workspace", {}).get("package", {}).get("version")
    version(value)
    return value


def updated_source(root, name, tag, cargo, rust_version, revision, model_key="", *,
                   lock_updates=(), expected_digest=None):
    project = PROJECTS[name]
    release = tag.removeprefix("v")
    url = f"https://codeload.github.com/{project['repository']}/tar.gz/{revision}"
    archive = fetch(url)
    digest = hashlib.sha256(archive).hexdigest()
    if expected_digest and digest != expected_digest:
        raise ValueError(f"Pinned source checksum changed: {name}")
    with tempfile.TemporaryDirectory(prefix=f"{name}-update-") as temporary:
        work = Path(temporary)
        with tarfile.open(fileobj=io.BytesIO(archive)) as source_tar:
            source_tar.extractall(work, filter="data")
        directories = list(work.iterdir())
        if len(directories) != 1 or not directories[0].is_dir():
            raise ValueError("Expected one source directory")
        source = directories[0]
        archive_name = project["repository"].split("/")[-1]
        if source.name != f"{archive_name}-{revision}":
            raise ValueError("Archive directory does not match the pinned release commit")
        if manifest_version(source, project["release_manifest"]) != release:
            raise ValueError(f"Source manifest does not match {name} release {release}")
        for patch in sorted((root / "package" / name / "patches").glob("*.patch")):
            subprocess.run(["git", "apply", "--check", str(patch)], cwd=source, check=True)
            subprocess.run(["git", "apply", str(patch)], cwd=source, check=True)
        edits = {}
        if name == "protun-openwrt":
            edits = adapt(root, source, tag, revision, model_key)
            for client_source in (root / "package/protun-openwrt/files").glob("openwrt-*.rs"):
                relative = client_source.relative_to(root)
                if relative in edits:
                    (source / client_source.name).write_bytes(edits[relative])
                else:
                    shutil.copyfile(client_source, source / client_source.name)
        else:
            edits = adapt(root, source, tag, revision, model_key, package=name)
        build_source = (source / project["manifest"]).parent
        if name != "protun-openwrt":
            shutil.copyfile(root / "package" / name / "files/Cargo.lock", build_source / "Cargo.lock")
        # Retain existing dependencies wherever the new manifest permits them.
        if lock_updates:
            for dependency, precise in lock_updates:
                subprocess.run([cargo, "update", "-p", dependency, "--precise", precise],
                               cwd=build_source, check=True)
        else:
            subprocess.run([cargo, "update", "--workspace"], cwd=build_source, check=True)
        features = ["--features", ",".join(project["features"])] if project["features"] else []
        if name == "protun-openwrt":
            features.insert(0, "--no-default-features")
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
    return digest, lock, edits


def update(root, cargo="cargo", model_key="", replay="", lane=""):
    root = root.resolve()
    workflow = (root / WORKFLOW).read_text(encoding="utf-8")
    recipes = {path.parent.name for path in root.glob("package/*/Makefile")}
    expected = PROJECTS.keys()
    if recipes != expected:
        raise ValueError(
            "Package recipes and updater targets do not match: "
            f"missing recipes={sorted(expected - recipes)}, "
            f"unconfigured recipes={sorted(recipes - expected)}"
        )
    sdk = match_one(r"^\s+SDK_ARCH: ([a-z0-9_-]+)-" + VERSION + "$", workflow)
    architecture, current_openwrt = sdk.group(1), sdk.group(2)
    rust_version = match_one(r"^\s+RUST_VERSION: " + VERSION + "$", workflow).group(1)
    new_openwrt = current_openwrt if replay or lane == "tailscale" else latest_openwrt(current_openwrt)
    sdk_changed = version(new_openwrt) > version(current_openwrt)
    if sdk_changed and not sdk_available(architecture, new_openwrt):
        print(f"OpenWrt {new_openwrt} SDK is not published yet; retaining {current_openwrt}")
        sdk_changed = False
    changes = {}
    summary = []
    for name in sorted(recipes):
        if replay and name != replay:
            continue
        make_path = Path("package") / name / "Makefile"
        makefile = (root / make_path).read_text(encoding="utf-8")
        original = makefile
        if match_one(r"^PKG_NAME:=(.+)$", makefile).group(1) != name:
            raise ValueError(f"Package name does not match its folder: {name}")
        current = match_one(r"^PKG_VERSION:=" + VERSION + "$", makefile).group(1)
        revision = int(match_one(r"^PKG_RELEASE:=(\d+)$", makefile).group(1))
        project = PROJECTS[name]
        match_one(r"^PKG_HASH:=[0-9a-f]{64}$", makefile)
        pinned = match_one(r"^PKG_SOURCE_COMMIT:=(" + COMMIT + ")$", makefile).group(1)
        update_package = not lane or (lane == "tailscale") == (name == "tailscale-rs")
        tag = latest_tag(project["repository"]) if update_package and not replay else f"v{current}"
        release = tag.removeprefix("v")
        resolved = release_commit(project["repository"], tag) if update_package and not replay else pinned
        if version(release) == version(current) and resolved != pinned:
            raise ValueError(f"Published {project['repository']} tag {tag} changed commit")
        if replay or version(release) > version(current):
            expected_url = f"https://codeload.github.com/{project['repository']}/tar.gz/{resolved}"
            source_url = match_one(r"^PKG_SOURCE_URL:=(https://[^\s?]+)$", makefile).group(1)
            source_file = match_one(r"^PKG_SOURCE_URL_FILE:=(\S+)$", makefile).group(1)
            configured_url = f"{source_url}/{source_file}".replace("$(PKG_SOURCE_COMMIT)", resolved)
            if configured_url != expected_url:
                raise ValueError(f"Release tag does not match {name}'s source URL: {tag}")
            digest, lock, edits = updated_source(root, name, tag, cargo, rust_version, resolved, model_key)
            if replay and digest != match_one(r"^PKG_HASH:=([0-9a-f]{64})$", makefile).group(1):
                raise ValueError(f"Pinned replay source checksum changed: {name}")
            changes.update(edits)
            makefile = makefile.replace(f"PKG_SOURCE_COMMIT:={pinned}", f"PKG_SOURCE_COMMIT:={resolved}")
            makefile = re.sub(r"^PKG_VERSION:=" + VERSION + "$", "PKG_VERSION:=" + release, makefile, flags=re.MULTILINE)
            makefile = re.sub(r"^PKG_HASH:=[0-9a-f]{64}$", "PKG_HASH:=" + digest, makefile, flags=re.MULTILINE)
            next_revision = revision + 1 if replay else 1
            makefile = re.sub(r"^PKG_RELEASE:=\d+$", f"PKG_RELEASE:={next_revision}", makefile, flags=re.MULTILINE)
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


def refresh_tailscale_lock(root, cargo, dependencies):
    package = "tailscale-rs"
    recipe = root / "package" / package / "Makefile"
    text = recipe.read_text(encoding="utf-8")
    release = match_one(r"^PKG_VERSION:=" + VERSION + "$", text).group(1)
    revision = match_one(r"^PKG_SOURCE_COMMIT:=(" + COMMIT + ")$", text).group(1)
    digest = match_one(r"^PKG_HASH:=([0-9a-f]{64})$", text).group(1)
    workflow = (root / WORKFLOW).read_text(encoding="utf-8")
    rust = match_one(r"^\s+RUST_VERSION: " + VERSION + "$", workflow).group(1)
    _, updated, _ = updated_source(root, package, f"v{release}", cargo, rust, revision,
                                  lock_updates=dependencies, expected_digest=digest)
    lock = recipe.parent / "files/Cargo.lock"
    if lock.read_bytes().replace(b"\r\n", b"\n") == updated.replace(b"\r\n", b"\n"):
        return
    number = match_one(r"^PKG_RELEASE:=(\d+)$", text).group(1)
    lock.write_bytes(updated.replace(b"\r\n", b"\n"))
    recipe.write_text(text.replace(f"PKG_RELEASE:={number}\n", f"PKG_RELEASE:={int(number) + 1}\n"),
                      encoding="utf-8", newline="\n")


def lock_update(value):
    match = re.fullmatch(r"([a-zA-Z0-9_-]+(?:@\d+\.\d+\.\d+)?)=(\d+\.\d+\.\d+)", value)
    if not match:
        raise argparse.ArgumentTypeError("use dependency[@old-version]=new-version")
    return match.groups()


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, default=Path(__file__).resolve().parents[2])
    parser.add_argument("--cargo", default="cargo")
    parser.add_argument("--replay", choices=sorted(PROJECTS), default="")
    parser.add_argument("--lane", choices=("standard", "tailscale"), default="")
    parser.add_argument("--refresh-tailscale-lock", action="append", type=lock_update)
    args = parser.parse_args()
    if args.refresh_tailscale_lock:
        if args.lane or args.replay:
            parser.error("a lockfile refresh cannot be combined with a release check")
        refresh_tailscale_lock(args.root, args.cargo, args.refresh_tailscale_lock)
    else:
        update(args.root, args.cargo, os.environ.pop("OPENCODE_API_KEY", ""), args.replay, args.lane)
