"""Tests for tavily_search.py — Tavily's own request shape and result
document. The shared MCP decoding is tested in test_mcp_http.py. No network.

    python3 -m unittest discover -s prompts/skills/web-search/scripts -p 'test_*.py'
"""

import os
import sys
import unittest

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from mcp_http import RateLimited, SearchError  # noqa: E402
from tavily_search import (  # noqa: E402
    extract_arguments,
    format_results,
    mcp_headers,
    search_arguments,
)


class Request(unittest.TestCase):
    def test_keyless_sends_the_keyless_access_header(self):
        self.assertEqual(mcp_headers(None), {"X-Tavily-Access-Mode": "keyless"})

    def test_a_key_rides_as_a_bearer_token_without_the_keyless_header(self):
        self.assertEqual(mcp_headers("tvly-k"), {"Authorization": "Bearer tvly-k"})

    def test_search_arguments(self):
        self.assertEqual(
            search_arguments("ratatui", 3, "basic"),
            {"query": "ratatui", "max_results": 3, "search_depth": "basic"},
        )

    def test_an_extract_sends_its_query_only_when_given(self):
        self.assertEqual(
            extract_arguments(["https://a"], None), {"urls": ["https://a"], "format": "markdown"}
        )
        self.assertEqual(extract_arguments(["https://a"], "why")["query"], "why")


class Render(unittest.TestCase):
    # The live tool text's shape: the REST response as a JSON string.
    SEARCH = (
        '{"query": "q", "answer": null, "images": [], "results": ['
        '{"url": "https://a", "title": "A", "content": "snippet", "score": 0.9, "raw_content": null},'
        '{"url": "https://b", "title": null, "content": "", "score": 0.5}],'
        '"auth_mode": "keyless", "keyless_notice": "Served on Tavily\'s free keyless tier."}'
    )

    def test_search_results_render_as_title_url_blocks(self):
        self.assertEqual(
            format_results(self.SEARCH),
            "Title: A\nURL: https://a\n\nsnippet\n\n---\n\nTitle: (untitled)\nURL: https://b\n",
        )

    def test_an_answer_leads_when_present(self):
        text = '{"answer": "Yes.", "results": [{"url": "https://a", "title": "A", "content": "c"}]}'
        self.assertTrue(format_results(text).startswith("Answer: Yes.\n\n---\n\n"))

    def test_an_extract_result_shows_its_raw_content(self):
        text = '{"results": [{"url": "https://a", "raw_content": "# Page"}], "failed_results": []}'
        self.assertEqual(format_results(text), "Title: (untitled)\nURL: https://a\n\n# Page")

    def test_failed_extract_urls_are_listed(self):
        text = '{"results": [], "failed_results": [{"url": "https://x", "error": "timeout"}]}'
        self.assertEqual(format_results(text), "Failed: https://x (timeout)")

    def test_an_error_document_in_a_200_result_is_an_error(self):
        # A bad key comes back as HTTP 200, `isError: false`, and this text.
        text = (
            '{"error": "Search failed", "status": 401,'
            ' "detail": {"error": "Unauthorized: missing or invalid API key."}}'
        )
        with self.assertRaises(SearchError) as caught:
            format_results(text)
        self.assertIn("Search failed (401)", str(caught.exception))
        self.assertIn("missing or invalid API key", str(caught.exception))

    def test_a_keyless_cap_document_is_rate_limited_with_its_countdown(self):
        # The live shape past the keyless quota: HTTP 200, `isError: false`.
        text = (
            '{"code": "monthly_cap_reached_bonus_eligible",'
            ' "message": "You reached the monthly keyless Tavily limit.",'
            ' "next_actions": [], "window": "month", "retry_after_seconds": 2640,'
            ' "auth_mode": "keyless"}'
        )
        with self.assertRaises(RateLimited) as caught:
            format_results(text)
        self.assertEqual(caught.exception.message, "You reached the monthly keyless Tavily limit.")
        self.assertEqual(caught.exception.retry_after, 2640)

    def test_any_other_code_document_is_an_error(self):
        with self.assertRaises(SearchError) as caught:
            format_results('{"code": "bad_request", "message": "nope"}')
        self.assertNotIsInstance(caught.exception, RateLimited)
        self.assertIn("bad_request: nope", str(caught.exception))

    def test_text_that_is_not_a_result_document_passes_through(self):
        self.assertEqual(format_results("rate limit reached, try later"), "rate limit reached, try later")


if __name__ == "__main__":
    unittest.main()
