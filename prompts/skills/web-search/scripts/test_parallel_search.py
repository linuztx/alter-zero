"""Tests for parallel_search.py — Parallel's own request shape. The
shared MCP decoding is tested in test_mcp_http.py. No network.

    python3 -m unittest discover -s prompts/skills/web-search/scripts -p 'test_*.py'
"""

import os
import sys
import unittest

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from parallel_search import (  # noqa: E402
    fetch_arguments,
    format_results,
    mcp_headers,
    search_arguments,
)


class Request(unittest.TestCase):
    def test_keyless_sends_no_extra_header(self):
        self.assertEqual(mcp_headers(None), {})

    def test_a_key_rides_as_a_bearer_token(self):
        self.assertEqual(mcp_headers("k"), {"Authorization": "Bearer k"})

    def test_the_objective_stands_in_for_missing_queries(self):
        self.assertEqual(
            search_arguments("ratatui inline viewport", [], "s"),
            {
                "objective": "ratatui inline viewport",
                "search_queries": ["ratatui inline viewport"],
                "session_id": "s",
            },
        )

    def test_given_queries_are_sent_as_is(self):
        args = search_arguments("o", ["a b c", "d e f"], "s")
        self.assertEqual(args["search_queries"], ["a b c", "d e f"])

    def test_a_fetch_sends_its_objective_only_when_given(self):
        self.assertEqual(
            fetch_arguments(["https://a"], None, "s"), {"urls": ["https://a"], "session_id": "s"}
        )
        self.assertEqual(fetch_arguments(["https://a"], "why", "s")["objective"], "why")


class Render(unittest.TestCase):
    # The live tool text's shape: a JSON document of results with excerpts.
    TEXT = (
        '{"search_id": "search_1", "results": ['
        '{"url": "https://a", "title": "A", "publish_date": null, "excerpts": ["one", "two"]},'
        '{"url": "https://b", "title": null, "publish_date": "2026-01-01", "excerpts": []}]}'
    )

    def test_results_render_as_title_url_blocks(self):
        self.assertEqual(
            format_results(self.TEXT),
            "Title: A\nURL: https://a\nPublished: N/A\n\none\n...\ntwo"
            "\n\n---\n\n"
            "Title: (untitled)\nURL: https://b\nPublished: 2026-01-01\n",
        )

    def test_text_that_is_not_a_result_document_passes_through(self):
        self.assertEqual(format_results("plain words"), "plain words")
        self.assertEqual(format_results('{"other": 1}'), '{"other": 1}')


if __name__ == "__main__":
    unittest.main()
