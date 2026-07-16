#!/usr/bin/env python3

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


def read_message(stream) -> dict | None:
    headers: dict[str, str] = {}

    while True:
        line = stream.readline()

        if not line:
            return None

        if line == b"\r\n":
            break

        name, value = line.decode("ascii").split(":", 1)
        headers[name.lower()] = value.strip()

    content_length = int(headers["content-length"])
    body = stream.read(content_length)

    if len(body) != content_length:
        raise RuntimeError("truncated DAP message")

    return json.loads(body)


process = subprocess.Popen(
    ["cargo", "run", "--quiet", "-p", "qnx-gdb-dap"],
    stdin=subprocess.PIPE,
    stdout=subprocess.PIPE,
    stderr=sys.stderr,
)

assert process.stdin is not None
assert process.stdout is not None


def send(message: dict) -> None:
    process.stdin.write(encode_message(message))
    process.stdin.flush()


send({
    "seq": 1,
    "type": "request",
    "command": "initialize",
    "arguments": {
        "clientID": "manual-test",
        "adapterID": "qnx-gdb",
        "linesStartAt1": True,
        "columnsStartAt1": True,
    },
})

send({
    "seq": 2,
    "type": "request",
    "command": "launch",
    "arguments": {
        "gdb": (
            "/opt/qnx650/host/linux/x86/usr/bin/"
            "ntoarm-gdb"
        ),
        "program": (
            "/home/haruspex/ide-4.7-workspace/"
            "test_qnx_c/arm/o-le-v7-g/test_qnx_c_g"
        ),
        "target": "192.168.1.28:8080",
        "deployment": {
            "mode": "upload",
            "remoteProgram": "/dev/shmem/test_qnx_c_g",
        },
    },
})

send({
    "seq": 3,
    "type": "request",
    "command": "setBreakpoints",
    "arguments": {
        "source": {
            "path": (
                "/home/haruspex/ide-4.7-workspace/"
                "test_qnx_c/test_qnx_c.c"
            ),
        },
        "breakpoints": [
            {
                "line": 7,
            },
        ],
    },
})

send({
    "seq": 4,
    "type": "request",
    "command": "configurationDone",
    "arguments": {},
})

stopped = False

while True:
    message = read_message(process.stdout)

    if message is None:
        break

    print(json.dumps(message, indent=2))

    if (
        message.get("type") == "event"
        and message.get("event") == "stopped"
    ):
        stopped = True
        break

if stopped:
    send({
        "seq": 5,
        "type": "request",
        "command": "disconnect",
        "arguments": {
            "terminateDebuggee": False,
        },
    })

    while True:
        message = read_message(process.stdout)

        if message is None:
            break

        print(json.dumps(message, indent=2))

        if (
            message.get("type") == "event"
            and message.get("event") == "terminated"
        ):
            break

process.stdin.close()
process.wait(timeout=5)
