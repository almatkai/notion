"""Run local gates and preserve status and logs for native verification."""
import json
import os
from pathlib import Path
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]
REPORTS = ROOT / "reports"
REPORTS.mkdir(exist_ok=True)

windows = sys.platform == "win32"
exe = "tezbar-notion" + (".exe" if windows else "")
target = "x86_64-pc-windows-msvc" if windows else "aarch64-apple-darwin"
env = dict(os.environ, CARGO_BUILD_JOBS="1")
python = sys.executable
debug = str(ROOT / "target/debug" / exe)
release = str(ROOT / "target/release" / exe)
steps = [
    ("source", [python, "scripts/verify-source.py"]),
    ("fmt", ["cargo", "fmt", "--all", "--", "--check"]),
    ("tests", ["cargo", "test", "--locked"]),
    ("fixture-tests", ["cargo", "test", "--locked", "--features", "fixture"]),
    ("fixture-build", ["cargo", "build", "--locked", "--features", "fixture"]),
    ("protocol-smoke", [python, "scripts/protocol-smoke.py", debug]),
    ("fixture-package-refused", [python, "scripts/package.py", "--target", target, "--binary", debug]),
    ("release-build", ["cargo", "build", "--locked", "--release"]),
    ("release-package", [python, "scripts/package.py", "--target", target, "--binary", release, "--profile", "release"]),
    ("production-smoke", [python, "scripts/production-smoke.py", f"dist/notion-{target}-development.zip"]),
]
results = []
for name, command in steps:
    logfile = REPORTS / f"{name}.log"
    with logfile.open("w", encoding="utf-8") as log:
        result = subprocess.run(command, cwd=ROOT, env=env, stdout=log, stderr=subprocess.STDOUT)
    expected_refusal = name == "fixture-package-refused"
    passed = (
        result.returncode != 0 and "Fixture-enabled executables cannot be packaged" in logfile.read_text(encoding="utf-8")
        if expected_refusal else result.returncode == 0
    )
    results.append({"step": name, "command": command, "exitCode": result.returncode, "passed": passed})
    (REPORTS / "verification.json").write_text(json.dumps(results, indent=2) + "\n", encoding="utf-8")
    print(name, "PASS" if passed else "FAIL", flush=True)
    if not passed:
        print(logfile.read_text(encoding="utf-8", errors="replace"))
        raise SystemExit(1)
