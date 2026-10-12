"""Shared core of the web-search skill's MCP scripts (`exa_search.py`,
`parallel_search.py` and `tavily_search.py`, which import it from this
directory): one JSON-RPC request to a hosted MCP server over streamable HTTP,
and its answer decoded.

The hosted search servers these scripts reach answer a bare, stateless
`tools/call` POST (no `initialize`, no session) in one of two shapes: a single
SSE `event: message` whose `data:` is the JSON-RPC response (Exa), or the
JSON-RPC response as plain JSON (Parallel). Standard library only; urllib
honours HTTPS_PROXY by itself and OpenSSL's default trust store honours
SSL_CERT_FILE, so this works behind a proxy unchanged.

Tests: `test_mcp_http.py` beside it in the repository (not seeded with the
skill).
"""

import json
import sys
import time
import urllib.error
import urllib.request

# A search result page is a few KB of text; bound the read anyway.
BODY_MAX_BYTES = 4 * 1024 * 1024
TIMEOUT_SECS = 60
# Cloudflare in front of mcp.exa.ai refuses urllib's default `Python-urllib/…`
# signature outright (error 1010), so every client names itself.
USER_AGENT = "alter-zero-search/0.1"
# Streamable HTTP servers refuse a client that does not accept both (406).
ACCEPT = "application/json, text/event-stream"


class SearchError(Exception):
    """A call that produced no text."""


class RateLimited(SearchError):
    """The free quota is spent; `retry_after` is the server's own countdown.

    `message` is the server's own text, passed on word for word, so the agent
    reads the limit as the provider states it."""

    def __init__(self, message, retry_after=None):
        super().__init__(message)
        self.message = message
        self.retry_after = retry_after

    def __str__(self):
        when = ""
        if self.retry_after is not None:
            when = f" (retry in {duration(self.retry_after)})"
        return f"rate limited{when}: {self.message}"


def duration(secs):
    """`secs` read at a glance: `2d 3h`, `13h 21m`, `15m`, `45s`."""
    days, rest = divmod(secs, 86400)
    hours, rest = divmod(rest, 3600)
    minutes, seconds = divmod(rest, 60)
    if days:
        return f"{days}d {hours}h"
    if hours:
        return f"{hours}h {minutes}m"
    if minutes:
        return f"{minutes}m"
    return f"{seconds}s"


class RpcError(SearchError):
    """A JSON-RPC error, or a tool result flagged `isError`."""

    def __init__(self, code, message):
        super().__init__(f"error {code}: {message}")
        self.code = code
        self.message = message


class ProtocolError(SearchError):
    """The body was not a JSON-RPC response we can read."""


class AuthRequired(SearchError):
    """The server refuses a keyless client (HTTP 401)."""


def tool_call_body(request_id, tool, arguments):
    """The JSON-RPC `tools/call` body: the whole request, no session."""
    return {
        "jsonrpc": "2.0",
        "id": request_id,
        "method": "tools/call",
        "params": {"name": tool, "arguments": arguments},
    }


def tools_list_body(request_id=1):
    """The JSON-RPC `tools/list` body."""
    return {"jsonrpc": "2.0", "id": request_id, "method": "tools/list", "params": {}}


def rpc_message(content_type, body):
    """The JSON-RPC message a response body carries.

    An event stream yields its first event that is a response (`result` or
    `error`); multi-line `data:` fields join with a newline, per the SSE spec.
    Anything else is parsed as the JSON message itself.
    """
    if not content_type.startswith("text/event-stream"):
        try:
            msg = json.loads(body)
        except json.JSONDecodeError as e:
            raise ProtocolError(str(e)) from e
        if isinstance(msg, dict) and ("result" in msg or "error" in msg):
            return msg
        # A refusal from in front of the server (Cloudflare's JSON errors carry
        # `title`/`detail`), not a JSON-RPC response.
        if isinstance(msg, dict) and msg.get("title"):
            raise ProtocolError(f"{msg['title']}: {msg.get('detail', '')}")
        raise ProtocolError(f"not a JSON-RPC response: {body[:300]}")
    data = []
    # A trailing blank line ends the last event, so a body without one still
    # flushes it.
    for line in body.splitlines() + [""]:
        if not line:
            if data:
                try:
                    msg = json.loads("\n".join(data))
                except json.JSONDecodeError:
                    msg = None
                if isinstance(msg, dict) and ("result" in msg or "error" in msg):
                    return msg
                data = []
        elif line.startswith("data:"):
            rest = line[len("data:") :]
            data.append(rest[1:] if rest.startswith(" ") else rest)
    raise ProtocolError("no JSON-RPC response in the event stream")


