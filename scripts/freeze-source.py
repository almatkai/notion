"""Freeze the portable port source, immutable upstream source and notices."""
import hashlib
import json
from pathlib import Path
import stat
import zipfile

ROOT = Path(__file__).resolve().parents[1]
EXCLUDED = {"target", "dist", ".git", "__pycache__", "reports"}


def digest(content):
    return hashlib.sha256(content).hexdigest()


def main():
    out = ROOT / "dist"
    out.mkdir(exist_ok=True)
    payload = {}
    for path in ROOT.rglob("*"):
        relative = path.relative_to(ROOT)
        if EXCLUDED.intersection(relative.parts):
            continue
        if path.is_file():
            assert not path.is_symlink()
            payload[relative.as_posix()] = path.read_bytes()

    archive = out / "notion-source-freeze.zip"
    with zipfile.ZipFile(archive, "w", zipfile.ZIP_DEFLATED) as zipped:
        for name, data in sorted(payload.items()):
            entry = zipfile.ZipInfo(name, date_time=(2026, 1, 1, 0, 0, 0))
            entry.create_system = 3
            entry.external_attr = (stat.S_IFREG | 0o644) << 16
            entry.compress_type = zipfile.ZIP_DEFLATED
            zipped.writestr(entry, data)
    report = {"archive": str(archive), "sha256": digest(archive.read_bytes()),
              "files": {name: digest(data) for name, data in sorted(payload.items())}}
    with zipfile.ZipFile(archive) as zipped:
        assert zipped.testzip() is None and set(zipped.namelist()) == set(payload)
        assert all(digest(zipped.read(name)) == sha for name, sha in report["files"].items())
    (out / "source-freeze.json").write_text(json.dumps(report, indent=2) + "\n", encoding="utf8")
    (out / (archive.name + ".sha256")).write_text(report["sha256"] + "  " + archive.name + "\n", encoding="utf8")
    print(json.dumps({name: value for name, value in report.items() if name != "files"}, indent=2))


if __name__ == "__main__":
    main()
