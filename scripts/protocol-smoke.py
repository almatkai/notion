"""Drive the real JSONL binary against an owned Notion and article server."""
import http.server
import json
import os
from pathlib import Path
import queue
import subprocess
import sys
import tempfile
import threading
from urllib.parse import parse_qs, urlsplit
import uuid

ROOT = Path(__file__).resolve().parents[1]
BINARY = Path(sys.argv[1]).resolve()
BUILD = json.loads(subprocess.check_output([str(BINARY), "--build-info"]))
assert BUILD == {"fixture": True, "apiVersion": "2026-03-11"}


def ident(n):
    return str(uuid.UUID(int=n))


PAGE, EMPTY, DATABASE, SOURCE, SECOND, RECORD, PARENT, NESTED = [ident(n) for n in range(1, 9)]
PAGE_PROPERTIES = {
    "Renamed title": {"id": "title", "type": "title", "title": [{"plain_text": "Record title"}]},
    "Estimate": {"id": "num", "type": "number", "number": 3},
    "State": {"id": "state", "type": "select", "select": {"id": "option-1", "name": "First"}},
}
SCHEMA = {
    "Renamed title": {"id": "title", "type": "title", "title": {}},
    "Estimate": {"id": "num", "type": "number", "number": {}},
    "State": {"id": "state", "type": "select", "select": {"options": [
        {"id": "option-1", "name": "First"}, {"id": "option-2", "name": "Second"}]}},
}


def page(i, title):
    properties = {"title": {"id": "title", "type": "title", "title": [{"plain_text": title}]}}
    if i == RECORD:
        properties = json.loads(json.dumps(PAGE_PROPERTIES))
    return {
        "object": "page", "id": i, "url": "https://www.notion.so/" + i.replace("-", ""),
        "properties": properties, "in_trash": False,
        "parent": {"type": "workspace"} if i != RECORD else {"type": "data_source_id", "data_source_id": SOURCE},
    }


def block(text, children=False):
    return {"type": "paragraph", "id": NESTED, "has_children": children,
            "paragraph": {"rich_text": [{"plain_text": text}]}}


def result(rows, cursor=None):
    return {"results": rows, "has_more": cursor is not None, "next_cursor": cursor}


