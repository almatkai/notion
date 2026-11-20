"""Run the packed production executable with a temporary profile and no auth."""
import json
import os
import queue
from pathlib import Path
import subprocess
import sys
import tempfile
import threading
import uuid
import zipfile

binary_zip = Path(sys.argv[1]).resolve()


def request(process, frames, method, params):
    ident = "h:" + str(uuid.uuid4())
    frame = json.dumps(dict(type="request", id=ident, method=method, params=params)) + "\n"
    process.stdin.write(frame.encode("utf8"))
    process.stdin.flush()
    reply = frames.get(timeout=15)
    assert reply["type"] == "response" and reply["id"] == ident and "error" not in reply, reply
    return reply["result"]


with tempfile.TemporaryDirectory(prefix="tezbar-notion-production-smoke-") as raw_dir:
    work = Path(raw_dir)
    for directory in ("data", "package", "temp"):
        (work / directory).mkdir()
    with zipfile.ZipFile(binary_zip) as archive:
        manifest = json.loads(archive.read("extension.json"))
        executable = next(name for name in manifest["files"] if name.startswith("bin/"))
        binary = work / "package" / Path(executable).name
        binary.write_bytes(archive.read(executable))
        binary.chmod(0o755)
    env = dict(os.environ, TEZBAR_NOTION_FIXTURE_ORIGIN="https://invalid-fixture.example/")
    with subprocess.Popen([str(binary)], stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, env=env) as process:
        frames = queue.Queue()

        def read_frames():
            for line in process.stdout:
                frames.put(json.loads(line))

        threading.Thread(target=read_frames, daemon=True).start()
        try:
            hello = frames.get(timeout=15)
            assert (hello["id"], hello["version"], hello["protocol"]) == (manifest["id"], manifest["version"], 1)
            args = {"protocol": 1, "extensionId": manifest["id"], "platform": manifest["platforms"][0],
                    "packageDir": str(work / "package"), "dataDir": str(work / "data"), "tempDir": str(work / "temp"),
                    "grantedCapabilities": manifest["capabilities"], "migrationPaths": {}, "reason": "test"}
            assert request(process, frames, "initialize", args)["ready"] is True
            for index, command in enumerate(manifest["commands"]):
                session = f"production-smoke-{index}"
                reply = request(process, frames, "session.open", {"sessionId": session, "commandName": command["name"],
                                                          "argumentValues": {}, "preferences": {}})
                assert reply["root"]["type"] == "Detail" and "Internal Integration Secret" in reply["root"]["props"]["markdown"]
                request(process, frames, "session.dispose", {"sessionId": session})
            request(process, frames, "shutdown", {})
            process.stdin.close()
            assert process.wait(timeout=10) == 0
        finally:
            if process.poll() is None:
                process.kill()
    print("Production ZIP binary initialized and served all six setup views in a temporary directory without credentials.")
