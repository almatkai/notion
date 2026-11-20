"""Verify original inventory/bytes and native manifest mappings from the immutable source."""
import hashlib
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
UPSTREAM = ROOT / "upstream"
EXPECTED_COMMIT = "459326f4f557692a79246dba2d013746f56d724b"
ORIGINAL_PREFIX = "extensions/notion/"


def read_json(path):
    return json.loads(path.read_text(encoding="utf-8"))


def main():
    inventory = read_json(UPSTREAM / "source-blobs.json")
    source = read_json(UPSTREAM / "package.json")
    store = read_json(ROOT / "UPSTREAM.json")
    manifest = read_json(ROOT / "extension.json")
    assert inventory["commit"] == store["commit_sha"] == EXPECTED_COMMIT
    assert manifest["repository"] == "https://github.com/almatkai/notion"
    paths = set()
    for row in inventory["files"]:
        rel = row["path"].removeprefix(ORIGINAL_PREFIX)
        paths.add(rel)
        data = (UPSTREAM / rel).read_bytes()
        assert hashlib.sha256(data).hexdigest() == row["sha256"], rel
        assert hashlib.sha1(f"blob {len(data)}\0".encode() + data).hexdigest() == row["gitBlob"], rel
        if rel.startswith("assets/"):
            assert data == (ROOT / "resources" / rel).read_bytes(), rel
    present = {path.relative_to(UPSTREAM).as_posix() for path in UPSTREAM.rglob("*") if path.is_file()}
    assert present == paths | {"source-blobs.json"}
    assert (UPSTREAM / "LICENSE").read_bytes() == (ROOT / "licenses/raycast-extensions-MIT.txt").read_bytes()
    assert (UPSTREAM / "skills/notion-knowledge-capture/LICENSE").read_bytes() == (ROOT / "licenses/notion-knowledge-capture-LICENSE.txt").read_bytes()
    assert source["platforms"] == manifest["platforms"] == store["platforms"]
    assert source["author"] == manifest["author"] == "HenriChabrand" and source["owner"] == "notion"
    assert [c["name"] for c in source["commands"]] == [c["name"] for c in manifest["commands"]] == [c["name"] for c in store["commands"]]
    assert len(manifest["commands"]) == 6 and len(store["tools"]) == 7
    for orig, port in zip(source["commands"], manifest["commands"]):
        assert {name: port[name] for name in orig} == orig
    for orig, port in zip(source["preferences"], manifest["preferences"]):
        if orig["name"] == "notion_token":
            orig = {**orig, "placeholder": "ntn_..."}
        assert port == orig
    assert manifest["replaces"] == ["raycast.notion"] and manifest["id"] == "com.tezbar.native.notion"
    assert (manifest["schemaVersion"], manifest["runtime"], manifest["execution"], manifest["protocolVersion"]) == (3, "rust", "process", 1)
    print(f"Verified {len(paths)} Git blobs at {EXPECTED_COMMIT}, 6 command IDs, 7 tool IDs, preferences, assets and notices.")


if __name__ == "__main__":
    main()
