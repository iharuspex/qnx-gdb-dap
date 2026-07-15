#!/usr/bin/env python3

import io
import json
import subprocess
import sys


def encode_message(message: dict) -> bytes:
    body = json.dumps(
        message,
        separators=(",", ":"),
    ).encode("utf-8")

    return (
        f"Content-Length: {len(body)}\r\n\r\n".encode("ascii")
        + body
    )


def decode_messages(data: bytes) -> list[dict]:
    stream = io.BytesIO(data)
    messages: list[dict] = []

    while True:
        headers: dict[str, str] = {}

        while True:
            line = stream.readline()

            if not line:
                return messages

            if line == b"\r\n":
                break

            name, value = line.decode("ascii").split(":", 1)
            headers[name.lower()] = value.strip()

        content_length = int(headers["content-length"])
        body = stream.read(content_length)

        if len(body) != content_length:
            raise RuntimeError("truncated DAP response")

        messages.append(json.loads(body))


requests = [
    {
        "seq": 1,
        "type": "request",
        "command": "initialize",
        "arguments": {
            "clientID": "manual-test",
            "adapterID": "qnx-gdb",
            "pathFormat": "path",
            "linesStartAt1": True,
            "columnsStartAt1": True,
        },
    },
    {
        "seq": 2,
        "type": "request",
        "command": "launch",
        "arguments": {
            "gdb": (
                "/opt/qnx650/host/linux/x86/usr/bin/"
                "ntoarm-gdb"
            ),
            "program": (
                "/home/haruspex/projects/qnx/sigma/software/"
                "fadapt-mavlink/build/fadapt-mavlink"
            ),
            "target": "192.168.1.28:8080",
        },
    },
    {
        "seq": 3,
        "type": "request",
        "command": "disconnect",
        "arguments": {
            "terminateDebuggee": False,
        },
    },
]

process = subprocess.run(
    ["cargo", "run", "--quiet", "-p", "qnx-gdb-dap"],
    input=b"".join(encode_message(request) for request in requests),
    stdout=subprocess.PIPE,
    stderr=sys.stderr,
    check=True,
)

for message in decode_messages(process.stdout):
    print(json.dumps(message, indent=2))
