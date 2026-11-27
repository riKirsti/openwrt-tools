"""Compare the pinned prepare and health commands against identical controller fixtures."""
import importlib.util
import json
from pathlib import Path
from statistics import median
import sys
from time import monotonic_ns

fixture = Path(__file__).with_name("test_controller_contract.py")
spec = importlib.util.spec_from_file_location("controller_fixture", fixture)
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)
roots = dict(zip(("baseline", "candidate"), map(Path, sys.argv[1:3]), strict=True))

def sample(label):
    module.CONTROLLER = roots[label] / "package/protun-openwrt/files/controller"
    test = module.ControllerContract()
    test.setUp()
    try:
        measurements = {}
        for command in ("prepare", "health"):
            if command == "health":
                test.ready_worker()
            log = test.root / "ip.log"
            if log.exists():
                log.unlink()
            started = monotonic_ns()
            result = test.invoke(command)
            measurements[command + "_ms"] = (monotonic_ns() - started) / 1e6
            if result.returncode:
                raise RuntimeError(result.stderr)
            measurements[command + "_route_writes"] = sum(
                " route replace " in line for line in log.read_text().splitlines()
            )
        return measurements
    finally:
        test.doCleanups()

sample("baseline")
sample("candidate")
aa, ab = [], []
for _ in range(4):
    aa.append((sample("baseline"), sample("baseline")))
    ab.append((sample("baseline"), sample("candidate")))
    right, left = sample("candidate"), sample("baseline")
    ab.append((left, right))
results = {}
for key in aa[0][0]:
    left = median(pair[0][key] for pair in ab)
    right = median(pair[1][key] for pair in ab)
    result = {"baseline": left, "candidate": right}
    if key.endswith("_ms"):
        control_spread = max(abs(y[key] / x[key] - 1) for x, y in aa)
        effects = sorted(y[key] / x[key] - 1 for x, y in ab)
        result.update(aa_spread=control_spread, paired_median=median(effects),
                      middle_six=[effects[1], effects[-2]],
                      decision=("inconclusive" if control_spread > .05 else
                                "regression" if effects[1] > .05 else
                                "inconclusive" if effects[-2] > .05 else "passed"))
    results[key] = result
print(json.dumps({"results": results, "aa": aa, "ab": ab}, indent=2))
