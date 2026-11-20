"""Run the pinned OpenCode CLI and verify the model before accepting edits."""

import json
import os
from pathlib import Path
import subprocess
from tempfile import TemporaryDirectory


MODEL = "opencode/deepseek-v4.1-flash"


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
    if any(event["type"] == "error" for event in events):
        raise ValueError("OpenCode reported an error")
    sessions = {event["sessionID"] for event in events if event["type"] == "step_finish"}
    if len(sessions) != 1 or not next(iter(sessions)):
        raise ValueError("OpenCode did not finish an identifiable session")
    return sessions.pop()


def verify_models(output, session):
    exported = json.loads(output)
    provider, model = MODEL.split("/")
    expected = {"providerID": provider, "id": model}
    if exported["info"]["id"] != session:
        raise ValueError("Unexpected OpenCode session export")
    assistants = [message for message in exported["messages"] if message["type"] == "assistant"]
    if not assistants or any(message.get("model") != expected or message.get("error")
                             for message in assistants):
        raise ValueError("OpenCode did not complete with the configured model")


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
                          "encoding": "utf-8", "check": True}
            execution = subprocess.run(
                [executable, "run", "--standalone", "--format", "json", "--agent", "build",
                 "--model", MODEL, "--", prompt],
                env={**child, "OPENCODE_API_KEY": key}, stdin=subprocess.DEVNULL, **invocation)
            session = completed_session(execution.stdout)
            exported = subprocess.run(
                [executable, "session", "export", "--standalone", "--sanitize", session],
                env=child, stdin=subprocess.DEVNULL, **invocation)
            verify_models(exported.stdout, session)
        finally:
            config_path.unlink()
