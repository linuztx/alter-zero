"""Tests for mcp_http.py — the shared decoding half, against the live
response shapes of the hosted search servers. No network.

    python3 -m unittest discover -s prompts/skills/web-search/scripts -p 'test_*.py'
"""

import os
import shutil
import subprocess
import sys
import tempfile
import unittest

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from mcp_http import (  # noqa: E402
    AuthRequired,
    ProtocolError,
    RateLimited,
    RpcError,
    decode,
    retry_after_secs,
    rpc_message,
    tool_call_body,
    tool_lines,
    tool_text,
    tools_list_body,
)

# The live `tools/list` answer's shape, trimmed: one SSE event.
SSE_LIST = (
    "event: message\n"
    'data: {"result":{"tools":[{"name":"web_search_exa"}]},"jsonrpc":"2.0","id":1}\n\n'
)
# The live 429 body, verbatim in shape: plain JSON, `id: null`.
RATE_LIMITED = (
    '{"jsonrpc":"2.0","error":{"code":-32000,'
    '"message":"You\'ve hit Exa\'s free MCP rate limit."},"id":null}'
)
# Parallel's free-tier refusal, live: HTTP 200, a JSON-RPC error saying so.
PARALLEL_LIMITED = (
    '{"jsonrpc":"2.0","id":1,"error":{"code":-32000,"message":'
    '"You\'ve hit the free-tier rate limit for Parallel Search MCP. To continue'
    ' with higher limits, add your own API key."}}'
)
# Exa's other refusal, live and verbatim: HTTP 200, one SSE event, the limit
# flagged in `_meta` and its text telling an MCP client how to reconfigure.
EXA_LIMITED_SSE = (
    "event: message\n"
    'data: {"result":{"_meta":{"ai.exa/rateLimited":true},"content":[{"type":"text",'
    '"text":"You\'ve hit Exa\'s free MCP rate limit. To continue using without limits,'
    " create your own Exa API key.\\n\\nFix: Create API key at"
    " https://dashboard.exa.ai/api-keys , and then update Exa MCP URL to this"
    ' https://mcp.exa.ai/mcp?exaApiKey=YOUR_EXA_API_KEY"}]},"jsonrpc":"2.0","id":1}\n\n'
)


class ToolCallBody(unittest.TestCase):
    def test_a_tool_call_is_a_bare_stateless_json_rpc_request(self):
        body = tool_call_body(7, "web_search_exa", {"query": "q"})
        self.assertEqual(body["jsonrpc"], "2.0")
        self.assertEqual(body["id"], 7)
        self.assertEqual(body["method"], "tools/call")
        self.assertEqual(body["params"]["name"], "web_search_exa")
        self.assertEqual(body["params"]["arguments"]["query"], "q")
        self.assertNotIn("sessionId", body)


class RpcMessage(unittest.TestCase):
    def test_an_event_stream_yields_its_response_event(self):
        msg = rpc_message("text/event-stream", SSE_LIST)
        self.assertEqual(msg["result"]["tools"][0]["name"], "web_search_exa")

    def test_an_event_stream_without_a_closing_blank_line_still_parses(self):
        self.assertEqual(rpc_message("text/event-stream", SSE_LIST.rstrip())["id"], 1)

    def test_notifications_ahead_of_the_response_are_skipped(self):
        body = (
            'data: {"jsonrpc":"2.0","method":"notifications/progress"}\n\n'
            'data: {"jsonrpc":"2.0","id":1,"result":{}}\n\n'
        )
        self.assertEqual(rpc_message("text/event-stream", body)["id"], 1)

    def test_multi_line_data_fields_join_with_newlines(self):
        body = 'data: {"jsonrpc":"2.0",\ndata: "id":1,"result":{}}\n\n'
        self.assertEqual(rpc_message("text/event-stream", body)["id"], 1)

    def test_a_plain_json_body_parses_as_is(self):
        msg = rpc_message("application/json", RATE_LIMITED)
        self.assertEqual(msg["error"]["code"], -32000)

    def test_a_json_body_that_is_not_json_rpc_is_a_protocol_error(self):
        # Cloudflare's 1010 refusal, which a banned user agent gets as JSON.
        body = (
            '{"title":"Error 1010: Access denied","status":403,'
            '"detail":"The site owner has blocked access based on your browser\'s signature."}'
        )
        with self.assertRaises(ProtocolError) as caught:
            rpc_message("application/json; charset=utf-8", body)
        self.assertIn("Error 1010: Access denied", str(caught.exception))

    def test_an_event_stream_with_no_response_is_a_protocol_error(self):
        with self.assertRaises(ProtocolError):
            rpc_message("text/event-stream", "event: ping\n\n")


