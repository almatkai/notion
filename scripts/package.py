"""Inventory and package a local build of the standalone extension."""
import argparse
import hashlib
import json
from pathlib import Path
import stat
import subprocess
import sys
import zipfile

ROOT = Path(__file__).resolve().parents[1]
SDK_REV = "1a11afc14693cc6463e0b95ae7d61991b4a60427"


def digest(contents):
    return hashlib.sha256(contents).hexdigest()


def report(binary, flag):
    return json.loads(subprocess.check_output([str(binary), flag], timeout=15))


def check_architecture(contents, windows):
    if windows:
        offset = int.from_bytes(contents[60:64], "little")
        valid = contents[:2] == b"MZ" and contents[offset:offset + 6] == b"PE\0\0d\x86"
    else:
        valid = contents[:4] == bytes.fromhex("cffaedfe") and int.from_bytes(contents[4:8], "little") == 0x0100000C
    if not valid:
        raise SystemExit("Executable machine architecture is incompatible with the requested target.")


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--target", choices=["x86_64-pc-windows-msvc", "aarch64-apple-darwin"], required=True)
    parser.add_argument("--profile", choices=["debug", "release"], default="debug")
    args = parser.parse_args()
    binary = args.binary.resolve()
    build = report(binary, "--build-info")
    if build.get("fixture") is not False:
        raise SystemExit("Fixture-enabled executables cannot be packaged.")
    identity = report(binary, "--protocol-info")
    manifest = json.loads((ROOT / "extension.json").read_text(encoding="utf8"))
    assert (identity["id"], identity["version"], identity["protocol"]) == (manifest["id"], manifest["version"], 1)
    assert identity["commands"] == [command["name"] for command in manifest["commands"]]
    assert f'#{SDK_REV}"' in (ROOT / "Cargo.lock").read_text(encoding="utf8")
    subprocess.run([sys.executable, str(ROOT / "scripts/vendor-licenses.py"), "--check"], cwd=ROOT, check=True)

    windows = args.target == "x86_64-pc-windows-msvc"
    exe = "bin/tezbar-notion" + (".exe" if windows else "")
    contents = binary.read_bytes()
    check_architecture(contents, windows)
    payload = {exe: contents, "LICENSE": (ROOT / "LICENSE").read_bytes(), "NOTICE": (ROOT / "NOTICE").read_bytes()}
    for folder in ["resources", "licenses"]:
        for path in (ROOT / folder).rglob("*"):
            assert not path.is_symlink()
            if path.is_file():
                payload[path.relative_to(ROOT).as_posix()] = path.read_bytes()
    manifest["files"] = {name: digest(contents) for name, contents in sorted(payload.items())}
    manifest["artifacts"] = {}
    assert manifest["icon"] in payload
    payload["extension.json"] = (json.dumps(manifest, indent=2, ensure_ascii=False) + "\n").encode("utf8")
    out = ROOT / "dist"
    out.mkdir(exist_ok=True)
    archive = out / f"notion-{args.target}-development.zip"
    with zipfile.ZipFile(archive, "w", zipfile.ZIP_DEFLATED) as zipped:
        for name, data in sorted(payload.items()):
            entry = zipfile.ZipInfo(name, date_time=(2026, 1, 1, 0, 0, 0))
            entry.create_system = 3
            entry.external_attr = (stat.S_IFREG | (0o755 if name == exe else 0o644)) << 16
            entry.compress_type = zipfile.ZIP_DEFLATED
            zipped.writestr(entry, data)
    with zipfile.ZipFile(archive) as zipped:
        assert zipped.testzip() is None and set(zipped.namelist()) == set(payload)
        assert all(digest(zipped.read(name)) == sha for name, sha in manifest["files"].items())
        assert stat.S_IMODE(zipped.getinfo(exe).external_attr >> 16) == 0o755
    artifact = {
        "target": args.target, "profile": args.profile, "archive": str(archive),
        "sha256": digest(archive.read_bytes()), "executable": exe,
        "executableSha256": manifest["files"][exe], "files": manifest["files"],
        "developmentOnly": True, "desktopAcceptance": "not-run",
    }
    (out / (archive.name + ".artifact.json")).write_text(json.dumps(artifact, indent=2) + "\n", encoding="utf8")
    (out / (archive.name + ".sha256")).write_text(artifact["sha256"] + "  " + archive.name + "\n", encoding="utf8")
    print(json.dumps({key: value for key, value in artifact.items() if key != "files"}, indent=2))


if __name__ == "__main__":
    main()
