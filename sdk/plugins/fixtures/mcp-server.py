"""Offline MCP stdio fixture. No shell, file tools, model calls, or network access."""
import json
import sys
import threading
import time

lock = threading.Lock()
cancelled = set()


def send(value):
    with lock:
        print(json.dumps(value), flush=True)


def tool(request):
    identity = request["id"]
    name = request.get("params", {}).get("name")
    if name == "wait":
        for _ in range(100):
            time.sleep(0.05)
            if identity in cancelled:
                return
    send({"jsonrpc": "2.0", "id": identity, "result": {
        "content": [{"type": "text", "text": "offline MCP fixture"}],
        "isError": name == "error",
    }})


for line in sys.stdin:
    request = json.loads(line)
    method = request.get("method")
    if method == "initialize":
        send({"jsonrpc": "2.0", "id": request["id"], "result": {
            "protocolVersion": "2025-11-25", "capabilities": {"tools": {"listChanged": True}},
            "serverInfo": {"name": "offline-stdio-fixture", "version": "1.0.0"},
        }})
    elif method == "tools/list":
        send({"jsonrpc": "2.0", "id": request["id"], "result": {"tools": [
            {"name": name, "inputSchema": {"type": "object"}} for name in ["echo", "error", "wait"]
        ]}})
    elif method == "tools/call":
        threading.Thread(target=tool, args=(request,), daemon=True).start()
    elif method == "notifications/cancelled":
        cancelled.add(request["params"]["requestId"])
    elif method == "notifications/initialized":
        send({"jsonrpc": "2.0", "method": "notifications/tools/list_changed"})
