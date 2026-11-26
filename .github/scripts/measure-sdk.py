"""Measure the checked OpenWrt SDK build with separate baseline and candidate caches."""

import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time


METRICS = {"cold_build_seconds", "warm_build_seconds", "apk_bytes"}
PACKAGES = {"protun-openwrt", "tailscale-rs"}


def decision(metric, scope, pairs):
    baseline = [sample[0][metric] if metric != "apk_bytes" else sample[0][metric][scope] for sample in pairs]
    candidate = [sample[1][metric] if metric != "apk_bytes" else sample[1][metric][scope] for sample in pairs]
    if metric == "apk_bytes" and (len(set(baseline)) != 1 or len(set(candidate)) != 1):
        return "inconclusive"
    if max(candidate) < min(baseline):
        return "improvement observed"
    if min(candidate) > max(baseline):
        return "regression observed"
    return "no repeatable improvement"


def measure(work, scratch, script):
    scratch.mkdir()
    env = {**os.environ, "GITHUB_WORKSPACE": str(work), "RUNNER_TEMP": str(scratch)}
    result = {}
    try:
        for metric in ("cold_build_seconds", "warm_build_seconds"):
            start = time.monotonic()
            subprocess.run(["bash", str(script)], cwd=work, env=env, check=True)
            result[metric] = time.monotonic() - start
    finally:
        subprocess.run(["sudo", "chown", "-R", f"{os.getuid()}:{os.getgid()}", str(scratch)], check=True)
    packages = {}
    for path in (scratch / "tools-apk").glob("*.apk"):
        for package in PACKAGES:
            if path.name.startswith(f"{package}-"):
                packages[package] = path.stat().st_size
    if packages.keys() != PACKAGES:
        raise ValueError("Expected both APKs from the benchmark")
    result["apk_bytes"] = packages
    return result


def main():
    metric, scope = sys.argv[1:]
    if metric not in METRICS or (metric == "apk_bytes" and scope not in PACKAGES) or (
            metric != "apk_bytes" and scope != "feed"):
        raise ValueError("Unknown benchmark or scope")
    candidate = Path.cwd()
    runner = Path(os.environ["RUNNER_TEMP"])
    with tempfile.TemporaryDirectory(dir=runner, prefix="sdk-comparison-") as directory:
        parent = Path(directory)
        baseline = parent / "baseline"
        subprocess.run(["git", "worktree", "add", "--detach", str(baseline), "HEAD"], check=True)
        try:
            script = baseline / ".github/scripts/run-sdk.sh"
            pairs = []
            for round_number, order in enumerate((("base", "candidate"), ("candidate", "base"))):
                samples = {}
                for arm in order:
                    work = baseline if arm == "base" else candidate
                    samples[arm] = measure(work, parent / f"{round_number}-{arm}", script)
                pairs.append((samples["base"], samples["candidate"]))
        finally:
            subprocess.run(["git", "worktree", "remove", "--force", str(baseline)], check=True)
    outcomes = {name: decision("apk_bytes", name, pairs) for name in PACKAGES}
    outcomes.update({name: decision(name, "feed", pairs) for name in METRICS - {"apk_bytes"}})
    verdict = decision(metric, scope, pairs)
    if verdict == "improvement observed" and "regression observed" in outcomes.values():
        verdict = "regression observed"
    evidence = {"metric": metric, "scope": scope, "pairs": pairs, "outcomes": outcomes, "decision": verdict}
    (runner / "tools-apk" / "benchmark.json").write_text(json.dumps(evidence, indent=2), encoding="utf-8")
    with Path(os.environ["GITHUB_STEP_SUMMARY"]).open("a", encoding="utf-8") as summary:
        print(f"Performance proposal ({scope}, {metric}) — {verdict}", file=summary)


if __name__ == "__main__":
    main()
