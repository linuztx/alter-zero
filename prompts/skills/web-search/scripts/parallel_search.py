#!/usr/bin/env python3
"""Parallel web search with no API key, over Parallel's hosted Search MCP
(`https://search.parallel.ai/mcp`): one of the web-search skill's scripts.

    python3 parallel_search.py --list                     # Parallel's tools
    python3 parallel_search.py "how ratatui inline viewports work"
    python3 parallel_search.py "ratatui inline viewport docs" -q "ratatui Viewport::Inline" -q "ratatui insert_before"
    python3 parallel_search.py --fetch https://ratatui.rs/ --objective "what ratatui is"
    python3 parallel_search.py --json "..."               # the raw JSON text
    PARALLEL_API_KEY=... python3 parallel_search.py "..."

Standard library only; the MCP plumbing is `mcp_http.py`, which has to stay
in this directory. Tests: `test_parallel_search.py` beside it in the
repository (not seeded with the skill).

What it shows (checked against the live service, 2026-10):

- `tools/list` answers keyless, as plain JSON (not an SSE event), handing out
  an `Mcp-Session-Id` the client never has to echo.
- The tools are `web_search` (`objective` + `search_queries`, both required)
  and `web_fetch` (`urls`, optionally steered by `objective`). Both take an
  optional `session_id` — "a random value … at the start of your session",
  reused for every call in it. Parallel's docs say it feeds free-tier rate
  limiting, so `PARALLEL_SESSION_ID` keeps one across runs; without it a run
  mints its own.
- Responses name Parallel's terms and privacy policy in `x-parallel-terms`
  and `x-parallel-privacy`.
- A URL `web_fetch` could not read comes back in the document's `errors`,
  not among its results.
"""

import argparse
import json
import os
import sys
import uuid

# mcp_http.py sits beside this script. Python puts the script's directory on
# the import path by itself, but not in isolated mode (-I) or under
# PYTHONSAFEPATH, so put it there explicitly; and importing it must not leave
# a __pycache__ folder in the skill's directory.
sys.dont_write_bytecode = True
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from mcp_http import run, tool_call_body, tools_list_body  # noqa: E402

MCP_ENDPOINT = "https://search.parallel.ai/mcp"


def mcp_headers(key):
    """The request headers past the shared ones: the key, when there is one."""
    return {"Authorization": f"Bearer {key}"} if key else {}


def search_arguments(objective, queries, session_id):
    """`web_search`'s arguments. At least one query is required; the
    objective itself stands in when none is given."""
    return {
        "objective": objective,
        "search_queries": list(queries) or [objective],
        "session_id": session_id,
    }


def fetch_arguments(urls, objective, session_id):
    """`web_fetch`'s arguments; the objective is optional and steers which
    excerpts come back."""
    arguments = {"urls": list(urls), "session_id": session_id}
    if objective:
        arguments["objective"] = objective
    return arguments


def format_results(text):
    """Parallel's tool text — a JSON document of results with excerpts — as
    readable `Title:`/`URL:` blocks, the URLs a fetch could not read listed
    last. Text of any other shape passes through."""
    try:
        doc = json.loads(text)
    except json.JSONDecodeError:
        return text
    if not isinstance(doc, dict) or not isinstance(doc.get("results"), list):
        return text
    blocks = []
    for r in doc["results"]:
        block = (
            f"Title: {r.get('title') or '(untitled)'}\n"
            f"URL: {r.get('url')}\n"
            f"Published: {r.get('publish_date') or 'N/A'}\n"
        )
        excerpts = [e for e in r.get("excerpts") or [] if isinstance(e, str)]
        if excerpts:
            block += "\n" + "\n...\n".join(excerpts)
        blocks.append(block)
    out = "\n\n---\n\n".join(blocks)
    failures = [failure_line(e) for e in doc.get("errors") or [] if isinstance(e, dict)]
    if failures:
        out = "\n\n".join(filter(None, [out, "\n".join(failures)]))
    return out


def failure_line(error):
    """One entry of a fetch's `errors`: the URL and why it was not read."""
    reason = error.get("error_type") or "not read"
    if error.get("http_status_code"):
        reason += f", HTTP {error['http_status_code']}"
    return f"Failed: {error.get('url')} ({reason})"


def main(argv=None):
    parser = argparse.ArgumentParser(description="Parallel search, keyless, over MCP.")
    parser.add_argument("words", nargs="*", help="the objective, or URLs with --fetch")
    parser.add_argument(
        "-q", dest="queries", action="append", default=[], help="a keyword query (repeatable)"
    )
    parser.add_argument("--objective", help="what to look for in fetched pages")
    parser.add_argument("--fetch", action="store_true", help="read the URLs given")
    parser.add_argument("--list", action="store_true", help="list Parallel's MCP tools")
    parser.add_argument("--json", action="store_true", help="print the raw JSON text")
    args = parser.parse_args(argv)
    if not args.list and not args.words:
        parser.error("an objective (or URLs with --fetch) is required")
    key = os.environ.get("PARALLEL_API_KEY") or None

    session_id = os.environ.get("PARALLEL_SESSION_ID") or uuid.uuid4().hex
    if args.list:
        payload = tools_list_body()
    elif args.fetch:
        payload = tool_call_body(
            1, "web_fetch", fetch_arguments(args.words, args.objective, session_id)
        )
    else:
        payload = tool_call_body(
            1, "web_search", search_arguments(" ".join(args.words), args.queries, session_id)
        )
    return run(MCP_ENDPOINT, payload, mcp_headers(key), None if args.json else format_results)


if __name__ == "__main__":
    sys.exit(main())
