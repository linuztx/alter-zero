"""Tests for mcp_http.py — the shared decoding half, against the live
response shapes of the hosted search servers. No network.

    python3 -m unittest discover -s prompts/skills/web-search/scripts -p 'test_*.py'
"""

import importlib
import io
import os
import shutil
import socket
import subprocess
import sys
import tempfile
import threading
import time
import unittest
from contextlib import redirect_stderr, redirect_stdout
from unittest import mock

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import mcp_http  # noqa: E402
from mcp_http import (  # noqa: E402
    AuthRequired,
    ProtocolError,
    RateLimited,
    RpcError,
    call,
    decode,
    retry_after_secs,
    rpc_message,
    run,
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
# Tavily's answer to a key it does not know, live and verbatim: HTTP 401 and
# an OAuth error document, which is no JSON-RPC error for all its `error` key.
TAVILY_BAD_KEY = (
    '{"error": "invalid_token", "error_description": "Authentication failed. The'
    " provided bearer token is invalid, expired, or no longer recognized by the"
    " server. To resolve: clear authentication tokens in your MCP client and"
    ' reconnect. Your client should automatically re-register and obtain new tokens."}'
)
# Parallel's, live and verbatim: HTTP 401 and a document of its own.
PARALLEL_BAD_KEY = '{"code":16,"message":"Invalid API key (C.1)"}'


def gateway_page(reason):
    """What a gateway answers in a server's place: nginx's page for `reason`
    (`502 Bad Gateway`), byte for byte."""
    return (
        f"<html>\r\n<head><title>{reason}</title></head>\r\n<body>\r\n"
        f"<center><h1>{reason}</h1></center>\r\n</body>\r\n</html>\r\n"
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

    def test_an_event_stream_is_told_by_its_type_whatever_its_case(self):
        # A media type is case-insensitive, and may carry parameters.
        for content_type in ("Text/Event-Stream", "text/event-stream; charset=utf-8", " text/event-stream"):
            with self.subTest(content_type=content_type):
                self.assertEqual(rpc_message(content_type, SSE_LIST)["id"], 1)

    def test_an_event_stream_under_another_type_is_still_read(self):
        # A server, or a proxy in front of it, that drops or mislabels the
        # type still sent an answer.
        for content_type in ("", "application/octet-stream", "application/json"):
            with self.subTest(content_type=content_type):
                self.assertEqual(rpc_message(content_type, SSE_LIST)["id"], 1)

    def test_an_oauth_error_document_is_not_a_json_rpc_error(self):
        # Its `error` is a string; a JSON-RPC error is an object. Read as one,
        # it crashed the script on `error.get`.
        with self.assertRaises(ProtocolError) as caught:
            rpc_message("application/json", TAVILY_BAD_KEY)
        self.assertTrue(
            str(caught.exception).startswith("invalid_token: Authentication failed."),
            str(caught.exception),
        )

    def test_a_body_that_is_no_answer_is_named_for_what_it_is(self):
        # The JSON parser's own complaint ("Expecting value: line 1 column 1
        # (char 0)") says nothing about what the server sent.
        cases = {
            gateway_page("502 Bad Gateway"): 'a web page titled "502 Bad Gateway"',
            "<!DOCTYPE html>\n<html><body>nope</body></html>": "a web page",
            "": "an empty answer",
            "  \n": "an empty answer",
            "upstream connect error\nor disconnect": "upstream connect error or disconnect",
            PARALLEL_BAD_KEY: "Invalid API key (C.1)",
            '{"detail":"Not Found"}': "Not Found",
            '{"unexpected":true}': 'not a JSON-RPC response: {"unexpected":true}',
        }
        for body, words in cases.items():
            with self.subTest(body=body[:40]):
                with self.assertRaises(ProtocolError) as caught:
                    rpc_message("text/html", body)
                self.assertEqual(str(caught.exception), words)


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

    def test_a_gateways_page_is_named_under_its_status(self):
        with self.assertRaises(ProtocolError) as caught:
            decode(502, "text/html", gateway_page("502 Bad Gateway"))
        self.assertEqual(str(caught.exception), 'HTTP 502: a web page titled "502 Bad Gateway"')
        # A page under a success status is no answer either.
        with self.assertRaises(ProtocolError) as caught:
            decode(200, "text/html; charset=utf-8", "<html><head><title>Sign in</title></head></html>")
        self.assertEqual(str(caught.exception), 'HTTP 200: a web page titled "Sign in"')

    def test_a_429_from_in_front_of_the_server_is_rate_limited(self):
        # A gateway's own rate limit carries no JSON-RPC error to say so: a
        # page, a line of text, or nothing, under the status and its header.
        cases = {
            gateway_page("429 Too Many Requests"): 'a web page titled "429 Too Many Requests"',
            "Too Many Requests": "Too Many Requests",
            "": "an empty answer",
        }
        for body, words in cases.items():
            with self.subTest(body=body[:30]):
                with self.assertRaises(RateLimited) as caught:
                    decode(429, "text/html", body, None, 30)
                self.assertEqual(caught.exception.retry_after, 30)
                self.assertEqual(str(caught.exception), f"rate limited (retry in 30s): {words}")

    def test_a_refused_key_says_so_in_the_servers_words(self):
        # Tavily and Parallel both answer a key they do not know with a 401
        # and a document of their own; the request carried a key, so the
        # server did not want one, it refused this one.
        challenge = 'Bearer error="invalid_token", error_description="Authentication failed."'
        with self.assertRaises(AuthRequired) as caught:
            decode(401, "application/json", TAVILY_BAD_KEY, challenge, None, True)
        self.assertTrue(
            str(caught.exception).startswith(
                "HTTP 401: this server refused the key (invalid_token: Authentication failed. The"
            ),
            str(caught.exception),
        )
        with self.assertRaises(AuthRequired) as caught:
            decode(401, "application/json", PARALLEL_BAD_KEY, None, None, True)
        self.assertEqual(
            str(caught.exception), "HTTP 401: this server refused the key (Invalid API key (C.1))"
        )

    def test_a_401_to_a_keyless_request_still_needs_a_key(self):
        with self.assertRaises(AuthRequired) as caught:
            decode(401, "application/json", TAVILY_BAD_KEY)
        self.assertTrue(str(caught.exception).startswith("HTTP 401: this server needs a key ("))
        with self.assertRaises(AuthRequired) as caught:
            decode(401, "text/html", "")
        self.assertEqual(str(caught.exception), "HTTP 401: this server needs a key (no details)")


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

    def test_a_shape_no_schema_allows_is_an_error_never_a_crash(self):
        # Each of these raised AttributeError, which nothing catches: the
        # agent read a Python traceback in place of a reason.
        with self.assertRaises(ProtocolError) as caught:
            tool_text({"result": "ok"}, 200)
        self.assertEqual(str(caught.exception), 'not a tool result: "ok"')
        with self.assertRaises(RpcError) as caught:
            tool_text({"error": "boom"}, 200)
        self.assertEqual(caught.exception.message, "boom")
        with self.assertRaises(RpcError) as caught:
            tool_text({"error": {"code": -32602, "message": {"why": "bad"}}}, 200)
        self.assertEqual(caught.exception.message, '{"why": "bad"}')

    def test_content_that_is_not_text_parts_is_skipped(self):
        msg = {"result": {"content": ["plain", None, {"type": "text", "text": "kept"}, {"text": 7}]}}
        self.assertEqual(tool_text(msg, 200), "kept")
        self.assertEqual(tool_text({"result": {"content": "not a list"}}, 200), "")
        self.assertEqual(tool_text({"result": {"_meta": "odd", "content": []}}, 200), "")


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

    def test_tool_lines_skip_what_is_not_a_tool(self):
        self.assertEqual(tool_lines({"tools": ["odd", {"name": "t", "inputSchema": "odd"}]}), ["t  ()  required: []"])
        self.assertEqual(tool_lines("odd"), [])


# A search's answer, trimmed: one SSE event carrying the tool text.
SSE_RESULT = (
    "event: message\n"
    'data: {"jsonrpc":"2.0","id":1,"result":{"content":[{"type":"text","text":"Title: A"}]}}\n\n'
)


def answered(body):
    """The server stood in for: every request is answered with `body`."""
    return mock.patch(
        "mcp_http.post_json", return_value=(200, {"content-type": "text/event-stream"}, body)
    )


def captured(fn, *args):
    """`fn(*args)`'s exit code, stdout and stderr."""
    out, err = io.StringIO(), io.StringIO()
    with redirect_stdout(out), redirect_stderr(err):
        code = fn(*args)
    return code, out.getvalue(), err.getvalue()


class Quiet(unittest.TestCase):
    # A search prints its results and nothing else: a line per request on
    # stderr costs the agent tokens on every call and fills the collapsed
    # tool cell's few rows with an endpoint and a byte count.

    def test_a_successful_call_prints_its_text_and_nothing_on_stderr(self):
        with answered(SSE_RESULT):
            result = captured(run, "https://mcp.example/", tool_call_body(1, "t", {}), {})
        self.assertEqual(result, (0, "Title: A\n", ""))

    def test_a_failed_call_prints_only_its_error(self):
        message = rpc_message("text/event-stream", EXA_LIMITED_SSE)["result"]["content"][0]["text"]
        with answered(EXA_LIMITED_SSE):
            result = captured(run, "https://mcp.example/", tool_call_body(1, "t", {}), {})
        self.assertEqual(result, (1, "", f"rate limited: {message}\n"))

    def test_each_script_prints_its_results_and_nothing_on_stderr(self):
        for name in ("exa_search", "parallel_search", "tavily_search"):
            with self.subTest(script=name):
                with answered(SSE_RESULT):
                    code, out, err = captured(importlib.import_module(name).main, ["a query"])
                self.assertEqual((code, err), (0, ""))
                self.assertIn("Title: A", out)


def serve(respond):
    """A server on a loopback socket: each request's head and body are handed
    to `respond(connection, head)`, which answers and decides when to hang
    up. Returns its URL and the heads it was sent, lowercased."""
    listener = socket.socket()
    listener.bind(("127.0.0.1", 0))
    listener.listen(8)
    heads = []

    def loop():
        while True:
            try:
                connection, _ = listener.accept()
            except OSError:
                return
            data = b""
            while b"\r\n\r\n" not in data:
                chunk = connection.recv(65536)
                if not chunk:
                    break
                data += chunk
            head = data.split(b"\r\n\r\n")[0].decode("latin-1").lower()
            heads.append(head)
            threading.Thread(target=respond, args=(connection, head), daemon=True).start()

    threading.Thread(target=loop, daemon=True).start()
    return f"http://127.0.0.1:{listener.getsockname()[1]}/mcp", heads


class Wire(unittest.TestCase):
    # The request and the read themselves, against a real socket: everything
    # above stands a canned answer in for them.

    def test_an_answer_on_a_stream_the_server_holds_open_is_read_at_once(self):
        # A server may keep its event stream open after it has answered.
        # Reading to the stream's end waited out the whole timeout and then
        # failed, with the answer already received.
        def respond(connection, head):
            connection.sendall(
                b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n"
                b"Cache-Control: no-cache\r\n\r\n: keep-alive\n\n" + SSE_RESULT.encode()
            )
            time.sleep(20)
            connection.close()

        url, _ = serve(respond)
        started = time.monotonic()
        with mock.patch.object(mcp_http, "TIMEOUT_SECS", 5):
            msg, status, _ = call(url, tool_call_body(1, "t", {}), {})
        self.assertEqual(tool_text(msg, status), "Title: A")
        self.assertLess(time.monotonic() - started, 3)

    def test_a_json_answer_is_read_whole(self):
        body = b'{"jsonrpc":"2.0","id":1,"result":{"content":[{"type":"text","text":"Title: A"}]}}'

        def respond(connection, head):
            connection.sendall(
                b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n"
                + f"Content-Length: {len(body)}\r\nConnection: close\r\n\r\n".encode()
                + body
            )
            connection.close()

        url, _ = serve(respond)
        msg, status, _ = call(url, tool_call_body(1, "t", {}), {})
        self.assertEqual(tool_text(msg, status), "Title: A")

    def test_every_request_names_its_client_and_takes_either_answer(self):
        # Cloudflare refuses urllib's own `Python-urllib/…` signature, and a
        # streamable HTTP server a client that does not accept both shapes.
        def respond(connection, head):
            connection.sendall(b"HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n")
            connection.close()

        url, heads = serve(respond)
        with self.assertRaises(ProtocolError):
            call(url, tools_list_body(), {"X-Extra": "1"})
        self.assertIn(f"user-agent: {mcp_http.USER_AGENT}".lower(), heads[0])
        self.assertIn("accept: application/json, text/event-stream", heads[0])
        self.assertIn("content-type: application/json", heads[0])
        self.assertIn("x-extra: 1", heads[0])

    def test_a_refused_key_reaches_the_agent_as_a_reason(self):
        # End to end: the 401 a provider answers a key it does not know with.
        def respond(connection, head):
            body = TAVILY_BAD_KEY.encode()
            connection.sendall(
                b"HTTP/1.1 401 Unauthorized\r\nContent-Type: application/json\r\n"
                b'WWW-Authenticate: Bearer error="invalid_token"\r\n'
                + f"Content-Length: {len(body)}\r\nConnection: close\r\n\r\n".encode()
                + body
            )
            connection.close()

        url, _ = serve(respond)
        code, out, err = captured(
            run, url, tool_call_body(1, "t", {}), {"Authorization": "Bearer nope"}
        )
        self.assertEqual((code, out), (1, ""))
        self.assertTrue(
            err.startswith("HTTP 401: this server refused the key (invalid_token: Authentication"),
            err,
        )
        self.assertNotIn("Traceback", err)


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
