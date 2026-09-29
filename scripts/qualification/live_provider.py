"""Deterministic localhost OpenAI stream fixture, with real tool definitions."""

import http.server
import json
import re
import shlex


class Provider(http.server.BaseHTTPRequestHandler):
    def log_message(self, *_):
        pass

    def do_POST(self):
        request = json.loads(
            self.rfile.read(int(self.headers.get("Content-Length", 0)))
        )
        messages = request.get("messages", [])
        user = next(
            (
                str(message.get("content", ""))
                for message in reversed(messages)
                if message.get("role") == "user"
            ),
            "",
        )
        followup = next(
            (
                tool["function"]["name"]
                for tool in request.get("tools", [])
                if tool["function"]["name"].lower().endswith("followup")
            ),
            None,
        )
        if "SYNTHETIC_FOLLOWUP" in user:
            print(
                json.dumps(
                    {
                        "synthetic_tool_names": [
                            tool["function"]["name"]
                            for tool in request.get("tools", [])
                        ]
                    }
                ),
                flush=True,
            )
        chunks = [
            ({"role": "assistant", "content": "SYNTHETIC_TUI_WITNESS\n"}, None),
            ({}, "stop"),
        ]
        if "SYNTHETIC_FOLLOWUP" in user and followup:
            arguments = {
                "question": "SYNTHETIC_HELD_QUESTION",
                "multiple": None,
                **{f"option{index}": None for index in range(1, 6)},
            }
            chunks = [
                (
                    {
                        "role": "assistant",
                        "tool_calls": [
                            {
                                "index": 0,
                                "id": "synthetic-followup",
                                "type": "function",
                                "function": {
                                    "name": followup,
                                    "arguments": json.dumps(arguments),
                                },
                            }
                        ],
                    },
                    None,
                ),
                ({}, "tool_calls"),
            ]
        permission = re.search(
            r"SYNTHETIC_PERMISSION_(accept|reject|cancel|expire)", user
        )
        last_user = max(
            (
                index
                for index, message in enumerate(messages)
                if message.get("role") == "user"
            ),
            default=-1,
        )
        answered = any(
            message.get("role") == "tool" for message in messages[last_user + 1 :]
        )
        shell = next(
            (
                tool["function"]["name"]
                for tool in request.get("tools", [])
                if tool["function"]["name"].lower().endswith("shell")
            ),
            None,
        )
        if permission and shell and not answered:
            mode = permission[1]
            witness = self.server.witness_root / f"permission-{mode}"
            arguments = {
                "command": f"printf approved >> {shlex.quote(str(witness))}",
                "cwd": str(self.server.witness_root),
            }
            chunks = [
                (
                    {
                        "role": "assistant",
                        "tool_calls": [
                            {
                                "index": 0,
                                "id": f"synthetic-permission-{mode}",
                                "type": "function",
                                "function": {
                                    "name": shell,
                                    "arguments": json.dumps(arguments),
                                },
                            }
                        ],
                    },
                    None,
                ),
                ({}, "tool_calls"),
            ]
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.end_headers()
        for delta, finish in chunks:
            chunk = {
                "id": "synthetic",
                "object": "chat.completion.chunk",
                "created": 1,
                "model": "synthetic",
                "choices": [{"index": 0, "delta": delta, "finish_reason": finish}],
            }
            self.wfile.write(b"data: " + json.dumps(chunk).encode() + b"\n\n")
        self.wfile.write(b"data: [DONE]\n\n")
        self.wfile.flush()
