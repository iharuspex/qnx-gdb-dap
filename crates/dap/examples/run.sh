#!/bin/bash

python3 - <<'PY' | cargo run -p qnx-dap --example read_message
import json
import sys

message = {
    "seq": 1,
    "type": "request",
    "command": "initialize",
}

body = json.dumps(message, separators=(",", ":")).encode()

sys.stdout.buffer.write(
    f"Content-Length: {len(body)}\r\n\r\n".encode() + body
)
PY
