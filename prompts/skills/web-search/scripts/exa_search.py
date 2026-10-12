#!/usr/bin/env python3
"""Exa web search with no API key, over Exa's hosted MCP endpoint: one of the
web-search skill's scripts.

    python3 exa_search.py --list                       # Exa's tools (free, never counted)
    python3 exa_search.py "ratatui inline viewport"    # keyless search
    python3 exa_search.py -n 3 --objective "official docs first" "ratatui inline viewport"
    python3 exa_search.py --fetch https://ratatui.rs/  # keyless page fetch
    python3 exa_search.py --rest "ratatui"             # no MCP: the REST API
    EXA_API_KEY=... python3 exa_search.py "..."        # past the free quota

Standard library only. The MCP plumbing is `mcp_http.py`, which has to stay
in this directory. Tests: `test_exa_search.py` beside it in the repository
(not seeded with the skill).

What it shows (checked against the live service, 2026-10):

- `https://mcp.exa.ai/mcp` answers a bare, stateless JSON-RPC `tools/call`
  POST with no `initialize` handshake and no session. It needs
  `Accept: application/json, text/event-stream`. A success comes back as one
  SSE `event: message` whose `data:` line is the JSON-RPC response
  (`result.content[].text`); an error comes back as plain JSON.
- Keyless use is a daily per-IP quota. Past it the endpoint answers HTTP 429
  with a JSON-RPC -32000 error and `retry-after` counting down to 00:00 UTC,
  or HTTP 200 with `result._meta["ai.exa/rateLimited"]`. A shared cloud IP
  may get no quota at all (`x-ratelimit-limit: 0`).
- `EXA_API_KEY` lifts the limit, sent as `Authorization: Bearer {key}`.
- Without MCP (`--rest`) there is no free keyless path:
  `api.exa.ai/search` answers HTTP 402 under the x402 pay-per-call protocol
  ($0.007 a search, paid in USDC), `/answer` a 401. With a key it is the
  ordinary API, billed against the account's credits.
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
    SearchError,
    log,
    post_json,
    run,
    tool_call_body,
    tools_list_body,
)

MCP_ENDPOINT = "https://mcp.exa.ai/mcp"
REST_SEARCH = "https://api.exa.ai/search"


def mcp_headers(key):
    """The request headers past the shared ones: Exa's attribution header,
    and the key, when there is one, as a bearer token (the route the 429's
    own message names)."""
    headers = {"x-exa-source": "alter-zero"}
    if key:
        headers["Authorization"] = f"Bearer {key}"
    return headers


def search_arguments(query, num_results, objective=None):
    """`web_search_exa`'s arguments. `objective` is required by the live
    schema; the query itself is a fair default for a one-shot search."""
    return {"query": query, "objective": objective or query, "numResults": num_results}


def x402_price_usd(body):
    """The cheapest per-call price an x402 `402 Payment Required` body names,
    in US dollars (`accepts[].extra.totalUsd`), or None."""
    prices = [
        a["extra"]["totalUsd"]
        for a in body.get("accepts") or []
        if isinstance((a.get("extra") or {}).get("totalUsd"), (int, float))
    ]
    return min(prices) if prices else None


def format_rest_results(body):
    """The few fields of a REST `/search` answer this script prints."""
    blocks = []
    for r in body.get("results") or []:
        blocks.append(
            f"Title: {r.get('title') or '(untitled)'}\n"
            f"URL: {r.get('url')}\n"
            f"Published: {r.get('publishedDate') or '-'}\n"
            f"{(r.get('text') or '').strip()}\n"
        )
    return "\n".join(blocks)


def rest_search(query, num_results, key):
    """`POST /search` on the REST API, no MCP anywhere. Keyless, it reports
    the price the 402 asks; keyed, the results."""
    payload = {
        "query": query,
        "numResults": num_results,
        "type": "auto",
        "contents": {"text": {"maxCharacters": 600}},
    }
    headers = {"x-api-key": key} if key else {}
    status, _, body = post_json(REST_SEARCH, payload, headers)
    log(f"HTTP {status} · {len(body)} bytes")
    if status == 402:
        price = x402_price_usd(json.loads(body))
        price = f"${price:.3f}" if price is not None else "an unnamed price"
        raise SearchError(
            "402 Payment Required: no free keyless REST access - x402 pay-per-call"
            f" at {price} a search (crypto wallet), or set EXA_API_KEY"
        )
    if status != 200:
        raise SearchError(f"HTTP {status}: {body}")
    return format_rest_results(json.loads(body))


def main(argv=None):
    parser = argparse.ArgumentParser(description="Exa search, keyless, over MCP.")
    parser.add_argument("words", nargs="*", help="the query, or URLs with --fetch")
    parser.add_argument("-n", type=int, default=5, help="number of results (default 5)")
    parser.add_argument("--objective", help="what to rank first (default: the query)")
    parser.add_argument("--fetch", action="store_true", help="read the URLs given")
    parser.add_argument("--list", action="store_true", help="list Exa's MCP tools")
    parser.add_argument("--rest", action="store_true", help="the REST API, no MCP")
    args = parser.parse_args(argv)
    if not args.list and not args.words:
        parser.error("a query (or URLs with --fetch) is required")
    key = os.environ.get("EXA_API_KEY") or None
    mode = "EXA_API_KEY" if key else "keyless"

    if args.rest:
        log(f"{REST_SEARCH} · {mode}")
        try:
            print(rest_search(" ".join(args.words), args.n, key))
            return 0
        except (SearchError, OSError, json.JSONDecodeError) as e:
            print(e, file=sys.stderr)
            return 1
    log(f"{MCP_ENDPOINT} · {mode}")
    if args.list:
        payload = tools_list_body()
    elif args.fetch:
        payload = tool_call_body(
            1, "web_fetch_exa", {"urls": args.words, "maxCharacters": 3000}
        )
    else:
        payload = tool_call_body(
            1, "web_search_exa", search_arguments(" ".join(args.words), args.n, args.objective)
        )
    return run(MCP_ENDPOINT, payload, mcp_headers(key))


if __name__ == "__main__":
    sys.exit(main())
