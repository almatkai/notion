#!/usr/bin/env python3
"""Vendor exact dependency license/notice files for both published targets."""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess

ROOT = Path(__file__).resolve().parents[1]
TARGETS = ("aarch64-apple-darwin", "x86_64-pc-windows-msvc")
# This crates.io distribution includes in-file MPL notices but no standalone
# license document. Preserve the exact covered source crate and Mozilla text.
EXTRA_NOTICES = {
    ("selectors", "0.41.0"): {
        "LICENSE-MPL-2.0": "fab3dd6bdab226f1c08630b1dd917e11fcb4ec5e1e020e2c16f83a0a13863e85",
        "selectors-0.41.0.crate": "8322aebea94a6cc288ff884e8a49bdbceef60206c400c6c46d22e62fc4fd91a6",
    }
}


def inventory():
    packages = {}
    for target in TARGETS:
        metadata = json.loads(subprocess.check_output(
            ["cargo", "metadata", "--locked", "--format-version", "1", "--filter-platform", target],
            cwd=ROOT))
        nodes = {node["id"]: node for node in metadata["resolve"]["nodes"]}
        queue = [metadata["resolve"]["root"]]
        selected = set()
        while queue:
            ident = queue.pop()
            if ident in selected:
                continue
            selected.add(ident)
            queue.extend(dep["pkg"] for dep in nodes[ident]["deps"]
                         if any(kind["kind"] in (None, "build") for kind in dep["dep_kinds"]))
        for package in metadata["packages"]:
            if package["id"] not in selected or package["source"] is None:
                continue
            item = packages.setdefault(package["id"], {"package": package, "targets": []})
            item["targets"].append(target)
    return sorted(packages.values(), key=lambda item: (item["package"]["name"], item["package"]["version"]))

def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    entries = []
    for item in inventory():
        package = item["package"]
        source = Path(package["manifest_path"]).parent
        files = [path for path in source.iterdir()
                 if path.is_file() and path.name.upper().startswith(("LICENSE", "NOTICE", "COPYING", "COPYRIGHT"))]
        if package.get("license_file"):
            extra = source / package["license_file"]
            if extra.is_file() and extra not in files:
                files.append(extra)
        extras = EXTRA_NOTICES.get((package["name"], package["version"]), {})
        if not files and not extras:
            raise SystemExit("Missing upstream license documents: " + package["id"])
        entry = {"name": package["name"], "version": package["version"],
                 "declaredLicenses": package["license"], "source": package["source"],
                 "targets": item["targets"], "documents": {}}
        for path in sorted(files):
            relative = Path("licenses/crates") / (package["name"] + "-" + package["version"]) / path.name
            contents = path.read_bytes()
            destination = ROOT / relative
            if args.check:
                assert destination.read_bytes() == contents, "Changed/missing notice: " + str(relative)
            else:
                destination.parent.mkdir(parents=True, exist_ok=True)
                destination.write_bytes(contents)
            entry["documents"][relative.as_posix()] = hashlib.sha256(contents).hexdigest()
        for name, expected in extras.items():
            relative = Path("licenses/crates") / (package["name"] + "-" + package["version"]) / name
            digest = hashlib.sha256((ROOT / relative).read_bytes()).hexdigest()
            assert digest == expected, "Changed supplemental upstream notice: " + str(relative)
            entry["documents"][relative.as_posix()] = digest
        entries.append(entry)
    index = json.dumps({"targets": TARGETS, "dependencies": entries}, indent=2, ensure_ascii=False) + "\n"
    lines = ["# Third-party Rust dependency notices", "",
             "Resolved from the locked normal and build dependency closure for both published targets.",
             "Development-only dependencies are excluded. The original license, notice and copyright",
             "documents supplied by each dependency are preserved byte for byte; all supplied",
             "alternative-license documents are included. Proc-macro build dependencies are included.",
             "Package-owned and original Raycast notices remain in LICENSE, NOTICE and licenses/.", "",
             "The selectors 0.41.0 crate publishes in-file MPL 2.0 notices. Its complete original",
             "[source archive](https://static.crates.io/crates/selectors/selectors-0.41.0.crate) is bundled",
             "with the [Mozilla Public License text](https://www.mozilla.org/en-US/MPL/2.0/).", "",
             "| Crate | Version | Declared licenses | Bundled documents |", "|---|---|---|---|"]
    for entry in entries:
        documents = ", ".join("[" + Path(name).name + "](" + Path(name).relative_to("licenses").as_posix() + ")"
                              for name in entry["documents"])
        lines.append("| " + entry["name"] + " | " + entry["version"] + " | " +
                     str(entry["declaredLicenses"]) + " | " + documents + " |")
    markdown = "\n".join(lines) + "\n"
    for relative, contents in (("licenses/dependency-license-index.json", index), ("licenses/THIRD_PARTY.md", markdown)):
        path = ROOT / relative
        if args.check:
            assert path.read_text(encoding="utf-8") == contents, "Stale inventory: " + relative
        else:
            path.write_text(contents, encoding="utf-8")
    print("Verified" if args.check else "Vendored", len(entries), "dependency notice sets")

if __name__ == "__main__":
    main()
