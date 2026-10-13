"""Tests for parallel_search.py — Parallel's own request shape. The
shared MCP decoding is tested in test_mcp_http.py. No network.

    python3 -m unittest discover -s prompts/skills/web-search/scripts -p 'test_*.py'
"""

import json
import os
import sys
import unittest
from unittest import mock

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from parallel_search import (  # noqa: E402
    fetch_arguments,
    format_results,
    main,
    mcp_headers,
    search_arguments,
)


def results_doc(count, excerpt="e"):
    """A result document of `count` results, each carrying `excerpt`."""
    return json.dumps(
        {
            "results": [
                {"url": f"https://r{i}", "title": f"R{i}", "excerpts": [excerpt]}
                for i in range(count)
            ]
        }
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

    def test_every_result_and_its_whole_excerpts_are_printed(self):
        # All the context Parallel returns reaches the agent: a search answers
        # with ten results, and a page read can be one 70,105-character
        # excerpt (python.org's downloads page). None of it is cut.
        out = format_results(results_doc(10, "x " * 35000))
        self.assertEqual(out.count("Title: "), 10)
        self.assertEqual(out.count("x " * 35000), 10)

    def test_urls_a_fetch_could_not_read_are_listed_last(self):
        # The live shape of a fetch error, beside a page that did come back.
        text = json.dumps(
            {
                "results": [{"url": "https://a", "title": "A", "excerpts": ["page"]}],
                "errors": [
                    {
                        "url": "https://example.invalid/",
                        "error_type": "connect_error",
                        "http_status_code": None,
                        "content": None,
                    },
                    {"url": "https://b/gone", "error_type": "fetch_error", "http_status_code": 404},
                ],
            }
        )
        self.assertTrue(
            format_results(text).endswith(
                "page\n\n"
                "Failed: https://example.invalid/ (connect_error)\n"
                "Failed: https://b/gone (fetch_error, HTTP 404)"
            )
        )

    def test_a_fetch_that_read_nothing_still_names_its_failures(self):
        text = json.dumps(
            {"results": [], "errors": [{"url": "https://x", "error_type": "connect_error"}]}
        )
        self.assertEqual(format_results(text), "Failed: https://x (connect_error)")


class Options(unittest.TestCase):
    def render_for(self, argv):
        """The renderer `main` hands to the request it would send."""
        with mock.patch("parallel_search.run", return_value=0) as run:
            self.assertEqual(main(argv), 0)
        return run.call_args.args[3]

    def test_a_search_hands_every_result_on_whole(self):
        render = self.render_for(["an objective"])
        out = render(results_doc(10, "x " * 35000))
        self.assertEqual(out.count("Title: "), 10)
        self.assertEqual(out.count("x " * 35000), 10)

    def test_json_prints_the_document_untouched(self):
        self.assertIsNone(self.render_for(["--json", "an objective"]))


if __name__ == "__main__":
    unittest.main()