def decode(status, content_type, body, www_authenticate=None):
    """`rpc_message`, with the HTTP status naming what a non-answer means.

    A 401 is a server that wants a key (its `WWW-Authenticate` says how); any
    other error status with no JSON-RPC body is reported with the status.
    """
    try:
        return rpc_message(content_type, body)
    except ProtocolError as e:
        if status == 401:
            raise AuthRequired(
                f"HTTP 401: this server needs a key ({www_authenticate or body.strip() or 'no details'})"
            ) from e
        if status >= 400:
            raise ProtocolError(f"HTTP {status}: {e}") from e
        raise


def joined_text(result):
    """A tool result's `content[].text` parts, joined."""
    parts = result.get("content") or []
    return "\n".join(p["text"] for p in parts if isinstance(p.get("text"), str))


def tool_text(msg, status, retry_after=None):
    """The text a JSON-RPC response carries, or the error it names.

    `status` and `retry_after` turn a 429 into `RateLimited`, as does an
    error that says it is a rate limit whatever the status (Parallel answers
    its free-tier limit with HTTP 200).
    """
    error = msg.get("error")
    if error is not None:
        message = error.get("message") or ""
        if status == 429 or "rate limit" in message.lower():
            raise RateLimited(message, retry_after)
        raise RpcError(error.get("code", 0), message)
    result = msg.get("result") or {}
    # A rate limit can also arrive as a 200 tool result flagged in `_meta`
    # (Exa's `ai.exa/rateLimited`).
    meta = result.get("_meta") or {}
    if any(k.rsplit("/", 1)[-1] == "rateLimited" and v is True for k, v in meta.items()):
        raise RateLimited(joined_text(result), retry_after)
    text = joined_text(result)
    if result.get("isError") is True:
        raise RpcError(0, text)
    return text


def retry_after_secs(value):
    """A `Retry-After` header in whole seconds, or None (the HTTP-date form
    is left alone — no server here sends it)."""
    return int(value) if value and value.strip().isdigit() else None


def tool_lines(result):
    """One line per tool a `tools/list` result names: its arguments, and
    which of them are required."""
    lines = []
    for tool in result.get("tools") or []:
        schema = tool.get("inputSchema") or {}
        props = ", ".join(schema.get("properties") or {})
        lines.append(f"{tool.get('name')}  ({props})  required: {schema.get('required', [])}")
    return lines


def post_json(url, payload, headers):
    """POST `payload` as JSON; return (status, headers, body text).

    An HTTP error status is a response like any other here, not an exception.
    """
    request = urllib.request.Request(
        url,
        data=json.dumps(payload).encode(),
        headers={"Content-Type": "application/json", "User-Agent": USER_AGENT, **headers},
        method="POST",
    )
    try:
        response = urllib.request.urlopen(request, timeout=TIMEOUT_SECS)
    except urllib.error.HTTPError as e:
        response = e
    with response:
        body = response.read(BODY_MAX_BYTES).decode("utf-8", errors="replace")
        return response.status, response.headers, body


def call(url, payload, headers):
    """One MCP round trip: the decoded JSON-RPC message, plus status and
    `retry-after`, logging the exchange to stderr."""
    started = time.monotonic()
    status, resp_headers, body = post_json(url, payload, {"Accept": ACCEPT, **headers})
    content_type = resp_headers.get("content-type", "")
    limit = resp_headers.get("x-ratelimit-limit")
    remaining = resp_headers.get("x-ratelimit-remaining")
    quota = f" · quota {remaining}/{limit} left" if limit and remaining else ""
    log(
        f"HTTP {status} · {content_type or 'no body type'} · {len(body)} bytes"
        f" · {time.monotonic() - started:.2f}s{quota}"
    )
    msg = decode(status, content_type, body, resp_headers.get("www-authenticate"))
    return msg, status, retry_after_secs(resp_headers.get("retry-after"))


def run(url, payload, headers, render=None):
    """Make the call and print what it answered: a tool listing for
    `tools/list`, else the tool's text, passed through `render` when given.
    Returns the process exit code."""
    try:
        msg, status, retry_after = call(url, payload, headers)
        if payload["method"] == "tools/list" and "result" in msg:
            print("\n".join(tool_lines(msg["result"])))
        else:
            text = tool_text(msg, status, retry_after)
            print(render(text) if render else text)
        return 0
    except (SearchError, OSError) as e:
        print(e, file=sys.stderr)
        return 1


def log(line):
    print(f"· {line}", file=sys.stderr)