class Decode(unittest.TestCase):
    def test_a_plain_json_rpc_answer_is_returned_whatever_the_status(self):
        # Parallel's shape: the JSON-RPC response as plain JSON.
        body = '{"jsonrpc":"2.0","id":1,"result":{"content":[]}}'
        self.assertEqual(decode(200, "application/json", body)["id"], 1)

    def test_a_401_with_no_json_rpc_body_needs_a_key(self):
        # Tavily's keyless answer: an empty 401 naming its OAuth metadata.
        challenge = 'Bearer scope="openid offline_access"'
        with self.assertRaises(AuthRequired) as caught:
            decode(401, "", "", challenge)
        self.assertIn("needs a key", str(caught.exception))
        self.assertIn(challenge, str(caught.exception))

    def test_another_error_status_with_no_json_rpc_body_names_the_status(self):
        with self.assertRaises(ProtocolError) as caught:
            decode(502, "text/html", "<html>bad gateway</html>")
        self.assertIn("HTTP 502", str(caught.exception))

    def test_a_429_json_rpc_error_still_decodes_for_tool_text(self):
        msg = decode(429, "application/json", RATE_LIMITED)
        with self.assertRaises(RateLimited):
            tool_text(msg, 429)


class ToolText(unittest.TestCase):
    def test_a_429_error_is_rate_limited_with_the_servers_countdown(self):
        msg = rpc_message("application/json", RATE_LIMITED)
        with self.assertRaises(RateLimited) as caught:
            tool_text(msg, 429, 48073)
        self.assertEqual(caught.exception.retry_after, 48073)
        self.assertEqual(
            str(caught.exception),
            "rate limited (retry in 13h 21m): You've hit Exa's free MCP rate limit.",
        )

    def test_a_rate_limit_error_on_a_200_is_rate_limited(self):
        # Parallel answers its free-tier limit with HTTP 200, not 429.
        msg = rpc_message("application/json", PARALLEL_LIMITED)
        with self.assertRaises(RateLimited) as caught:
            tool_text(msg, 200)
        self.assertTrue(
            str(caught.exception).startswith(
                "rate limited: You've hit the free-tier rate limit for Parallel Search MCP."
            )
        )
        self.assertIsNone(caught.exception.retry_after)

    def test_a_rate_limit_passes_the_providers_message_on_word_for_word(self):
        # The agent reads the limit as the provider states it, line breaks
        # and all, rather than a summary of it.
        msg = rpc_message("text/event-stream", EXA_LIMITED_SSE)
        with self.assertRaises(RateLimited) as caught:
            tool_text(msg, 200)
        self.assertEqual(
            str(caught.exception),
            "rate limited: You've hit Exa's free MCP rate limit. To continue using without"
            " limits, create your own Exa API key.\n\nFix: Create API key at"
            " https://dashboard.exa.ai/api-keys , and then update Exa MCP URL to this"
            " https://mcp.exa.ai/mcp?exaApiKey=YOUR_EXA_API_KEY",
        )

    def test_a_server_error_on_another_status_is_an_rpc_error(self):
        # -32000 is JSON-RPC's generic server-error code; on its own it is no
        # rate limit.
        body = '{"jsonrpc":"2.0","id":1,"error":{"code":-32000,"message":"Internal error"}}'
        msg = rpc_message("application/json", body)
        with self.assertRaises(RpcError) as caught:
            tool_text(msg, 200)
        self.assertEqual(caught.exception.code, -32000)

    def test_a_200_result_flagged_rate_limited_in_meta_is_rate_limited(self):
        msg = {
            "result": {
                "_meta": {"ai.exa/rateLimited": True},
                "content": [{"type": "text", "text": "free MCP rate limit"}],
            }
        }
        with self.assertRaises(RateLimited) as caught:
            tool_text(msg, 200)
        self.assertEqual(str(caught.exception), "rate limited: free MCP rate limit")
        self.assertIsNone(caught.exception.retry_after)

    def test_result_text_parts_join(self):
        msg = {
            "result": {
                "content": [
                    {"type": "text", "text": "Title: A"},
                    {"type": "text", "text": "Title: B"},
                ]
            }
        }
        self.assertEqual(tool_text(msg, 200), "Title: A\nTitle: B")

    def test_a_tool_result_flagged_is_error_is_an_error(self):
        msg = {
            "result": {"isError": True, "content": [{"type": "text", "text": "bad query"}]}
        }
        with self.assertRaises(RpcError) as caught:
            tool_text(msg, 200)
        self.assertEqual(caught.exception.message, "bad query")