class LocalNotion(http.server.BaseHTTPRequestHandler):
    calls = []
    content = {PAGE: [block("Original first", True)], EMPTY: [], NESTED: [block("Child block text")]}
    pages = {i: page(i, name) for i, name in [
        (PAGE, "Workspace Page"), (EMPTY, "Empty Page"), (RECORD, "Record title"), (PARENT, "Notes Parent")]}
    sources = {
        i: {"object": "data_source", "id": i, "title": [{"plain_text": name}],
            "parent": {"type": "database_id", "database_id": DATABASE}, "properties": SCHEMA, "in_trash": False}
        for i, name in [(SOURCE, "Tasks"), (SECOND, "Projects")]
    }

    def log_message(self, *args):
        return

    def do_GET(self):
        self.dispatch()

    def do_POST(self):
        self.dispatch()

    def do_PATCH(self):
        self.dispatch()

    def send_data(self, status, data, content_type="application/json", **headers):
        if isinstance(data, dict):
            data = json.dumps(data).encode()
        self.send_response(status)
        self.send_header("Content-Type", content_type)
        self.send_header("Content-Length", str(len(data)))
        for name, value in headers.items():
            self.send_header(name.replace("_", "-"), value)
        self.end_headers()
        self.wfile.write(data)

    def dispatch(self):
        url = urlsplit(self.path)
        method = self.command
        size = int(self.headers.get("Content-Length", "0"))
        args = json.loads(self.rfile.read(size)) if size else {k: v[-1] for k, v in parse_qs(url.query).items()}
        self.calls.append((method, url.path, args))
        if url.path == "/article":
            assert self.headers.get("Authorization") is None and self.headers.get("Notion-Version") is None
            self.send_data(200, b"<title>Fixture Article</title><main><h1>Fixture Heading</h1><p>Capturable content.</p></main>", "text/html")
            return
        if url.path == "/redirect-denied":
            self.send_data(302, b"", Location="http://127.0.0.1:9/article")
            return
        assert self.headers.get("Authorization") == "Bearer ntn_fixture_only"
        assert self.headers.get("Notion-Version") == "2026-03-11"
        parts = url.path.removeprefix("/v1/").split("/")
        status, reply = 200, {}
        if parts == ["users", "me"]:
            reply = {"object": "user", "name": "Fixture Integration"}
        elif parts == ["search"]:
            query = args.get("query", "")
            cursor = args.get("start_cursor")
            object_filter = args.get("filter", {}).get("value")
            if query == "rate limit":
                self.send_data(429, {"message": "fixture secret marker"}, Retry_After="2")
                return
            if query == "cursor cycle":
                reply = result([], "cycle-a" if not cursor or cursor == "cycle-b" else "cycle-b")
            elif query == "second only":
                rows = [self.pages[EMPTY]] if cursor else []
                reply = result(rows, None if cursor else "workspace-next")
            elif query == "Make Notes":
                reply = result([] if not cursor else [
                    p for p in self.pages.values() if p["properties"].get("title", {}).get("title", [{}])[0].get("plain_text") == "Make Notes"
                ], None if cursor else "search-notes")
            elif object_filter == "data_source":
                reply = result(list(self.sources.values()))
            elif object_filter == "page":
                reply = result([self.pages[PAGE], self.pages[EMPTY], self.pages[PARENT]])
            else:
                reply = result([self.pages[EMPTY]] if cursor else [self.pages[PAGE], self.sources[SOURCE]],
                               None if cursor else "workspace-next")
        elif len(parts) == 2 and parts[0] == "databases" and parts[1] == DATABASE:
            reply = {"object": "database", "id": DATABASE, "title": [{"plain_text": "Fixture Database"}],
                     "data_sources": [{"id": SOURCE, "name": "Tasks"}, {"id": SECOND, "name": "Projects"}]}
        elif len(parts) >= 2 and parts[0] == "data_sources":
            source = self.sources.get(parts[1])
            if source is None:
                status = 404
            elif parts[-1] == "query":
                cursor = args.get("start_cursor")
                reply = result([self.pages[RECORD]], None if cursor else "records-next")
            elif method == "PATCH":
                assert set(args) == {"in_trash"}
                source.update(args)
                reply = source
            else:
                reply = source
        elif len(parts) == 3 and parts[0] == "blocks" and parts[2] == "children":
            if method == "GET":
                rows = self.content.get(parts[1], [])
                children = [value for value in rows if value.get("type") == "child_page"]
                if len(children) and not args.get("start_cursor"):
                    reply = result([block("Earlier block")], "children-next")
                elif args.get("start_cursor") == "children-next":
                    reply = result(children)
                else:
                    reply = result(rows)
            else:
                children = args["children"]
                if "FAIL WRITE" in json.dumps(children):
                    self.send_data(500, {"message": "fixture secret marker"})
                    return
                rows = self.content.setdefault(parts[1], [])
                if args.get("position") == {"type": "start"}:
                    rows[0:0] = children
                else:
                    assert "position" not in args and "after" not in args
                    rows.extend(children)
                reply = result(children)
        elif parts == ["pages"] and method == "POST":
            props = args["properties"]
            title_value = next(value["title"] for value in props.values() if "title" in value)
            title = "".join(value["text"]["content"] for value in title_value)
            created = ident(80 + len(self.pages))
            reply = page(created, title)
            self.pages[created] = reply
            parent = args["parent"]
            if parent["type"] == "page_id":
                assert set(parent) == {"type", "page_id"}
                self.content.setdefault(parent["page_id"], []).append({
                    "id": created, "type": "child_page", "child_page": {"title": title}
                })
            else:
                assert parent["type"] == "data_source_id" and parent["data_source_id"] in self.sources
            self.content[created] = args.get("children", [])
        elif len(parts) == 2 and parts[0] == "pages":
            reply = self.pages.get(parts[1])
            if reply is None:
                status, reply = 404, {}
            elif method == "PATCH":
                if "in_trash" in args:
                    assert set(args) == {"in_trash"}
                    reply.update(args)
                else:
                    assert "properties" in args and args["properties"] != {}
        else:
            assert False, (method, parts)
        self.send_data(status, reply)


