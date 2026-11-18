#!/usr/bin/env python3
"""Seeded MCP fuzzer for `prism-editor-mcp` (schema-level and editor-UI modes).

    python scripts/mcp_fuzz.py --project projects/template --seed 42 --steps 25
    python scripts/mcp_fuzz.py --project projects/template --mode editor --seed 7 --steps 20

Speaks JSON-RPC over stdio to `prism-editor-mcp`. In `schema` mode it operates
on a temporary copy of a scene document (no GPU or display required). In `editor`
mode it drives a live `prism-editor-egui` session and scans `editor_logs` for
panics and Vulkan validation errors.
"""

from __future__ import annotations

import argparse
import json
import random
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[1]
PANIC_MARKERS = ("panicked at", "RUST_BACKTRACE", "Validation Error:", "VUID-")


class McpClient:
    """Minimal line-delimited JSON-RPC client for `prism-editor-mcp`."""

    def __init__(self, cmd: list[str], cwd: Path) -> None:
        self.proc = subprocess.Popen(
            cmd,
            cwd=str(cwd),
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            encoding="utf-8",
            errors="replace",
        )
        self._next_id = 1

    def request(self, method: str, params: dict[str, Any] | None = None) -> dict[str, Any]:
        assert self.proc.stdin is not None and self.proc.stdout is not None
        req_id = self._next_id
        self._next_id += 1
        payload: dict[str, Any] = {"jsonrpc": "2.0", "id": req_id, "method": method}
        if params is not None:
            payload["params"] = params
        self.proc.stdin.write(json.dumps(payload) + "\n")
        self.proc.stdin.flush()

        while True:
            line = self.proc.stdout.readline()
            if not line:
                stderr = self.proc.stderr.read() if self.proc.stderr else ""
                raise RuntimeError(f"MCP server exited unexpectedly: {stderr}")
            msg = json.loads(line)
            if msg.get("id") == req_id:
                if "error" in msg:
                    raise RuntimeError(f"JSON-RPC error for {method}: {msg['error']}")
                return msg.get("result", {})

    def notify(self, method: str, params: dict[str, Any] | None = None) -> None:
        assert self.proc.stdin is not None
        payload: dict[str, Any] = {"jsonrpc": "2.0", "method": method}
        if params is not None:
            payload["params"] = params
        self.proc.stdin.write(json.dumps(payload) + "\n")
        self.proc.stdin.flush()

    def initialize(self) -> None:
        self.request(
            "initialize",
            {
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": {"name": "prism-mcp-fuzz", "version": "0.1.0"},
            },
        )
        self.notify("notifications/initialized")

    def call_tool(self, name: str, arguments: dict[str, Any] | None = None) -> tuple[bool, str]:
        result = self.request("tools/call", {"name": name, "arguments": arguments or {}})
        is_error = bool(result.get("isError", False))
        texts = [
            item.get("text", "")
            for item in result.get("content", [])
            if isinstance(item, dict) and item.get("type") == "text"
        ]
        return is_error, "\n".join(texts)

    def close(self) -> None:
        if self.proc.poll() is None:
            self.proc.terminate()
            try:
                self.proc.wait(timeout=5)
            except subprocess.TimeoutExpired:
                self.proc.kill()
        for stream in (self.proc.stdin, self.proc.stdout, self.proc.stderr):
            if stream is not None:
                stream.close()


def find_panic_lines(log_text: str) -> list[str]:
    return [line for line in log_text.splitlines() if any(m in line for m in PANIC_MARKERS)]


