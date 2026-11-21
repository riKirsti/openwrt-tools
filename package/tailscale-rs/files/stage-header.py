"""Stage the header from Cargo's build output, including cached builds."""

import json
from pathlib import Path
import shutil
import sys


messages, crate, destination = map(Path, sys.argv[1:])
crate_id = crate.resolve().as_uri()
with messages.open(encoding="utf-8") as stream:
    cargo = [json.loads(line) for line in stream if line.startswith("{")]
outputs = {
    Path(message["out_dir"])
    for message in cargo
    if message["reason"] == "build-script-executed"
    and message["package_id"].partition("#")[0].removeprefix("path+") == crate_id
}
if len(outputs) != 1:
    raise SystemExit(f"Expected one Cargo build output for {crate}, found {len(outputs)}")
shutil.copyfile(outputs.pop() / "tailscale.h", destination)
