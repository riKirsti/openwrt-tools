"""Run the pinned inference CLI and verify the model before accepting edits."""

import json
import os
from pathlib import Path
import subprocess
from tempfile import TemporaryDirectory


MANIFEST = Path(__file__).resolve().parents[1] / "opencode/package.json"
MODEL = json.loads(MANIFEST.read_text(encoding="utf-8"))["config"]["model"]


def configuration(editable):
    def rule(action, resource, effect):
        return {"action": action, "resource": resource, "effect": effect}

    return {
        "$schema": "https://opencode.ai/config.json",
        "model": MODEL,
        "agents": {"title": {"model": MODEL}, "summary": {"model": MODEL},
                   "compaction": {"model": MODEL}},
        "update": "disable",
        "share": "disabled",
        "permissions": [
            rule("*", "*", "deny"),
            *(rule(action, "*", "allow") for action in ("read", "glob", "grep")),
            rule("read", ".git*", "deny"),
            rule("read", "*.env", "deny"),
            rule("read", "*.env.*", "deny"),
            *(rule("edit", path, "allow") for path in editable),
        ],
        "policies": [
            rule("provider.use", "*", "deny"),
            rule("provider.use", "opencode", "allow"),
            *(rule(f"{name}.use", "*", "deny") for name in ("plugin", "mcp", "skill", "websearch")),
            *(rule("tool.use", f"{action}:*", "deny")
              for action in ("shell", "subagent", "external_directory", "webfetch", "execute")),
        ],
    }


def completed_session(output):
    events = [json.loads(line) for line in output.splitlines() if line.strip()]
    for event in events:
        if event["type"] == "error":
            category = event.get("error", {}).get("type", "")
            if category not in {"provider.no-route", "provider.rate-limit", "provider.authentication",
                                "provider.error", "session.permission", "unknown"}:
                category = "unclassified"
            raise ValueError(f"OpenCode inference error category: {category}")
    sessions = {event["sessionID"] for event in events if event["type"] == "step_finish"}
    if len(sessions) != 1 or not next(iter(sessions)):
        raise ValueError("OpenCode did not finish an identifiable session")
    return sessions.pop()


def verify_models(output, session):
    exported = json.loads(output)
    provider, model = MODEL.split("/")
    if exported["info"]["id"] != session:
        raise ValueError("Unexpected OpenCode session export")
    assistants = [message for message in exported["messages"] if message["type"] == "assistant"]
    if not assistants:
        raise ValueError("OpenCode session contained no completed assistant turns")
    for message in assistants:
        actual = message.get("model", {})
        if actual.get("providerID") != provider or actual.get("id") != model:
            raise ValueError("OpenCode assistant turn used a different provider or model")
        if message.get("error"):
            raise ValueError("OpenCode session contained an unsuccessful assistant turn")


def run(key, prompt, work, editable):
    config_path = work / "opencode.json"
    if (work / "opencode.jsonc").exists():
        raise ValueError("Unexpected project OpenCode configuration")
    executable = os.environ.get("OPENCODE_CLI", "opencode")
    with TemporaryDirectory(prefix="opencode-ci-") as folder, config_path.open("x", encoding="utf-8") as config:
        try:
            json.dump(configuration(editable), config)
            config.close()
            child = {name: os.environ[name] for name in
                     ("PATH", "SYSTEMROOT", "WINDIR", "TEMP", "TMP", "CI", "LANG")
                     if name in os.environ}
            child.update({f"XDG_{name}_HOME": str(Path(folder) / name.lower())
                          for name in ("CONFIG", "DATA", "CACHE", "STATE")})
            child["OPENCODE_DISABLE_CLAUDE_CODE"] = "true"
            child["OPENCODE_DISABLE_FREE_MODELS"] = "true"
            invocation = {"cwd": work, "stdout": subprocess.PIPE, "stderr": subprocess.DEVNULL,
                          "encoding": "utf-8", "check": False}
            execution = subprocess.run(
                [executable, "run", "--standalone", "--format", "json", "--agent", "build",
                 "--model", MODEL, "--", prompt],
                env={**child, "OPENCODE_API_KEY": key}, stdin=subprocess.DEVNULL, **invocation)
            session = completed_session(execution.stdout)
            if execution.returncode:
                raise ValueError("OpenCode inference exited unsuccessfully")
            print("OpenCode inference completed; verifying the session export.", flush=True)
            exported = subprocess.run(
                [executable, "session", "export", "--standalone", "--sanitize", session],
                env=child, stdin=subprocess.DEVNULL, **invocation)
            if exported.returncode:
                raise ValueError("OpenCode could not export the private session")
            verify_models(exported.stdout, session)
        finally:
            config_path.unlink()