class Peer:
    def __init__(self, work, endpoint):
        self.frames = queue.Queue()
        self.host_calls = []
        self.confirm = False
        self.process = subprocess.Popen(
            [str(BINARY)], stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
            env=dict(os.environ, TEZBAR_NOTION_FIXTURE_ORIGIN=endpoint),
        )

        def pump():
            for line in self.process.stdout:
                self.frames.put(json.loads(line))

        threading.Thread(target=pump, daemon=True).start()
        hello = self.frames.get(timeout=15)
        assert hello["id"] == "com.tezbar.native.notion" and hello["protocol"] == 1
        for child in ("data", "temp"):
            (work / child).mkdir()
        caps = json.loads((ROOT / "extension.json").read_text(encoding="utf8"))["capabilities"]
        self.request("initialize", {"protocol": 1, "extensionId": hello["id"], "packageDir": str(ROOT),
                                    "dataDir": str(work / "data"), "tempDir": str(work / "temp"),
                                    "grantedCapabilities": caps, "migrationPaths": {}, "reason": "test",
                                    "platform": "Windows" if sys.platform == "win32" else "macOS"})

    def send(self, frame):
        self.process.stdin.write((json.dumps(frame) + "\n").encode())
        self.process.stdin.flush()

    def request(self, method, params):
        request_id = "h:" + str(uuid.uuid4())
        self.send({"type": "request", "id": request_id, "method": method, "params": params})
        while True:
            frame = self.frames.get(timeout=38)
            if frame["type"] == "response" and frame["id"] == request_id:
                assert "error" not in frame, frame
                assert "ntn_fixture_only" not in json.dumps(frame), "Auth was returned to the renderer"
                return frame["result"]
            if frame["type"] == "request":
                method = frame["method"]
                self.host_calls.append((method, frame["params"]))
                answers = {"host.dialog.confirm": {"response": int(self.confirm)}, "host.ai.ask": "## Synthetic summary\n\nNice article.",
                           "host.clipboard.read": "Clipboard draft", "host.selection.read": None,
                           "host.quicklinks.create": {"status": "created"}}
                self.send({"type": "response", "id": frame["id"], "result": answers.get(method, True)})

    def open(self, session, command, preferences=None, arguments=None):
        prefs = {"notion_token": "ntn_fixture_only", "open_in": "Browser"}
        return self.request("session.open", {"sessionId": session, "commandName": command,
                            "argumentValues": arguments or {}, "preferences": prefs if preferences is None else preferences})

    def call(self, session, operation, data=None):
        return self.request("session.call", {"sessionId": session, "operation": "extension:" + operation, "payload": data or {}})

    def action(self, session, view, title, values=None, row=None):
        actions = view["actions"]
        if row:
            node = next(node for node in view["root"]["children"] if node["props"].get("id") == row)
            actions = [a for a in actions if a["id"] in node["props"]["actionIds"]]
        action = next(a for a in actions if a["title"] == title)
        return self.call(session, "invoke-action", {"actionId": action["id"], "formValues": values or {}})

    def close(self):
        self.request("shutdown", {})
        self.process.stdin.close()
        assert self.process.wait(timeout=10) == 0, self.process.stderr.read().decode()


def field(view, name):
    return next(node["props"] for node in view["root"]["children"] if node["props"].get("id") == name)


