#!/usr/bin/env python3
"""Tavily web search with no API key, over Tavily's hosted MCP
(`https://mcp.tavily.com/mcp/`) and its keyless access header: one of the
web-search skill's scripts.

    python3 tavily_search.py --list                      # Tavily's tools
    python3 tavily_search.py "ratatui inline viewport"   # keyless search
    python3 tavily_search.py -n 3 --depth advanced "ratatui inline viewport"
    python3 tavily_search.py --extract https://ratatui.rs/ --query "what ratatui is"
    python3 tavily_search.py --json "..."                # the raw JSON text
    TAVILY_API_KEY=tvly-... python3 tavily_search.py "..."

Standard library only; the MCP plumbing is `mcp_http.py`, which has to stay
in this directory. Tests: `test_tavily_search.py` beside it in the
repository (not seeded with the skill).

What it shows (checked against the live service, 2026-10):

- Without a key the server answers 401 with an OAuth challenge — unless the
  request carries `X-Tavily-Access-Mode: keyless`, Tavily's documented free
  tier (docs.tavily.com/documentation/keyless): free but rate-limited, no
  published number, and only search and extract (`tavily_crawl`, `_map` and
  `_research` still need a key).
- With the header a bare, stateless `tools/call` works — no `initialize`, no
  session — answered as one SSE event. The tool text is the REST response as
  a JSON string, `auth_mode: "keyless"` included.
- A key goes as `Authorization: Bearer tvly-…` (or `?tavilyApiKey=`); it
  outranks the keyless header, and `x-api-key` is not read here. A bad key
  comes back as HTTP 200, `isError: false`, with an `error` document as the
  text — which is why `format_results` checks for one.
- Past the keyless quota (monthly, per IP) the answer is still HTTP 200 and
  `isError: false`: a `{"code": "monthly_cap_reached…", "message", "window",
  "retry_after_seconds", "next_actions"}` document. Its next actions are x402
  pay-per-call, signing up for a key, or bonus keyless credits for answering
  two survey questions at `/keyless/bonus` — this script takes none of them.
"""

import argparse
import json
import os
import sys

# mcp_http.py sits beside this script. Python puts the script's directory on
# the import path by itself, but not in isolated mode (-I) or under
# PYTHONSAFEPATH, so put it there explicitly; and importing it must not leave
# a __pycache__ folder in the skill's directory.
sys.dont_write_bytecode = True
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from mcp_http import (  # noqa: E402
    RateLimited,
    SearchError,
    log,
    retry_after_secs,
    run,
    tool_call_body,
    tools_list_body,
)

MCP_ENDPOINT = "https://mcp.tavily.com/mcp/"
SEARCH_DEPTHS = ("basic", "advanced", "fast", "ultra-fast")


def mcp_headers(key):
    """The request headers past the shared ones: the key as a bearer token
    when there is one, else Tavily's keyless access header."""
    if key:
        return {"Authorization": f"Bearer {key}"}
    return {"X-Tavily-Access-Mode": "keyless"}


def search_arguments(query, max_results, depth):
    """`tavily_search`'s arguments (`query` is the only required one)."""
    return {"query": query, "max_results": max_results, "search_depth": depth}


def extract_arguments(urls, query):
    """`tavily_extract`'s arguments; `query` is optional and reranks the
    extracted chunks toward it."""
    arguments = {"urls": list(urls), "format": "markdown"}
    if query:
        arguments["query"] = query
    return arguments


def format_results(text):
    """Tavily's tool text — the REST response as JSON — as readable
    `Title:`/`URL:` blocks, an answer leading when there is one and failed
    extractions listed last. An `error` document is raised; text of any other
    shape passes through."""
    try:
        doc = json.loads(text)
    except json.JSONDecodeError:
        return text
    if not isinstance(doc, dict):
        return text
    if doc.get("error"):
        detail = doc.get("detail")
        if isinstance(detail, dict):
            detail = detail.get("error") or json.dumps(detail)
        status = f" ({doc['status']})" if doc.get("status") else ""
        raise SearchError(f"{doc['error']}{status}: {detail or 'no details'}")
    results = doc.get("results")
    failed = doc.get("failed_results")
    if not isinstance(results, list) and not isinstance(failed, list):
        # A refusal with no results: past the keyless quota it is
        # `{"code": "monthly_cap_reached…", "message", "window",
        # "retry_after_seconds", …}`, still HTTP 200 and `isError: false`.
        if isinstance(doc.get("code"), str) and doc.get("message"):
            if "window" in doc or "retry_after_seconds" in doc:
                retry_after = retry_after_secs(str(doc.get("retry_after_seconds")))
                raise RateLimited(doc["message"], retry_after)
            raise SearchError(f"{doc['code']}: {doc['message']}")
        return text
    blocks = []
    if doc.get("answer"):
        blocks.append(f"Answer: {doc['answer']}")
    for r in results or []:
        block = f"Title: {r.get('title') or '(untitled)'}\nURL: {r.get('url')}\n"
        body = r.get("raw_content") or r.get("content")
        if body:
            block += "\n" + body.strip()
        blocks.append(block)
    out = "\n\n---\n\n".join(blocks)
    failures = [f"Failed: {f.get('url')} ({f.get('error')})" for f in failed or []]
    if failures:
        out = "\n\n".join(filter(None, [out, "\n".join(failures)]))
    return out


def main(argv=None):
    parser = argparse.ArgumentParser(description="Tavily search, keyless, over MCP.")
    parser.add_argument("words", nargs="*", help="the query, or URLs with --extract")
    parser.add_argument("-n", type=int, default=5, help="number of results (default 5)")
    parser.add_argument("--depth", choices=SEARCH_DEPTHS, default="basic", help="search depth")
    parser.add_argument("--query", help="what to rank first in extracted pages")
    parser.add_argument("--extract", action="store_true", help="read the URLs given")
    parser.add_argument("--list", action="store_true", help="list Tavily's MCP tools")
    parser.add_argument("--json", action="store_true", help="print the raw JSON text")
    args = parser.parse_args(argv)
    if not args.list and not args.words:
        parser.error("a query (or URLs with --extract) is required")
    key = os.environ.get("TAVILY_API_KEY") or None
    log(f"{MCP_ENDPOINT} · {'TAVILY_API_KEY' if key else 'keyless'}")

    if args.list:
        payload = tools_list_body()
    elif args.extract:
        payload = tool_call_body(1, "tavily_extract", extract_arguments(args.words, args.query))
    else:
        payload = tool_call_body(
            1, "tavily_search", search_arguments(" ".join(args.words), args.n, args.depth)
        )
    return run(MCP_ENDPOINT, payload, mcp_headers(key), None if args.json else format_results)


if __name__ == "__main__":
    sys.exit(main())
