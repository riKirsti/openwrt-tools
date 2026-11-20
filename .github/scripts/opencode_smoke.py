"""Exercise the real locked CLI on harmless files without creating a proposal."""

import os
from pathlib import Path
import subprocess
from tempfile import TemporaryDirectory

import opencode_ci


def main():
    key = os.environ.pop("OPENCODE_API_KEY", "")
    if not key:
        raise SystemExit("OpenCode smoke requires the repository inference key.")
    with TemporaryDirectory(prefix="opencode-smoke-") as folder:
        root = Path(folder)
        project = root / "project"
        project.mkdir()
        (project / "allowed.txt").write_text("before\n", encoding="utf-8")
        (project / "protected.txt").write_text("preserve\n", encoding="utf-8")
        (root / "outside.txt").write_text("outside\n", encoding="utf-8")
        opencode_ci.run(
            key, "Change only allowed.txt to one line containing 'verified' using the edit tool. "
            "Read it back to verify. Do not touch any other file.", project, ("allowed.txt",),
        )
        if ((project / "allowed.txt").read_text(encoding="utf-8") != "verified\n"
                or (project / "protected.txt").read_text(encoding="utf-8") != "preserve\n"
                or (root / "outside.txt").read_text(encoding="utf-8") != "outside\n"
                or {path.name for path in project.iterdir()} != {"allowed.txt", "protected.txt"}):
            raise ValueError("The synthetic edit was missing or outside its scope")
        print(f"OpenCode smoke passed with verified model {opencode_ci.MODEL}.")


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, TypeError, KeyError, subprocess.CalledProcessError):
        raise SystemExit("OpenCode smoke failed. No proposal or release was created.") from None