def verify(peer, endpoint):
    checks = []
    calls = LocalNotion.calls
    view = peer.open("setup", "manage-connection", {})
    assert "Internal Integration Secret" in json.dumps(view) and calls == []
    peer.action("setup", view, "Open Extension Preferences")
    assert peer.host_calls[-1][0] == "host.settings.open"
    view = peer.open("manage", "manage-connection")
    assert "Fixture Integration" in json.dumps(view)
    peer.action("manage", view, "Test Connection")
    assert [path for _, path, _ in calls] == ["/v1/users/me"] * 2
    checks.append("Setup performs no HTTP and connection test uses GET users/me with the versioned header")

    view = peer.open("search", "search-page")
    old_action = view["actions"][0]["id"]
    assert view["hasMore"] is True
    view = peer.call("search", "load-more")
    assert view["hasMore"] is False and any(row["props"]["id"] == EMPTY for row in view["root"]["children"])
    assert peer.call("search", "invoke-action", {"actionId": old_action})["mode"] == "unchanged"
    view = peer.action("search", view, "Preview Page", row=PAGE)
    assert "Original first" in view["root"]["props"]["markdown"] and "Child block text" in view["root"]["props"]["markdown"]
    checks.append("Search cursor, stale callback fencing and nested rich-block preview")

    view = peer.action("search", view, "Back")
    view = peer.action("search", view, "Find by URL or ID")
    view = peer.action("search", view, "Find Item", {"target": DATABASE})
    assert {node["props"]["id"] for node in view["root"]["children"]} == {SOURCE, SECOND}
    view = peer.action("search", view, "Browse Data Source", row=SOURCE)
    view = peer.call("search", "search-text-changed", {"searchText": "Record"})
    assert calls[-1][2]["filter"] == {"property": "title", "title": {"contains": "Record"}}
    view = peer.call("search", "load-more")
    assert view["hasMore"] is False
    checks.append("Database ID resolves container and offers both data sources; records paginate and filter by the schema title ID")

    view = peer.action("search", view, "Edit Properties", row=RECORD)
    view = peer.action("search", view, "Edit Property", row="state")
    assert field(view, "property::select::state")["value"] == "option-1"
    before = len(calls)
    view = peer.action("search", view, "Save Property", {"property::select::state": "invalid"})
    assert len(calls) == before and "Enter a valid value" in json.dumps(view)
    view = peer.action("search", view, "Save Property", {"property::select::state": "option-2"})
    assert calls[-1] == ("PATCH", "/v1/pages/" + RECORD, {"properties": {"state": {"select": {"id": "option-2"}}}})
    checks.append("Property editor validates schema choices and updates only the selected property")

    view = peer.open("create", "create-database-page",
                     {"notion_token": "ntn_fixture_only", "useClipboard": "title", "closeAfterCreate": True})
    view = peer.action("create", view, "Create Page", row=SOURCE)
    assert field(view, "property::title::title")["value"] == "Clipboard draft"
    view = peer.action("create", view, "Create Page and Close", {
        "property::title::title": "Synthetic record", "property::number::num": "0",
        "property::select::state": "option-1", "content": "# Heading\n\n**Body**", "addDateDivider": True})
    method, path, body = calls[-1]
    assert (method, path) == ("POST", "/v1/pages")
    assert body["parent"] == {"type": "data_source_id", "data_source_id": SOURCE}
    assert body["properties"]["num"]["number"] == 0
    assert [value["type"] for value in body["children"]] == ["divider", "paragraph", "heading_1", "paragraph"]
    assert peer.host_calls[-1][0] == "host.window.visibility"
    checks.append("Database create enforces typed properties, honors clipboard and close preferences and submits rich blocks and date divider")

    for target, expected in [(PAGE, "New first populated"), (EMPTY, "New first empty")]:
        session = "append-" + target
        view = peer.open(session, "add-text-to-page", arguments={"text": expected})
        view = peer.action(session, view, "Add Text Here", row=target)
        assert field(view, "textToAppend")["value"] == expected
        view = peer.action(session, view, "Add Text to Page", {"prepend": True})
        method, path, body = calls[-1]
        assert (method, path) == ("PATCH", f"/v1/blocks/{target}/children")
        assert body.get("position") == {"type": "start"} and "after" not in body
        assert LocalNotion.content[target][0]["paragraph"]["rich_text"][0]["text"]["content"] == expected
    assert LocalNotion.content[PAGE][1]["paragraph"]["rich_text"][0]["plain_text"] == "Original first"
    checks.append("Versioned position.start on a populated and an empty page preserves original contents")

    view = peer.open("fail", "add-text-to-page")
    view = peer.action("fail", view, "Add Text Here", row=PAGE)
    before = len(calls)
    view = peer.action("fail", view, "Add Text to Page", {"textToAppend": "FAIL WRITE"})
    assert len(calls) == before + 1 and "fixture secret marker" not in json.dumps(view)
    assert field(view, "textToAppend")["value"] == "FAIL WRITE"
    checks.append("Ambiguous HTTP failure preserves drafts and never replays a mutation or echoes server diagnostics")

    competing_root = ident(900)
    LocalNotion.pages[competing_root] = page(competing_root, "Make Notes")
    LocalNotion.content[competing_root] = []
    prefs = {"notion_token": "ntn_fixture_only", "notes_page_name": "Make Notes", "note_style": "todo"}
    view = peer.open("note", "add-note", prefs, {"note": "Draft from argument"})
    assert field(view, "note")["value"] == "Draft from argument"
    view = peer.action("note", view, "Choose Parent Page", {"note": "Parent picker retains text"})
    view = peer.action("note", view, "Use This Parent", row=PARENT)
    assert field(view, "note")["value"] == "Parent picker retains text"
    begin = len(calls)
    view = peer.action("note", view, "Add Note")
    assert not any(path == "/v1/search" for _, path, _ in calls[begin:])
    created = [args for method, path, args in calls[begin:] if method == "POST" and path == "/v1/pages"]
    assert len(created) == 2 and created[1]["children"][0]["type"] == "to_do"
    assert created[0]["parent"] == {"type": "page_id", "page_id": PARENT}
    assert LocalNotion.content[competing_root] == []
    view = peer.action("note", view, "Back")
    begin = len(calls)
    view = peer.action("note", view, "Add Note", {"note": "Second note"})
    assert not any(method == "POST" and path == "/v1/pages" for method, path, _ in calls[begin:])
    assert calls[-1][0] == "PATCH" and calls[-1][2]["children"][0]["type"] == "to_do"
    assert any(args.get("start_cursor") == "children-next" for method, path, args in calls[begin:] if method == "GET")
    checks.append("Daily notes create root and day under an explicit parent, preserve draft, follow cursors and reuse pages")
    del LocalNotion.pages[competing_root]
    del LocalNotion.content[competing_root]
    view = peer.open("global-note", "add-note", prefs, {"note": "Global lookup"})
    begin = len(calls)
    view = peer.action("global-note", view, "Add Note")
    assert [args.get("start_cursor") for method, path, args in calls[begin:] if path == "/v1/search"] == [None, "search-notes"]
    assert not any(method == "POST" and path == "/v1/pages" for method, path, _ in calls[begin:])
    assert calls[-1][0] == "PATCH"
    checks.append("An explicit notes parent cannot select a competing global root; unscoped notes still follow search cursors")

    view = peer.open("bookmark", "quick-capture")
    view = peer.action("bookmark", view, "Capture Here", row=PAGE)
    begin = len(calls)
    view = peer.action("bookmark", view, "Capture", {"url": endpoint + "/article", "captureAs": "url"})
    assert len(calls) == begin + 1 and calls[-1][2]["children"][0]["type"] == "bookmark"
    view = peer.open("full", "quick-capture")
    view = peer.action("full", view, "Capture Here", row=SOURCE)
    view = peer.action("full", view, "Capture", {"url": endpoint + "/article", "captureAs": "full"})
    assert calls[-1][1] == "/v1/pages" and "Capturable content." in json.dumps(calls[-1][2])
    assert calls[-1][2]["properties"]["title"]["title"][0]["text"]["content"] == "Fixture Article"
    checks.append("Bookmark append avoids a download; full article capture uses a separate credential-free client and creates in the selected source")

    view = peer.open("ai", "quick-capture")
    view = peer.action("ai", view, "Capture Here", row=PAGE)
    begin = len(calls)
    view = peer.action("ai", view, "Capture", {"url": endpoint + "/article", "captureAs": "ai"})
    assert len(calls) == begin
    peer.confirm = True
    view = peer.action("ai", view, "Capture")
    assert "Synthetic summary" in json.dumps(calls[-1][2])
    assert any(method == "host.ai.ask" for method, _ in peer.host_calls)
    view = peer.action("ai", view, "Back")
    begin = len(calls)
    view = peer.action("ai", view, "Capture", {"url": endpoint + "/redirect-denied", "captureAs": "full"})
    assert len(calls) == begin + 1 and "only from the configured loopback origin" in json.dumps(view)
    checks.append("AI provider is called only after confirmation; external redirect is rejected by the fixture binary")

    view = peer.open("trash", "search-page")
    peer.confirm = False
    begin = len(calls)
    view = peer.action("trash", view, "Move to Trash", row=PAGE)
    assert len(calls) == begin
    peer.confirm = True
    for target, resource in [(PAGE, "pages"), (SOURCE, "data_sources")]:
        begin = len(calls)
        view = peer.action("trash", view, "Move to Trash", row=target)
        assert calls[-1] == ("PATCH", f"/v1/{resource}/{target}", {"in_trash": True})
        view = peer.action("trash", view, "Restore from Trash")
        assert calls[begin + 1] == ("PATCH", f"/v1/{resource}/{target}", {"in_trash": False})
    checks.append("Page and data source trash require confirmation; restore uses in_trash=false and refreshes results")

    view = peer.open("rate", "search-page")
    view = peer.call("rate", "search-text-changed", {"searchText": "rate limit"})
    assert "Try again in 2 seconds" in json.dumps(view) and "fixture secret marker" not in json.dumps(view)
    view = peer.call("rate", "search-text-changed", {"searchText": "cursor cycle"})
    view = peer.call("rate", "load-more")
    view = peer.call("rate", "load-more")
    assert "repeated a pagination cursor" in json.dumps(view)
    checks.append("Rate-limit feedback and cursor cycles are bounded")
    return checks


def main():
    bad = subprocess.run([str(BINARY)], input=b"", capture_output=True, timeout=10,
                         env=dict(os.environ, TEZBAR_NOTION_FIXTURE_ORIGIN="https://example.invalid/"))
    assert bad.returncode != 0 and not bad.stdout and b"literal loopback" in bad.stderr
    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), LocalNotion)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    try:
        with tempfile.TemporaryDirectory(prefix="tezbar-notion-fixture-") as work:
            endpoint = f"http://127.0.0.1:{server.server_port}"
            peer = Peer(Path(work), endpoint)
            try:
                checks = verify(peer, endpoint)
            finally:
                peer.close()
    finally:
        server.shutdown()
        server.server_close()

    checks.append("Fixture activation refuses a non-loopback origin before protocol initialization")
    report = {"passed": len(checks), "checks": checks, "apiVersion": BUILD["apiVersion"],
              "liveAccount": "not-used", "desktopAcceptance": "not-run"}
    (ROOT / "reports/protocol-smoke.json").write_text(json.dumps(report, indent=2) + "\n", encoding="utf8")
    print(json.dumps(report, indent=2))


if __name__ == "__main__":
    main()