class Helpers(unittest.TestCase):
    def test_a_tools_list_body_is_a_bare_json_rpc_request(self):
        self.assertEqual(
            tools_list_body(),
            {"jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": {}},
        )

    def test_a_retry_time_names_only_the_units_that_matter(self):
        cases = {
            48073: "rate limited (retry in 13h 21m): Limit reached.",
            3600: "rate limited (retry in 1h 0m): Limit reached.",
            948: "rate limited (retry in 15m): Limit reached.",
            45: "rate limited (retry in 45s): Limit reached.",
            2 * 86400 + 3 * 3600 + 5: "rate limited (retry in 2d 3h): Limit reached.",
            None: "rate limited: Limit reached.",
        }
        for secs, text in cases.items():
            with self.subTest(secs=secs):
                self.assertEqual(str(RateLimited("Limit reached.", secs)), text)

    def test_retry_after_reads_whole_seconds_only(self):
        self.assertEqual(retry_after_secs("48073"), 48073)
        self.assertIsNone(retry_after_secs(None))
        self.assertIsNone(retry_after_secs("Wed, 21 Oct 2026 07:28:00 GMT"))

    def test_tool_lines_name_arguments_and_required_ones(self):
        result = {
            "tools": [
                {
                    "name": "web_search",
                    "inputSchema": {
                        "properties": {"objective": {}, "search_queries": {}},
                        "required": ["objective"],
                    },
                }
            ]
        }
        self.assertEqual(
            tool_lines(result),
            ["web_search  (objective, search_queries)  required: ['objective']"],
        )

    def test_any_rate_limited_meta_flag_counts(self):
        msg = {"result": {"_meta": {"vendor.example/rateLimited": True}, "content": []}}
        with self.assertRaises(RateLimited):
            tool_text(msg, 200)


class SiblingImport(unittest.TestCase):
    def test_every_mcp_script_starts_without_its_directory_on_the_path(self):
        # Each script imports this module from its own directory. Isolated
        # mode (-I, which also implies PYTHONSAFEPATH's -P) leaves that
        # directory off sys.path, and an agent may well run a script that way.
        here = os.path.dirname(os.path.abspath(__file__))
        for script in ("exa_search.py", "parallel_search.py", "tavily_search.py"):
            with self.subTest(script=script):
                done = subprocess.run(
                    [sys.executable, "-I", os.path.join(here, script), "--help"],
                    capture_output=True,
                    text=True,
                    timeout=60,
                )
                self.assertEqual(done.returncode, 0, done.stderr)
                self.assertIn("usage:", done.stdout)

    def test_running_a_script_leaves_no_pycache_beside_it(self):
        # The scripts are seeded into the user's skills folder, where
        # importing this module would leave a __pycache__ folder behind.
        here = os.path.dirname(os.path.abspath(__file__))
        env = {
            k: v
            for k, v in os.environ.items()
            if k not in ("PYTHONDONTWRITEBYTECODE", "PYTHONPYCACHEPREFIX")
        }
        for script in ("exa_search.py", "parallel_search.py", "tavily_search.py"):
            with self.subTest(script=script), tempfile.TemporaryDirectory() as tmp:
                for name in (script, "mcp_http.py"):
                    shutil.copy(os.path.join(here, name), tmp)
                done = subprocess.run(
                    [sys.executable, os.path.join(tmp, script), "--help"],
                    capture_output=True,
                    text=True,
                    timeout=60,
                    env=env,
                )
                self.assertEqual(done.returncode, 0, done.stderr)
                self.assertEqual(sorted(os.listdir(tmp)), sorted([script, "mcp_http.py"]))


if __name__ == "__main__":
    unittest.main()