def run_schema_fuzz(client: McpClient, project_dir: Path, seed: int, steps: int) -> list[str]:
    rng = random.Random(seed)
    trace: list[str] = []

    is_err, docs_raw = client.call_tool("documents_list", {"kind": "scene"})
    if is_err:
        raise RuntimeError(f"documents_list failed: {docs_raw}")
    docs_json = json.loads(docs_raw)
    raw_docs = docs_json.get("documents", []) if isinstance(docs_json, dict) else docs_json
    scene_paths = [d["path"] if isinstance(d, dict) else d for d in raw_docs]

    is_err, schema_raw = client.call_tool("schema_list", {})
    if is_err:
        raise RuntimeError(f"schema_list failed: {schema_raw}")

    scratch_rel: str | None = None
    scratch_abs: Path | None = None
    try:
        if scene_paths:
            src = project_dir / scene_paths[0]
            if src.is_file():
                scratch_dir = project_dir / "assets" / "scenes"
                scratch_dir.mkdir(parents=True, exist_ok=True)
                with tempfile.NamedTemporaryFile(
                    prefix=".fuzz-", suffix=".scene.json", dir=scratch_dir, delete=False
                ) as tmp:
                    scratch_abs = Path(tmp.name)
                shutil.copy2(src, scratch_abs)
                scratch_rel = scratch_abs.relative_to(project_dir).as_posix()

        for step in range(steps):
            op = rng.choice(["documents_list", "schema_list", "document_read", "entity_get", "toggle_active"])
            trace.append(f"step {step}: {op}")
            if op == "documents_list":
                err, out = client.call_tool("documents_list", {"kind": "all"})
                if err:
                    raise RuntimeError(f"{trace[-1]}: {out}")
            elif op == "schema_list":
                err, out = client.call_tool("schema_list", {})
                if err:
                    raise RuntimeError(f"{trace[-1]}: {out}")
            elif op == "document_read" and scratch_rel:
                err, out = client.call_tool("document_read", {"path": scratch_rel})
                if err:
                    raise RuntimeError(f"{trace[-1]}: {out}")
            elif op in {"entity_get", "toggle_active"} and scratch_rel:
                err, doc_text = client.call_tool("document_read", {"path": scratch_rel})
                if err:
                    raise RuntimeError(f"{trace[-1]}: {doc_text}")
                doc = json.loads(doc_text)
                entities = doc.get("entities", [])
                if not entities:
                    continue
                ent = rng.choice(entities)
                ent_id = str(ent["id"])
                if op == "entity_get":
                    err, out = client.call_tool("entity_get", {"path": scratch_rel, "entity": ent_id})
                    if err:
                        raise RuntimeError(f"{trace[-1]}: {out}")
                else:
                    client.call_tool(
                        "field_set",
                        {
                            "path": scratch_rel,
                            "entity": ent_id,
                            "component": "prism_engine::scene::Active",
                            "field": "0",
                            "value": rng.choice([True, False]),
                        },
                    )
    finally:
        if scratch_abs is not None and scratch_abs.exists():
            scratch_abs.unlink()

    return trace


def run_editor_fuzz(client: McpClient, seed: int, steps: int) -> list[str]:
    rng = random.Random(seed)
    trace: list[str] = []

    err, out = client.call_tool("editor_launch", {})
    if err:
        raise RuntimeError(f"editor_launch failed: {out}")

    try:
        for step in range(steps):
            err, snap_raw = client.call_tool("editor_ui", {"action": "snapshot"})
            if err:
                raise RuntimeError(f"step {step} snapshot failed: {snap_raw}")

            action = rng.choice(["panel", "undo", "redo", "play_stop", "logs"])
            trace.append(f"step {step}: {action}")
            if action == "panel":
                tab = rng.choice(["sceneView", "gameView", "hierarchy", "inspector", "log"])
                client.call_tool("editor_ui", {"action": "click", "node": f"panel:{tab}"})
            elif action == "undo":
                client.call_tool("editor_ui", {"action": "click", "node": "command:Undo"})
            elif action == "redo":
                client.call_tool("editor_ui", {"action": "click", "node": "command:Redo"})
            elif action == "play_stop":
                client.call_tool("editor_play", {"action": "play"})
                client.call_tool("editor_play", {"action": "stop"})
            elif action == "logs":
                err, logs = client.call_tool("editor_logs", {"limit": 200})
                if not err:
                    bad = find_panic_lines(logs)
                    if bad:
                        raise RuntimeError(f"panic/validation error in editor_logs: {bad[:3]}")
    finally:
        client.call_tool("editor_quit", {})

    return trace


def main() -> int:
    parser = argparse.ArgumentParser(description="Seeded MCP fuzzer for Prisma")
    parser.add_argument("--project", default="projects/template", help="project path relative to repo root")
    parser.add_argument("--mode", choices=("schema", "editor"), default="schema")
    parser.add_argument("--seed", type=int, default=42)
    parser.add_argument("--steps", type=int, default=25)
    parser.add_argument("--mcp-bin", help="path to prebuilt prism-editor-mcp binary")
    args = parser.parse_args()

    project_dir = (ROOT / args.project).resolve()
    cmd = (
        [args.mcp_bin, "--project", str(project_dir)]
        if args.mcp_bin
        else ["cargo", "run", "--quiet", "-p", "prism-editor-mcp", "--", "--project", str(project_dir)]
    )

    client = McpClient(cmd, cwd=ROOT)
    try:
        client.initialize()
        if args.mode == "schema":
            trace = run_schema_fuzz(client, project_dir, args.seed, args.steps)
        else:
            trace = run_editor_fuzz(client, args.seed, args.steps)
        print(f"mcp_fuzz: ok (mode={args.mode}, seed={args.seed}, steps={len(trace)})")
        return 0
    except Exception as exc:
        sys.stderr.write(f"mcp_fuzz: FAILED (seed={args.seed}): {exc}\n")
        return 1
    finally:
        client.close()


if __name__ == "__main__":
    raise SystemExit(main())
