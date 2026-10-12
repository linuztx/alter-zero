"""Tests for exa_search.py — Exa's own request shape and its REST
fallback. The shared MCP decoding is tested in test_mcp_http.py. No network.

    python3 -m unittest discover -s prompts/skills/web-search/scripts -p 'test_*.py'
"""

import os
import sys
import unittest

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from exa_search import (  # noqa: E402
    format_rest_results,
    mcp_headers,
    search_arguments,
    x402_price_usd,
)


class Request(unittest.TestCase):
    def test_keyless_headers_carry_only_the_attribution_header(self):
        self.assertEqual(mcp_headers(None), {"x-exa-source": "alter-zero"})

    def test_a_key_rides_as_a_bearer_token(self):
        self.assertEqual(mcp_headers("k")["Authorization"], "Bearer k")

    def test_the_objective_defaults_to_the_query(self):
        self.assertEqual(
            search_arguments("ratatui", 3),
            {"query": "ratatui", "objective": "ratatui", "numResults": 3},
        )
        self.assertEqual(search_arguments("q", 3, "docs first")["objective"], "docs first")


class Rest(unittest.TestCase):
    def test_an_x402_body_prices_at_its_cheapest_accept(self):
        body = {
            "accepts": [
                {"extra": {"totalUsd": 0.007}},
                {"extra": {"totalUsd": 0.006}},
                {"extra": {}},
            ]
        }
        self.assertEqual(x402_price_usd(body), 0.006)
        self.assertIsNone(x402_price_usd({"error": "x"}))

    def test_a_rest_result_prints_the_fields_it_reads(self):
        body = {
            "requestId": "r",
            "results": [
                {
                    "id": "x",
                    "url": "https://a",
                    "title": "A",
                    "publishedDate": "2026-01-01",
                    "author": None,
                    "text": " body ",
                }
            ],
        }
        self.assertEqual(
            format_rest_results(body),
            "Title: A\nURL: https://a\nPublished: 2026-01-01\nbody\n",
        )


if __name__ == "__main__":
    unittest.main()
