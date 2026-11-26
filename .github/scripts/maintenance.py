"""Collect maintenance proposals from disposable checkouts for read-only qualification."""

import hashlib
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import time

from adapt_protunnext import files
from check_repository import problems
import opencode_ci


TASKS = {
    "performance": "Propose a measurable build-time or APK-size improvement for either package or the feed.",
    "refactor": "Propose a focused refactor that preserves existing package behavior and interfaces.",
    "cleanup": "Propose cleanup of redundant package code or outdated development, build or deployment docs.",
}
WORKLOADS = {"cold_build_seconds": {"feed"}, "warm_build_seconds": {"feed"},
             "apk_bytes": {"protun-openwrt", "tailscale-rs"}}
LIMIT = 256 * 1024


def git(work, *args):
    return subprocess.check_output(["git", *args], cwd=work)


def clean_text(path, data):
    text = data.decode("utf-8")
    if "\0" in text or list(problems(path, text)):
        raise ValueError(f"Invalid text or privacy finding in {path}")
    return text


def workload(notes):
    scopes = re.findall(r"(?m)^Scope: ([a-z0-9_-]+)$", notes)
    metrics = re.findall(r"(?m)^Metric: ([a-z0-9_-]+)$", notes)
    if len(scopes) == len(metrics) == 1 and scopes[0] in WORKLOADS.get(metrics[0], ()):
        return metrics[0], scopes[0]
    return "", ""


def propose(work, stage, out, key):
    if not key:
        raise ValueError("Hosted inference key unavailable")
    editable = [path.relative_to(work).as_posix()
                for root in (work / "package", work / "docs")
                for path in sorted(root.rglob("*")) if path.is_file()]
    notes_path = work / ".maintenance-notes.md"
    benchmark = work / ".benchmark-proposal.md"
    prompt = f"""You are running an OpenWrt feed maintenance preview in GitHub Actions.
Read AGENTS.md and follow its package guidance. {TASKS[stage]}
Verify findings against available official upstream evidence and identify any gaps.
Write short findings to {notes_path.name}. Record whether you changed anything.
For performance, declare Scope: feed, protun-openwrt, or tailscale-rs and
Metric: cold_build_seconds, warm_build_seconds, or apk_bytes on separate lines,
with exactly one value per field. Use feed for build times and a package for APK size.
If the workload requires a new benchmark, describe the procedure, metric,
baseline and expected result in {benchmark.name} for maintainer review.
Preserve package pins and runtime interfaces. Avoid personal data.
"""
    opencode_ci.run(key, prompt, work, (*editable, "README.md", notes_path.name, benchmark.name))
    # Scan regular files before Git stages the proposal. The checkout is disposable.
    for note in (notes_path, benchmark):
        if note.is_symlink():
            raise ValueError("Proposal notes cannot be symlinks")
    notes = clean_text(Path(notes_path.name), notes_path.read_bytes()) if notes_path.is_file() else ""
    if notes:
        (out / "findings.md").write_text(notes, encoding="utf-8")
        notes_path.unlink()
    if benchmark.is_file():
        data = clean_text(Path(benchmark.name), benchmark.read_bytes())
        (out / "benchmark-review.md").write_text(data, encoding="utf-8")
        benchmark.unlink()
    for path in (work / "package", work / "docs"):
        files(path)
    git(work, "add", "-A")
    git(work, "diff", "--cached", "--check")
    patch = git(work, "diff", "--cached", "--binary", "--no-renames")
    if not patch:
        return "benchmark awaiting review" if (out / "benchmark-review.md").is_file() else "no change"
    if len(patch) > LIMIT:
        raise ValueError("Proposal patch exceeds artifact size limit")
    scripts = Path(__file__).resolve().parent
    subprocess.run([sys.executable, str(scripts / "validate-candidate.py"), "maintenance"], cwd=work, check=True)
    # Use the copy of the checker in the detached checkout. Candidate scope excludes it.
    subprocess.run([sys.executable, str(work / ".github/scripts/check_repository.py")], cwd=work, check=True)
    for path in git(work, "diff", "--cached", "--name-only", "-z").decode().split("\0"):
        source = work / path
        if not path or not source.is_file():
            continue
        if source.suffix == ".py":
            compile(source.read_bytes(), str(source), "exec")
        elif source.suffix in {".sh", ".init"}:
            subprocess.run(["bash", "-n", str(source)], check=True)
    metric, scope = workload(notes) if stage == "performance" else ("", "")
    if stage == "performance" and (not metric or (out / "benchmark-review.md").is_file()):
        (out / "performance-review.patch").write_bytes(patch)
        return "benchmark awaiting review"
    (out / "update.patch").write_bytes(patch)
    with Path(os.environ["GITHUB_OUTPUT"]).open("a", encoding="utf-8") as output:
        print(f"sha={hashlib.sha256(patch).hexdigest()}", file=output)
        print(f"metric={metric}", file=output)
        print(f"scope={scope}", file=output)
    return "candidate ready for qualification"


def main():
    stage = sys.argv[1]
    root = Path.cwd()
    out = Path(os.environ["RUNNER_TEMP"]) / "maintenance-review"
    out.mkdir()
    key = os.environ.pop("OPENCODE_API_KEY", "")
    status = "proposal failed"
    started = time.monotonic()
    try:
        with tempfile.TemporaryDirectory(dir=out.parent) as directory:
            work = Path(directory) / "checkout"
            git(root, "worktree", "add", "--detach", str(work), "HEAD")
            try:
                status = propose(work, stage, out, key)
            finally:
                git(root, "worktree", "remove", "--force", str(work))
    finally:
        with Path(os.environ["GITHUB_STEP_SUMMARY"]).open("a", encoding="utf-8") as summary:
            print(f"{stage}: {status} ({time.monotonic() - started:.1f}s)", file=summary)


if __name__ == "__main__":
    main()
