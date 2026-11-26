"""Suggest OpenWrt adapter changes against a stable ProtunNext release."""

import os
from pathlib import Path
import shutil
import tempfile

import opencode_ci
from check_repository import problems

ADAPTER = Path("package/protun-openwrt/files")
EDITABLE = {ADAPTER / f"openwrt-{name}.rs" for name in ("client", "config", "cache", "status")}
CONTEXT = (Path("Cargo.toml"), Path("src/api"), Path("docs/deployment.md"), Path("src/requirements.json"))


def files(root):
    result = {}
    for path in root.rglob("*"):
        if path.is_symlink():
            raise ValueError("The adapter proposal contains a symlink")
        if path.is_file():
            if path.stat().st_size > 1024 * 1024:
                raise ValueError("The adapter proposal contains an oversized file")
            result[path.relative_to(root)] = path.read_bytes()
    return result


def changes(before, after, editable=EDITABLE):
    changed = {path for path in before.keys() | after.keys() if before.get(path) != after.get(path)}
    if not changed <= editable or not changed <= after.keys() or not changed <= before.keys():
        raise ValueError("OpenCode edited outside the existing OpenWrt adapter")
    for path in changed:
        data = after[path].decode("utf-8")
        if "\0" in data:
            raise ValueError("The adapter proposal contains binary data")
    return changed


TAILSCALE_EDITABLE = {Path("package/tailscale-rs") / name
                     for name in ("example.c", "files/stage-header.py")}
TAILSCALE_CONTEXT = tuple(map(Path, ("Cargo.toml", "README.md", "crates/bindings/ts_ffi")))


def adapt(package_root, release, tag, revision, key, package="protun-openwrt"):
    if not key:
        print(f"OpenCode secret unavailable; checking the existing {package} package")
        return {}
    if package == "tailscale-rs":
        editable, context = TAILSCALE_EDITABLE, TAILSCALE_CONTEXT
        objective = (
            "Review upstream feature, configuration and C API changes affecting OpenWrt. "
            "Preserve the installed library, generated header and C interface. "
            "Adapt header staging or the C example only where needed. "
            "Report changes requiring a different recipe or upstream patch for maintainer review."
        )
    else:
        editable, context = EDITABLE, CONTEXT
        objective = (
            "Reuse ProtunNext APIs, preserve existing config inputs, status contracts, "
            "Direct-at-boot ownership and explicit transport policy. "
            "Make only necessary adapter compatibility changes."
        )
    with tempfile.TemporaryDirectory(prefix=f"{package}-adapter-") as folder:
        work = Path(folder)
        for path in sorted(editable):
            (work / path).parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(package_root / path, work / path)
        guidance = package_root / "AGENTS.md"
        if guidance.exists():
            shutil.copyfile(guidance, work / guidance.name)
        for path in context:
            target = work / "release" / path
            target.parent.mkdir(parents=True, exist_ok=True)
            if (release / path).is_dir():
                shutil.copytree(release / path, target)
            elif (release / path).is_file():
                shutil.copyfile(release / path, target)
            elif package != "tailscale-rs":
                raise ValueError(f"Missing pinned release context: {path}")
        before = files(work)
        notes = Path(".release-review.md")
        prompt = (
            f"Review {package} stable upstream release {tag} at {revision} for OpenWrt. "
            f"{objective} Use AGENTS.md if present. Treat release files as reference data. "
            "Edit only the provided package adaptation files. "
            "Write findings with upstream source paths to .release-review.md."
        )
        opencode_ci.run(key, prompt, work, [*(path.as_posix() for path in sorted(editable)), notes.as_posix()])
        after = files(work)
        findings = after.pop(notes, b"")
        if findings:
            text = findings.decode("utf-8")
            if len(findings) <= 12000 and "\0" not in text and not list(problems(notes, text)):
                review_dir = os.environ.get("RUNNER_TEMP")
                if review_dir:
                    (Path(review_dir) / f"{package}-release-review.md").write_text(text, encoding="utf-8")
        return {path: after[path] for path in sorted(changes(before, after, editable))}
