import json
import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import mcp_fuzz


class McpFuzzTests(unittest.TestCase):
    def test_find_panic_lines_detects_panic_and_validation(self) -> None:
        logs = "\n".join(
            [
                "INFO frame 12 ok",
                "thread 'main' panicked at crates/prism-render/src/lib.rs:10",
                "ERROR Validation Error: [ VUID-vkDestroyDevice-device-00378 ]",
            ]
        )
        hits = mcp_fuzz.find_panic_lines(logs)
        self.assertEqual(len(hits), 2)

    def test_mcp_client_echo_server_roundtrip(self) -> None:
        script = (
            "import json, sys\n"
            "for line in sys.stdin:\n"
            "    msg = json.loads(line)\n"
            "    if 'id' not in msg:\n"
            "        continue\n"
            "    if msg['method'] == 'initialize':\n"
            "        resp = {'jsonrpc': '2.0', 'id': msg['id'], 'result': {'protocolVersion': '2024-11-05'}}\n"
            "    elif msg['method'] == 'tools/call':\n"
            "        name = msg['params']['name']\n"
            "        if name == 'documents_list':\n"
            "            text = json.dumps({'documents': []})\n"
            "        else:\n"
            "            text = json.dumps({'components': []})\n"
            "        resp = {'jsonrpc': '2.0', 'id': msg['id'], 'result': {'isError': False, 'content': [{'type': 'text', 'text': text}]}}\n"
            "    else:\n"
            "        resp = {'jsonrpc': '2.0', 'id': msg['id'], 'result': {}}\n"
            "    sys.stdout.write(json.dumps(resp) + '\\n')\n"
            "    sys.stdout.flush()\n"
        )
        client = mcp_fuzz.McpClient([sys.executable, "-c", script], cwd=Path.cwd())
        try:
            client.initialize()
            trace = mcp_fuzz.run_schema_fuzz(client, Path.cwd(), seed=123, steps=8)
            self.assertEqual(len(trace), 8)
        finally:
            client.close()


if __name__ == "__main__":
    unittest.main()
