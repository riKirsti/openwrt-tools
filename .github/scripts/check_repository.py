"""Fail CI on private data or broken local links in project files."""

from pathlib import Path
import re
import subprocess
from urllib.parse import unquote, urlsplit


ROOT = Path(__file__).resolve().parents[2]
EMAIL = re.compile(r"\b[A-Z0-9._%+-]+@([A-Z0-9.-]+\.[A-Z]{2,})\b", re.I)
EXAMPLE_DOMAINS = {"example.com", "example.net", "example.org", "users.noreply.github.com"}
PRIVATE_TEXT = {
    "local profile path": re.compile(r"\b[A-Z]:[\\/]Users[\\/][^\s\"<>]+|/(?:Users|home)/[^/\s\"<>]+", re.I),
    "private key or access token": re.compile(
        r"-----BEGIN (?:[A-Z]+ )*PRIVATE KEY-----|\b(?:gh[pousr]_[A-Za-z0-9]{36,}|github_pat_[A-Za-z0-9_]{40,})\b"
    ),
}
PRIVATE_SUFFIXES = {".key", ".p12", ".pfx", ".pcap", ".pcapng"}


def problems(path, text):
    for kind, pattern in PRIVATE_TEXT.items():
        if pattern.search(text):
            yield kind
    if any(match[1].lower() not in EXAMPLE_DOMAINS for match in EMAIL.finditer(text)):
        yield "non-example email"
    if path.suffix != ".md":
        return
    prose = re.sub(r"```.*?```", "", text, flags=re.S)
    for link in re.findall(r"!?\[[^\]]*\]\(([^)]+)\)", prose):
        target = urlsplit(link.strip().split(' "', 1)[0].strip("<>"))
        if not target.scheme and not target.netloc and target.path:
            resolved = (path.parent / unquote(target.path)).resolve()
            if not resolved.is_relative_to(ROOT) or not resolved.exists():
                yield "missing or outside-repository Markdown link"


def main():
    names = subprocess.check_output(["git", "ls-files", "-z"], cwd=ROOT).decode().split("\0")
    failed = False
    for name in filter(None, names):
        path = ROOT / name
        if path.parent == ROOT / "build" and path.suffix == ".apk":
            continue
        if path.suffix in PRIVATE_SUFFIXES:
            issues = ["private artifact in the source tree"]
        else:
            try:
                issues = list(problems(path, path.read_text(encoding="utf-8")))
            except UnicodeError:
                issues = ["unexpected binary in the source tree"]
        for issue in issues:
            print(f"{name}: {issue}")
            failed = True
    if failed:
        raise SystemExit(1)
    print("Project privacy and documentation checks passed.")


if __name__ == "__main__":
    main()
