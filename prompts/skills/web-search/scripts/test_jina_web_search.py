"""Tests for jina_web_search.py — offline: no test here touches the network.

The fixtures are trimmed copies of real pages each engine served through
Jina Reader, kept to the markup the parsers read. One live check per engine
runs only with JINA_WEB_SEARCH_LIVE=1, to re-verify the engines themselves:

    python3 -m unittest discover -s prompts/skills/web-search/scripts -p 'test_*.py'
    JINA_WEB_SEARCH_LIVE=1 python3 -m unittest test_jina_web_search.LiveTest
"""

import datetime
import gzip
import io
import json
import os
import sys
import unittest
import urllib.error
import urllib.parse
from email.message import Message
from unittest import mock

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import jina_web_search as jws  # noqa: E402

NOW = datetime.datetime(2026, 10, 12, 12, 0, 0, tzinfo=datetime.timezone.utc)

DDG_PAGE_1 = """<html><head><title>sourdough starter feeding at DuckDuckGo</title></head><body>
<div class="serp__results"><div id="links" class="results">
<div class="result results_links results_links_deep web-result">
  <div class="links_main links_deep result__body">
    <h2 class="result__title">
      <a rel="nofollow" class="result__a" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fwww.kingarthurbaking.com%2Frecipes%2Ffeeding%2Dstarter&rut=40b3">Feeding and Maintaining Your <b>Sourdough</b> Starter - King Arthur Baking</a>
    </h2>
    <div class="result__extras"><div class="result__extras__url">
      <span class="result__icon"><a rel="nofollow" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fwww.kingarthurbaking.com%2Frecipes%2Ffeeding%2Dstarter&rut=40b3"><img class="result__icon__img" width="16" height="16" alt="" src="//external-content.duckduckgo.com/ip3/www.kingarthurbaking.com.ico" name="i15"></a></span>
      <a class="result__url" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fwww.kingarthurbaking.com%2Frecipes%2Ffeeding%2Dstarter&rut=40b3">www.kingarthurbaking.com/recipes/feeding-starter</a>
      <span>&#160; &#160; 2026-09-01T00:00:00.0000000</span>
    </div></div>
    <a class="result__snippet" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fwww.kingarthurbaking.com%2Frecipes%2Ffeeding%2Dstarter&rut=40b3">Learn how to keep your <b>sourdough</b> <b>starter</b> healthy with regular feedings.</a>
    <div class="clear"></div>
  </div>
</div>
<div class="result results_links results_links_deep result--ad">
  <div class="links_main links_deep result__body">
    <h2 class="result__title"><a rel="nofollow" class="result__a" href="https://duckduckgo.com/y.js?ad_domain=flour.example&u3=x">Buy Flour Now</a></h2>
    <a class="result__snippet" href="https://duckduckgo.com/y.js?ad_domain=flour.example">Sponsored.</a>
  </div>
</div>
<div class="result results_links results_links_deep web-result">
  <div class="links_main links_deep result__body">
    <h2 class="result__title">
      <a rel="nofollow" class="result__a" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fwww.theperfectloaf.com%2Ffeed%2Dstarter%2F&rut=9c1e">How Do I Feed My Sourdough Starter? - The Perfect Loaf</a>
    </h2>
    <div class="result__extras"><div class="result__extras__url">
      <a class="result__url" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fwww.theperfectloaf.com%2Ffeed%2Dstarter%2F&rut=9c1e">www.theperfectloaf.com/feed-starter/</a>
    </div></div>
    <a class="result__snippet" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fwww.theperfectloaf.com%2Ffeed%2Dstarter%2F&rut=9c1e">How do I feed my <b>sourdough</b> <b>starter</b>? Keep it strong.</a>
  </div>
</div>
<div class="nav-link">
  <form action="/html/" method="post">
    <input type="submit" class="btn btn--alt" value="Next">
    <input type="hidden" name="q" value="sourdough starter feeding">
    <input type="hidden" name="s" value="10">
    <input type="hidden" name="nextParams" value="">
    <input type="hidden" name="v" value="l">
    <input type="hidden" name="o" value="json">
    <input type="hidden" name="dc" value="11">
    <input type="hidden" name="api" value="d.js">
    <input type="hidden" name="vqd" value="4-2069615338">
    <input name="kl" value="wt-wt" type="hidden">
  </form>
</div>
</div></div></body></html>"""

DDG_PAGE_2 = """<html><body><div id="links" class="results">
<div class="result results_links results_links_deep web-result"><div class="result__body">
  <h2 class="result__title"><a class="result__a" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Ftheperfectloaf.com%2Ffeed-starter&rut=1">How Do I Feed My Sourdough Starter? - The Perfect Loaf</a></h2>
  <a class="result__snippet" href="#">The same page again, without www and the slash.</a>
</div></div>
<div class="result results_links results_links_deep web-result"><div class="result__body">
  <h2 class="result__title"><a class="result__a" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fnatashaskitchen.com%2Ffeed%2F&rut=2">How to Feed Sourdough Starter - NatashasKitchen.com</a></h2>
  <a class="result__snippet" href="#">Everything you need to know.</a>
</div></div>
<div class="nav-link"><form action="/html/" method="post">
  <input type="submit" class="btn btn--alt" value="Previous">
  <input type="hidden" name="q" value="sourdough starter feeding"><input type="hidden" name="s" value="0">
</form></div>
</div></body></html>"""

DDG_ANOMALY = """<html><body><div class="anomaly-modal__title">Unfortunately, bots use
DuckDuckGo too.</div><form id="challenge-form" action="/anomaly.js"></form></body></html>"""

YAHOO_PAGE = """<html><head><title>python asyncio - Yahoo Search Results</title></head><body>
<div id="web" class="web-res"><h2 class="off-left">Search Results</h2>
<ol class="reg searchCenterMiddle">
<li class="first"><div class="dd fst lst algo algo-sr relsrch richAlgo" data-yga="{}"><div class="compTitle options-toggle"><a class="d-ib va-top mt-38" data-matarget="algo" target="_blank" referrerpolicy="origin" href="https://rd.search.yahoo.com/_ylt=A2RR;_ylu=Y29sbw-/RV=2/RE=1792978552/RO=10/RU=https%3a%2f%2frealpython.com%2fasync-io-python%2f/RK=0/RS=CizX-"><div class="d-ib p-abs t-0 l-0 fz-13"><span class="d-ib va-mid"><span class="fc-141414 d-b">Real Python</span>https://realpython.com › async-io-python</span></div><h3 style="display:block" class="title fc-2015C2-imp pt-6"><span class="d-b fz-20 lh-24">Python's asyncio: A Hands-On Walkthrough – Real Python</span></h3></a></div><div class="compText aAbs"><p class="fc-dustygray fz-14"><span class="fc-smoke">Jul 30, 2025 · </span>  <b>Python</b>’s <b>asyncio</b> library enables you to write concurrent code.</p></div><div class="compList d-f alternative-links"><ul class="fl-l w-300"><li><a class="ff-yahoo-vf algo-sitelink-title" href="https://rd.search.yahoo.com/_ylt=B/RV=2/RE=1/RO=10/RU=https%3a%2f%2frealpython.com%2fcourses%2fasyncio%2f/RK=0/RS=x-">Hands-On <b>Python</b> 3 Concurrency</a></li></ul></div></div></li>
<li><div class="dd algo algo-sr"><div class="compTitle"><a href="https://r.search.yahoo.com/_ylt=C/RV=2/RE=1/RO=10/RU=https%3a%2f%2fdocs.python.org%2f3%2flibrary%2fasyncio.html/RK=2/RS=y-"><h3 class="title"><span>asyncio — Asynchronous I/O</span></h3></a></div><div class="compText aAbs"><p>asyncio is a library to write <b>concurrent</b> code.</p></div></div></li>
<li><div class="Video uds uds-dd"><h3>Videos</h3><a href="https://video.search.yahoo.com/search/video?p=python">More videos</a></div></li>
</ol>
<div class="compPagination"><a class="next" href="https://search.yahoo.com/search;_ylt=A2R?ei=UTF-8&fr=fp-tts&p=python+asyncio&b=8&pz=7&bct=0&xargs=0">Next</a></div>
</div></body></html>"""

YAHOO_TEMPORARY = """<html><body><div id="web" class="web-res"><ol><li><div class="dd">
<p>We had temporary problems searching for web pages. Search again for <b>python</b>
or type a new query above.</p></div></li></ol></div></body></html>"""

WIKIPEDIA_JSON = json.dumps(
    {
        "batchcomplete": True,
        "continue": {"sroffset": 10, "continue": "-||"},
        "query": {
            "searchinfo": {"totalhits": 1683},
            "search": [
                {
                    "ns": 0,
                    "title": "Asynchrony (computer programming)",
                    "pageid": 49031794,
                    "wordcount": 359,
                    "snippet": 'or a promise is completed. Some <span class="searchmatch">programming</span> languages',
                    "timestamp": "2026-08-30T10:11:12Z",
                },
                {
                    "ns": 0,
                    "title": "Async/await",
                    "pageid": 1,
                    "wordcount": 4200,
                    "snippet": "the async/await pattern &amp; more",
                    "timestamp": "2026-10-01T00:00:00Z",
                },
            ],
        },
    }
)

GITHUB_PAGE = (
    '<html><body><script type="application/json" data-target="react-partial.embeddedData">'
    '{"props":{}}</script>'
    '<script type="application/json" data-target="react-app.embeddedData">'
    + json.dumps(
        {
            "payload": {
                "blackbirdSearchRoute": {
                    "results": [
                        {
                            "archived": False,
                            "followers": 4606,
                            "hl_name": "CachyOS/<em>linux</em>-cachyos",
                            "hl_trunc_description": "Archlinux <em>Kernel</em> based on different <em>schedulers</em>.",
                            "language": "Shell",
                            "repo": {
                                "repository": {
                                    "name": "linux-cachyos",
                                    "owner_login": "CachyOS",
                                    "updated_at": "2026-10-08T14:48:20.599Z",
                                }
                            },
                        },
                        {
                            "archived": True,
                            "followers": 88,
                            "hl_name": "aliyun/<em>plugsched</em>",
                            "hl_trunc_description": None,
                            "language": None,
                            "repo": {"repository": {"name": "plugsched", "owner_login": "aliyun"}},
                        },
                    ],
                    "page": 1,
                    "page_count": 40,
                    "result_count": 395,
                }
            }
        }
    )
    + "</script></body></html>"
)

ARXIV_PAGE = """<html><body><h1 class="title is-clearfix">Showing 1&ndash;25 of 54 results for all: <span class="mathjax">kernel scheduler</span></h1>
<nav class="pagination is-small is-centered" role="navigation" aria-label="pagination">
  <a href="" class="pagination-previous is-invisible">Previous</a>
  <a href="/search/?query=kernel+scheduler&searchtype=all&start=25" class="pagination-next">Next</a>
</nav>
<ol class="breathe-horizontal" start="1">
<li class="arxiv-result">
  <div class="is-marginless">
    <p class="list-title is-inline-block"><a href="https://arxiv.org/abs/2609.14123">arXiv:2609.14123</a>
      <span>&nbsp;[<a href="https://arxiv.org/pdf/2609.14123">pdf</a>]&nbsp;</span></p>
    <div class="tags is-inline-block">
      <span class="tag is-small is-link tooltip is-tooltip-top" data-tooltip="Operating Systems">cs.OS</span>
      <span class="tag is-small is-grey tooltip is-tooltip-top" data-tooltip="Distributed">cs.DC</span>
    </div>
  </div>
  <p class="title is-5 mathjax">
    Grouper: <span class="search-hit mathjax">Scheduling</span> Groups for Microservices
  </p>
  <p class="authors">
    <span class="has-text-black-bis has-text-weight-semibold">Authors:</span>
    <a href="/search/?searchtype=author&query=Kazemi%2C+K">Koosha Kazemi</a>,
    <a href="/search/?searchtype=author&query=Siavashi%2C+M">Mohammad Siavashi</a>,
    <a href="/search/?searchtype=author&query=Siavashi%2C+A">Ahmad Siavashi</a>,
    <a href="/search/?searchtype=author&query=Izadi%2C+M">Mohammad Izadi</a>
  </p>
  <p class="abstract mathjax">
    <span class="search-hit">Abstract</span>:
    <span class="abstract-short has-text-grey-dark mathjax" style="display: inline;">…short version…
      <a class="is-size-7">&#9661; More</a></span>
    <span class="abstract-full has-text-grey-dark mathjax" style="display: none;">
      Microsecond-scale core allocation makes colocating services worthwhile.
      <a class="is-size-7">&#9651; Less</a></span>
  </p>
  <p class="is-size-7"><span class="has-text-black-bis has-text-weight-semibold">Submitted</span> 12 September, 2026;
    <span class="has-text-black-bis has-text-weight-semibold">originally announced</span> September 2026.</p>
</li>
<li class="arxiv-result">
  <div class="is-marginless"><p class="list-title is-inline-block"><a href="https://arxiv.org/abs/2609.12276">arXiv:2609.12276</a></p></div>
  <p class="title is-5 mathjax">AKTS: Sub-Microsecond Kernel Policy Switching</p>
  <p class="authors"><span>Authors:</span> <a href="#">Mahdi Alizadeh</a></p>
  <p class="abstract mathjax"><span class="abstract-short">GPU-backed LLM servers multiplex requests.</span></p>
  <p class="is-size-7"><span>Submitted</span> 7 October, 2026; <span>originally announced</span> October 2026.</p>
</li>
</ol></body></html>"""

ARXIV_EMPTY = """<html><body><p class="is-size-4 has-text-warning">Sorry, your query for all:
qzx produced no results.</p></body></html>"""

HN_JSON = json.dumps(
    {
        "hits": [
            {
                "objectID": "50021127",
                "title": "Python 3.15",
                "url": "https://www.python.org/downloads/release/python-3150/",
                "author": "ngoldbaum",
                "points": 314,
                "num_comments": 109,
                "created_at": "2026-10-09T14:35:42Z",
                "_highlightResult": {"title": {"value": "<em>Python</em> 3.15"}},
            },
            {
                "objectID": "49900001",
                "title": "Ask HN: Which <async> runtime?",
                "url": None,
                "author": "someone",
                "points": 12,
                "num_comments": 3,
                "created_at": "2026-10-01T08:00:00Z",
                "story_text": "<p>Tokio or smol?</p><p>Asking for a friend &amp; colleague.</p>",
            },
        ],
        "page": 0,
        "nbPages": 3,
        "hitsPerPage": 10,
    }
)


class FakeFetcher:
    """Answers each target URL from a table; records every call."""

    def __init__(self, pages):
        self.pages = pages
        self.calls = []

    def __call__(self, url, *, respond_with, timeout, api_key=None, no_cache=False):
        self.calls.append(
            {
                "url": url,
                "respond_with": respond_with,
                "timeout": timeout,
                "api_key": api_key,
                "no_cache": no_cache,
            }
        )
        for prefix, answer in self.pages:
            if url.startswith(prefix):
                if isinstance(answer, list):
                    answer = answer.pop(0) if len(answer) > 1 else answer[0]
                if isinstance(answer, Exception):
                    raise answer
                return answer
        raise AssertionError(f"unexpected fetch: {url}")


def run_main(argv, fetcher, env=None):
    out, err = io.StringIO(), io.StringIO()
    sleeps = []
    code = jws.main(
        argv, env=env or {}, fetcher=fetcher, stdout=out, stderr=err, sleep=sleeps.append
    )
    return code, out.getvalue(), err.getvalue(), sleeps


def ddg(url_query="sourdough"):
    return "https://html.duckduckgo.com/html/?q=" + url_query


# ===== the request =====


class JinaUrlTest(unittest.TestCase):
    def test_the_target_survives_jinas_single_decode(self):
        # Jina percent-decodes the path once before fetching it: what reaches
        # the engine must be exactly the target, `+`, `%26` and `%23` intact.
        for query in ["c++ templates", "rock & roll", "#hashtag", "café 100%", "a=b?c/d"]:
            target = jws.DuckDuckGo().first_url(query)
            wrapped = jws.jina_url(target)
            self.assertTrue(
                wrapped.startswith("https://r.jina.ai/https://html.duckduckgo.com/html/%3Fq%3D")
            )
            self.assertEqual(urllib.parse.unquote(wrapped[len(jws.JINA_READER) :]), target)
            query_string = urllib.parse.urlsplit(target).query
            self.assertEqual(urllib.parse.parse_qs(query_string)["q"], [query])

    def test_the_wrapped_url_quotes_the_whole_target_into_the_path(self):
        self.assertEqual(
            jws.jina_url("https://html.duckduckgo.com/html/?q=c%2B%2B+x&s=10"),
            "https://r.jina.ai/https://html.duckduckgo.com/html/%3Fq%3Dc%252B%252B%2Bx%26s%3D10",
        )

    def test_no_target_parameter_can_reach_jina_as_its_own_option(self):
        # Jina reads its request's query string as its own options: Hacker
        # News's `page=0` came back as Jina's 400 "page must be >= 1".
        for engine in jws.ENGINES.values():
            wrapped = jws.jina_url(engine.first_url("rust async", None))
            self.assertEqual(urllib.parse.urlsplit(wrapped).query, "", engine.name)


class HeadersTest(unittest.TestCase):
    def test_a_key_is_sent_as_a_bearer_token(self):
        headers = jws.request_headers("html", api_key="jina_secret")
        self.assertEqual(headers["Authorization"], "Bearer jina_secret")

    def test_no_key_sends_no_authorization_at_all(self):
        for key in (None, ""):
            self.assertNotIn("Authorization", jws.request_headers("html", api_key=key))

    def test_format_and_cache_headers(self):
        headers = jws.request_headers("text", no_cache=True)
        self.assertEqual(headers["X-Respond-With"], "text")
        self.assertEqual(headers["X-No-Cache"], "true")
        self.assertNotIn("X-No-Cache", jws.request_headers("html"))

    def test_no_fetch_engine_is_forced(self):
        # `X-Engine: curl` made Jina run a JSON response through its HTML
        # text extraction: `&quot;` inside a Wikipedia snippet became a bare
        # `"` and the JSON no longer parsed.
        self.assertNotIn("X-Engine", jws.request_headers("text"))
        self.assertNotIn("X-Engine", jws.request_headers("html"))


class FakeResponse(io.BytesIO):
    def __init__(self, body, headers=None):
        super().__init__(body)
        self.headers = Message()
        for name, value in (headers or {}).items():
            self.headers[name] = value

    def __enter__(self):
        return self

    def __exit__(self, *exc):
        return False


class FetchTest(unittest.TestCase):
    def fetch(self, **kwargs):
        options = {"respond_with": "html", "timeout": 5, "api_key": None, "no_cache": False}
        options.update(kwargs)
        return jws.fetch("https://example.com/?q=a+b", **options)

    def test_sends_the_wrapped_url_and_headers(self):
        seen = {}

        def urlopen(request, timeout):
            seen["url"], seen["headers"], seen["timeout"] = (
                request.full_url,
                dict(request.header_items()),
                timeout,
            )
            return FakeResponse(b"<html>ok</html>", {"Content-Type": "text/html; charset=utf-8"})

        with mock.patch("urllib.request.urlopen", urlopen):
            body = self.fetch(api_key="k1", timeout=7)
        self.assertEqual(body, "<html>ok</html>")
        self.assertEqual(seen["url"], "https://r.jina.ai/https://example.com/%3Fq%3Da%2Bb")
        self.assertEqual(seen["headers"]["Authorization"], "Bearer k1")
        self.assertEqual(seen["timeout"], 7)

    def test_gzip_bodies_are_inflated(self):
        raw = gzip.compress("héllo".encode())
        response = FakeResponse(
            raw, {"Content-Encoding": "gzip", "Content-Type": "text/plain; charset=utf-8"}
        )
        with mock.patch("urllib.request.urlopen", return_value=response):
            self.assertEqual(self.fetch(), "héllo")

    def http_error(self, status, body, headers=None):
        message = Message()
        for name, value in (headers or {}).items():
            message[name] = value
        return urllib.error.HTTPError(
            "https://r.jina.ai/x", status, "err", message, io.BytesIO(body)
        )

    def test_a_429_carries_its_retry_after(self):
        error = self.http_error(429, b'{"code":429,"message":"Too many"}', {"Retry-After": "5"})
        with mock.patch("urllib.request.urlopen", side_effect=error):
            with self.assertRaises(jws.JinaHTTPError) as caught:
                self.fetch()
        self.assertEqual(caught.exception.status, 429)
        self.assertEqual(caught.exception.retry_after, 5.0)
        self.assertIn("JINA_API_KEY", str(caught.exception))

    def test_jinas_error_envelope_is_read(self):
        body = json.dumps(
            {"code": 401, "status": 40103, "message": "blocked: bad IP reputation"}
        ).encode()
        with mock.patch("urllib.request.urlopen", side_effect=self.http_error(401, body)):
            with self.assertRaises(jws.JinaHTTPError) as caught:
                self.fetch()
        self.assertEqual(caught.exception.code, 40103)
        self.assertIn("bad IP reputation", str(caught.exception))
        self.assertEqual(caught.exception.retry_wait(), 2.0)

    def test_a_rejected_key_is_named_and_not_retried(self):
        body = b'{"code":401,"status":40101,"message":"invalid token"}'
        with mock.patch("urllib.request.urlopen", side_effect=self.http_error(401, body)):
            with self.assertRaises(jws.JinaHTTPError) as caught:
                self.fetch(api_key="bad")
        self.assertIn("rejected JINA_API_KEY", str(caught.exception))
        self.assertIsNone(caught.exception.retry_wait())

    def test_network_failures_are_transient(self):
        import socket

        for error in (
            urllib.error.URLError(socket.timeout("timed out")),
            socket.timeout("x"),
            ConnectionResetError("reset"),
        ):
            with mock.patch("urllib.request.urlopen", side_effect=error):
                with self.assertRaises(jws.TransientError):
                    self.fetch()


class RetryWaitTest(unittest.TestCase):
    def test_policy(self):
        self.assertEqual(jws.JinaHTTPError(429).retry_wait(), 3.0)
        self.assertEqual(jws.JinaHTTPError(429, retry_after=4).retry_wait(), 4)
        self.assertIsNone(jws.JinaHTTPError(429, retry_after=60).retry_wait())
        self.assertIsNone(jws.JinaHTTPError(401, code=40103, keyed=True).retry_wait())
        self.assertIsNone(jws.JinaHTTPError(403).retry_wait())
        self.assertEqual(jws.JinaHTTPError(503).retry_wait(), 1.0)
        self.assertEqual(jws.JinaHTTPError(422).retry_wait(), 1.0)

    def test_retry_after_parsing(self):
        self.assertEqual(jws.parse_retry_after("7"), 7.0)
        self.assertIsNone(jws.parse_retry_after("Wed, 21 Oct 2026 07:28:00 GMT"))
        self.assertIsNone(jws.parse_retry_after(None))


# ===== the parsers =====


class DuckDuckGoTest(unittest.TestCase):
    def setUp(self):
        self.page = jws.DuckDuckGo().parse(DDG_PAGE_1, ddg())

    def test_results_skip_ads_and_unwrap_redirects(self):
        self.assertEqual(
            [r.url for r in self.page.results],
            [
                "https://www.kingarthurbaking.com/recipes/feeding-starter",
                "https://www.theperfectloaf.com/feed-starter/",
            ],
        )

    def test_an_ad_is_skipped_by_its_class_whatever_its_link(self):
        ad = DDG_PAGE_1.replace(
            "https://duckduckgo.com/y.js?ad_domain=flour.example&u3=x",
            "//duckduckgo.com/l/?uddg=https%3A%2F%2Fflour.example%2F&rut=ad",
        )
        urls = [r.url for r in jws.DuckDuckGo().parse(ad, ddg()).results]
        self.assertNotIn("https://flour.example/", urls)

    def test_title_date_and_snippet(self):
        first = self.page.results[0]
        self.assertEqual(
            first.title, "Feeding and Maintaining Your Sourdough Starter - King Arthur Baking"
        )
        self.assertEqual(first.date, "2026-09-01T00:00:00.0000000")
        self.assertEqual(
            first.snippet, "Learn how to keep your sourdough starter healthy with regular feedings."
        )
        self.assertEqual(self.page.results[1].date, "", "no date span, no date")

    def test_the_next_form_becomes_a_get(self):
        params = urllib.parse.parse_qs(
            urllib.parse.urlsplit(self.page.next_url).query, keep_blank_values=True
        )
        self.assertTrue(self.page.next_url.startswith(jws.DuckDuckGo.BASE + "?"))
        self.assertEqual(params["s"], ["10"])
        self.assertEqual(params["vqd"], ["4-2069615338"])
        self.assertEqual(params["q"], ["sourdough starter feeding"])
        self.assertEqual(params["nextParams"], [""])

    def test_only_a_previous_form_means_no_next_page(self):
        self.assertIsNone(jws.DuckDuckGo().parse(DDG_PAGE_2, ddg()).next_url)

    def test_a_bot_check_is_a_soft_block(self):
        with self.assertRaises(jws.SoftBlock):
            jws.DuckDuckGo().parse(DDG_ANOMALY, ddg())

    def test_an_unknown_page_is_an_error_not_an_empty_result(self):
        with self.assertRaises(jws.SearchError):
            jws.DuckDuckGo().parse("<html><body>Something else</body></html>", ddg())

    def test_no_results_page(self):
        page = jws.DuckDuckGo().parse('<div class="no-results">No results.</div>', ddg())
        self.assertEqual(page.results, [])

    def test_first_url_and_time_filter(self):
        self.assertEqual(
            jws.DuckDuckGo().first_url("rust tui"), "https://html.duckduckgo.com/html/?q=rust+tui"
        )
        self.assertEqual(
            jws.DuckDuckGo().first_url("x", "week"), "https://html.duckduckgo.com/html/?q=x&df=w"
        )


class YahooTest(unittest.TestCase):
    def setUp(self):
        self.page = jws.Yahoo().parse(YAHOO_PAGE, "https://search.yahoo.com/search?p=python")

    def test_organic_results_only(self):
        # The sitelink under the first result and the video block are not
        # results of their own.
        self.assertEqual(
            [r.url for r in self.page.results],
            [
                "https://realpython.com/async-io-python/",
                "https://docs.python.org/3/library/asyncio.html",
            ],
        )

    def test_the_date_moves_out_of_the_snippet(self):
        first = self.page.results[0]
        self.assertEqual(first.title, "Python's asyncio: A Hands-On Walkthrough – Real Python")
        self.assertEqual(first.date, "Jul 30, 2025")
        self.assertEqual(
            first.snippet, "Python’s asyncio library enables you to write concurrent code."
        )
        self.assertEqual(self.page.results[1].date, "")

    def test_next_link(self):
        self.assertIn("b=8", self.page.next_url)
        self.assertIn("fr=fp-tts", self.page.next_url)

    def test_temporary_problems_is_a_soft_block(self):
        with self.assertRaises(jws.SoftBlock):
            jws.Yahoo().parse(YAHOO_TEMPORARY, "u")

    def test_first_url_keeps_fr_and_maps_time(self):
        url = jws.Yahoo().first_url("python asyncio", "day")
        self.assertEqual(
            url, "https://search.yahoo.com/search?p=python+asyncio&ei=UTF-8&fr=fp-tts&btf=d"
        )


class WikipediaTest(unittest.TestCase):
    def test_hits_become_article_links(self):
        url = jws.Wikipedia().first_url("async programming")
        page = jws.Wikipedia().parse(WIKIPEDIA_JSON, url)
        first, second = page.results
        self.assertEqual(
            first.url, "https://en.wikipedia.org/wiki/Asynchrony_(computer_programming)"
        )
        self.assertEqual(first.snippet, "or a promise is completed. Some programming languages")
        self.assertEqual(first.date, "2026-08-30T10:11:12Z")
        self.assertEqual(first.info, "359 words")
        self.assertEqual(second.url, "https://en.wikipedia.org/wiki/Async/await")
        self.assertEqual(second.snippet, "the async/await pattern & more")
        self.assertIn("sroffset=10", page.next_url)
        self.assertIn("srsearch=async+programming", page.next_url)

    def test_jinas_title_header_is_tolerated(self):
        body = "Title: \n\nURL Source: x\n\nMarkdown Content:\n" + WIKIPEDIA_JSON
        self.assertEqual(len(jws.Wikipedia().parse(body, "https://x/?a=1").results), 2)

    def test_an_api_error(self):
        with self.assertRaises(jws.SearchError):
            jws.Wikipedia().parse('{"error": {"code": "x", "info": "bad"}}', "u")

    def test_the_last_page(self):
        body = json.dumps({"query": {"search": []}})
        self.assertIsNone(jws.Wikipedia().parse(body, "u").next_url)


class GitHubTest(unittest.TestCase):
    def test_repositories(self):
        url = jws.GitHub().first_url("kernel scheduler")
        page = jws.GitHub().parse(GITHUB_PAGE, url)
        first, second = page.results
        self.assertEqual(first.title, "CachyOS/linux-cachyos")
        self.assertEqual(first.url, "https://github.com/CachyOS/linux-cachyos")
        self.assertEqual(first.date, "2026-10-08T14:48:20.599Z")
        self.assertEqual(first.snippet, "Archlinux Kernel based on different schedulers.")
        self.assertEqual(first.info, "Shell · ★ 4,606")
        self.assertEqual(second.info, "★ 88 · archived")
        self.assertEqual(second.snippet, "")
        self.assertEqual(
            page.next_url, "https://github.com/search?q=kernel+scheduler&type=repositories&p=2"
        )

    def test_time_filter_adds_a_pushed_qualifier(self):
        url = jws.GitHub().first_url("tui", "week", now=NOW)
        self.assertEqual(
            urllib.parse.parse_qs(urllib.parse.urlsplit(url).query)["q"], ["tui pushed:>2026-10-05"]
        )

    def test_a_page_without_data(self):
        with self.assertRaises(jws.SoftBlock):
            jws.GitHub().parse("<html>You have exceeded a secondary rate limit.</html>", "u")
        with self.assertRaises(jws.SearchError):
            jws.GitHub().parse("<html>nope</html>", "u")


class ArXivTest(unittest.TestCase):
    def test_papers(self):
        url = jws.ArXiv().first_url("kernel scheduler")
        page = jws.ArXiv().parse(ARXIV_PAGE, url)
        first, second = page.results
        self.assertEqual(first.title, "Grouper: Scheduling Groups for Microservices")
        self.assertEqual(first.url, "https://arxiv.org/abs/2609.14123")
        self.assertEqual(first.date, "12 September, 2026")
        self.assertEqual(
            first.info, "Koosha Kazemi, Mohammad Siavashi, Ahmad Siavashi et al. · cs.OS, cs.DC"
        )
        self.assertEqual(
            first.snippet, "Microsecond-scale core allocation makes colocating services worthwhile."
        )
        self.assertEqual(second.snippet, "GPU-backed LLM servers multiplex requests.")
        self.assertEqual(second.info, "Mahdi Alizadeh")
        query = urllib.parse.parse_qs(
            urllib.parse.urlsplit(page.next_url).query, keep_blank_values=True
        )
        self.assertEqual(query["start"], ["25"])
        self.assertEqual(query["size"], ["25"])

    def test_no_results(self):
        self.assertEqual(jws.ArXiv().parse(ARXIV_EMPTY, "u").results, [])

    def test_an_unknown_page(self):
        with self.assertRaises(jws.SearchError):
            jws.ArXiv().parse("<html>maintenance</html>", "u")


class UnknownPageTest(unittest.TestCase):
    def test_every_html_engine_names_a_changed_page_the_same_way(self):
        # SKILL.md tells the agent that "does not recognise" means the engine
        # changed its page and to try another one, so every engine says it.
        for name, engine in jws.ENGINES.items():
            if engine.respond_with != "html":
                continue
            with self.subTest(engine=name):
                with self.assertRaisesRegex(jws.SearchError, "does not recognise"):
                    engine.parse("<html><body>Scheduled maintenance</body></html>", "u")


class HackerNewsTest(unittest.TestCase):
    def test_stories(self):
        url = jws.HackerNews().first_url("python")
        page = jws.HackerNews().parse(HN_JSON, url)
        story, ask = page.results
        self.assertEqual(story.url, "https://www.python.org/downloads/release/python-3150/")
        self.assertEqual(
            story.info,
            "314 points · 109 comments · by ngoldbaum · https://news.ycombinator.com/item?id=50021127",
        )
        self.assertEqual(story.date, "2026-10-09T14:35:42Z")
        self.assertEqual(ask.url, "https://news.ycombinator.com/item?id=49900001")
        self.assertEqual(ask.title, "Ask HN: Which <async> runtime?")
        self.assertEqual(ask.snippet, "Tokio or smol? Asking for a friend & colleague.")
        self.assertEqual(ask.info, "12 points · 3 comments · by someone")
        self.assertIn("page=1", page.next_url)

    def test_time_filter(self):
        url = jws.HackerNews().first_url("python", "day", now=NOW)
        expected = int((NOW - datetime.timedelta(days=1)).timestamp())
        self.assertIn(f"created_at_i>{expected}", urllib.parse.unquote(url))


class HtmlTreeTest(unittest.TestCase):
    def test_stray_end_tags_and_void_elements(self):
        doc = jws.parse_html("<div class='a b'><p>x<br>y</span></p><img src=z></div><div>t</div>")
        self.assertEqual(doc.find(cls="b").text(), "x y")
        self.assertEqual(len(doc.find_all("div")), 2)

    def test_script_text_is_not_page_text(self):
        doc = jws.parse_html("<div>a<script>var x = '<b>no</b>';</script>b</div>")
        self.assertEqual(doc.find("div").text(), "ab")

    def test_shorten_cuts_at_a_word(self):
        text = "word " * 200
        cut = jws.shorten(text, 50)
        self.assertLessEqual(len(cut), 50)
        self.assertTrue(cut.endswith("…"))
        self.assertTrue(cut[:-1].endswith("word"))


# ===== the search =====


class FetchPageRetryTest(unittest.TestCase):
    def run_page(self, answers, engine=None, api_key=None):
        fetcher = FakeFetcher([("https://", answers)])
        sleeps = []
        options = jws.SearchOptions(api_key=api_key)
        result = jws.fetch_page(engine or jws.DuckDuckGo(), ddg(), options, fetcher, sleeps.append)
        return result, fetcher.calls, sleeps

    def test_a_soft_block_is_retried_once_past_the_cache(self):
        page, calls, sleeps = self.run_page([DDG_ANOMALY, DDG_PAGE_1])
        self.assertEqual(len(page.results), 2)
        self.assertEqual([call["no_cache"] for call in calls], [False, True])
        self.assertEqual(sleeps, [1.0])

    def test_two_soft_blocks_give_up_with_advice(self):
        with self.assertRaises(jws.SearchError) as caught:
            self.run_page([DDG_ANOMALY, DDG_ANOMALY])
        self.assertIn("another engine", str(caught.exception))

    def test_a_rate_limit_waits_for_retry_after(self):
        page, calls, sleeps = self.run_page([jws.JinaHTTPError(429, retry_after=2), DDG_PAGE_1])
        self.assertEqual(len(calls), 2)
        self.assertEqual(sleeps, [2])

    def test_a_long_retry_after_is_reported_not_waited(self):
        with self.assertRaises(jws.JinaHTTPError):
            self.run_page([jws.JinaHTTPError(429, retry_after=90), DDG_PAGE_1])

    def test_a_refused_anonymous_request_is_retried_once(self):
        page, calls, sleeps = self.run_page([jws.JinaHTTPError(401, code=40103), DDG_PAGE_1])
        self.assertEqual((len(calls), sleeps), (2, [2.0]))

    def test_transient_errors_are_retried_once(self):
        with self.assertRaises(jws.TransientError):
            self.run_page([jws.TransientError("reset"), jws.TransientError("reset")])
        page, calls, _ = self.run_page([jws.TransientError("reset"), DDG_PAGE_1])
        self.assertEqual(len(calls), 2)


class SearchEngineTest(unittest.TestCase):
    def search(self, table, engine=None, **options):
        fetcher = FakeFetcher(table)
        report = jws.search_engine(
            engine or jws.DuckDuckGo(),
            "sourdough starter feeding",
            jws.SearchOptions(**options),
            fetcher,
            lambda s: None,
        )
        return report, fetcher.calls

    def test_pages_follow_the_next_form_and_deduplicate(self):
        report, calls = self.search(
            [(ddg("sourdough+starter+feeding&s=10"), DDG_PAGE_2), (ddg("sourdough"), DDG_PAGE_1)],
            pages=3,
        )
        self.assertEqual(report.pages, 2, "page 2 has no next form, so paging stops")
        self.assertEqual(len(calls), 2)
        self.assertEqual(
            [r.title for r in report.results],
            [
                "Feeding and Maintaining Your Sourdough Starter - King Arthur Baking",
                "How Do I Feed My Sourdough Starter? - The Perfect Loaf",
                "How to Feed Sourdough Starter - NatashasKitchen.com",
            ],
        )

    def test_one_page_by_default(self):
        report, calls = self.search([(ddg(), DDG_PAGE_1)])
        self.assertEqual((report.pages, len(calls)), (1, 1))

    def test_the_limit_stops_paging_early(self):
        report, calls = self.search([(ddg(), DDG_PAGE_1)], pages=5, limit=2)
        self.assertEqual(len(report.results), 2)
        self.assertEqual(len(calls), 1, "two results came from page 1; page 2 is never fetched")
        report, _ = self.search([(ddg(), DDG_PAGE_1)], limit=1)
        self.assertEqual(len(report.results), 1)

    def test_a_failure_after_page_one_keeps_the_results(self):
        report, _ = self.search(
            [
                (ddg("sourdough+starter+feeding&s=10"), jws.JinaHTTPError(403)),
                (ddg("sourdough"), DDG_PAGE_1),
            ],
            pages=2,
        )
        self.assertIsNone(report.error)
        self.assertEqual(len(report.results), 2)
        self.assertTrue(report.warnings[0].startswith("stopped after page 1:"))

    def test_a_failure_on_page_one_is_the_engines_error(self):
        report, _ = self.search([(ddg(), jws.JinaHTTPError(403, "nope"))])
        self.assertIn("HTTP 403: nope", report.error)
        self.assertEqual(report.results, [])

    def test_a_single_page_engine_ignores_pages_and_says_so(self):
        class OnePage(jws.DuckDuckGo):
            name = "onepage"
            paged = False

        report, calls = self.search([(ddg(), DDG_PAGE_1)], engine=OnePage(), pages=3)
        self.assertEqual(len(calls), 1)
        self.assertIn("single page", report.warnings[0])

    def test_unsupported_time_filter_is_dropped_with_a_warning(self):
        report, calls = self.search(
            [("https://en.wikipedia.org/", WIKIPEDIA_JSON)], engine=jws.Wikipedia(), time="week"
        )
        self.assertIn("--time is not supported", report.warnings[0])
        self.assertEqual(calls[0]["respond_with"], "text")

    def test_a_parser_crash_fails_that_engine_only(self):
        # Markup an engine changed overnight must cost that engine's results,
        # not the whole run: the other engines' threads still report.
        class Broken(jws.DuckDuckGo):
            name = "broken"

            def parse(self, body, url):
                raise KeyError("result__a")

        report, _ = self.search([(ddg(), DDG_PAGE_1)], engine=Broken())
        self.assertIn("could not read", report.error)
        self.assertIn("KeyError", report.error)

    def test_the_key_never_appears_in_an_error(self):
        report, _ = self.search(
            [(ddg(), jws.JinaHTTPError(400, "bad token sk-123"))], api_key="sk-123"
        )
        self.assertNotIn("sk-123", report.error)


# ===== output and the command line =====


class OutputTest(unittest.TestCase):
    def test_the_documented_text_format(self):
        result = jws.Result(
            "duckduckgo",
            "Feeding and Maintaining Your Sourdough Starter - King Arthur Baking",
            "https://www.kingarthurbaking.com/recipes/feeding-starter",
            date="2026-09-01T00:00:00.0000000",
            snippet="Learn how to keep your sourdough starter healthy with regular feedings.",
        )
        self.assertEqual(
            jws.format_result(result),
            "Feeding and Maintaining Your Sourdough Starter - King Arthur Baking\n"
            "https://www.kingarthurbaking.com/recipes/feeding-starter\n"
            "Date: 2026-09-01T00:00:00.0000000\n"
            "Learn how to keep your sourdough starter healthy with regular feedings.",
        )

    def test_info_leads_the_snippet_and_missing_lines_are_dropped(self):
        result = jws.Result("x", "T", "https://t", info="Channel · 1:00", snippet="S")
        self.assertEqual(jws.format_result(result), "T\nhttps://t\nChannel · 1:00 — S")
        self.assertEqual(jws.format_result(jws.Result("x", "T", "https://t")), "T\nhttps://t")


class MainTest(unittest.TestCase):
    def fetcher(self):
        return FakeFetcher(
            [
                ("https://html.duckduckgo.com/", DDG_PAGE_1),
                ("https://hn.algolia.com/", HN_JSON),
                ("https://en.wikipedia.org/", jws.JinaHTTPError(429, retry_after=120)),
            ]
        )

    def test_a_single_engine_prints_bare_results(self):
        code, out, err, _ = run_main(["sourdough", "starter"], self.fetcher())
        self.assertEqual(code, 0)
        self.assertTrue(
            out.startswith("Feeding and Maintaining Your Sourdough Starter - King Arthur Baking\n")
        )
        self.assertIn("\nDate: 2026-09-01T00:00:00.0000000\n", out)
        self.assertNotIn("##", out)
        self.assertEqual(err, "")

    def test_several_engines_are_grouped_and_failures_go_to_stderr(self):
        code, out, err, _ = run_main(
            ["--engine", "ddg,hn", "--engine", "wiki", "rust"], self.fetcher()
        )
        self.assertEqual(code, 0)
        self.assertIn("## duckduckgo (2 results)", out)
        self.assertIn("## hackernews (2 results)", out)
        self.assertNotIn("wikipedia", out)
        self.assertIn("jina_web_search.py: wikipedia: Jina Reader's rate limit was reached", err)
        self.assertLess(out.index("## duckduckgo"), out.index("## hackernews"))

    def test_json_output(self):
        code, out, err, _ = run_main(
            ["--json", "--engine", "ddg,wiki", "--limit", "1", "rust"], self.fetcher()
        )
        data = json.loads(out)
        self.assertEqual(code, 0)
        self.assertEqual(data["query"], "rust")
        self.assertEqual(len(data["results"]), 1)
        first = data["results"][0]
        self.assertEqual(set(first), {"engine", "title", "url", "date", "snippet", "info"})
        self.assertEqual(first["engine"], "duckduckgo")
        self.assertIsNone(first["info"])
        engines = {entry["engine"]: entry for entry in data["engines"]}
        self.assertIsNone(engines["duckduckgo"]["error"])
        self.assertIn("rate limit", engines["wikipedia"]["error"])
        self.assertEqual(err, "", "JSON mode reports failures in the document")

    def test_every_engine_failing_exits_1(self):
        code, out, err, _ = run_main(["--engine", "wiki", "rust"], self.fetcher())
        self.assertEqual((code, out), (1, ""))
        self.assertIn("rate limit", err)

    def test_zero_results(self):
        fetcher = FakeFetcher([("https://", '<div class="no-results">No results.</div>')])
        code, out, _, _ = run_main(["zzz"], fetcher)
        self.assertEqual((code, out), (0, "No results.\n"))

    def test_options_reach_the_fetcher(self):
        fetcher = FakeFetcher([("https://", DDG_PAGE_1)])
        run_main(
            ["--timeout", "12", "--no-cache", "--time", "month", "x"],
            fetcher,
            env={"JINA_API_KEY": " k \n"},
        )
        call = fetcher.calls[0]
        self.assertEqual((call["timeout"], call["no_cache"], call["api_key"]), (12.0, True, "k"))
        self.assertIn("df=m", call["url"])

    def test_an_empty_key_is_no_key(self):
        fetcher = FakeFetcher([("https://", DDG_PAGE_1)])
        run_main(["x"], fetcher, env={"JINA_API_KEY": "  "})
        self.assertIsNone(fetcher.calls[0]["api_key"])

    def test_all_searches_every_engine(self):
        self.assertEqual(
            [engine.name for engine in jws.resolve_engines(["all"])],
            ["duckduckgo", "yahoo", "wikipedia", "github", "arxiv", "hackernews"],
        )
        self.assertEqual([e.name for e in jws.resolve_engines(None)], ["duckduckgo"])
        self.assertEqual(
            [e.name for e in jws.resolve_engines(["hn, gh", "HN"])], ["hackernews", "github"]
        )

    def test_usage_errors_exit_2(self):
        for argv in (
            [],
            ["  "],
            ["--engine", "google", "x"],
            # YouTube is not searched: neither its name nor its old alias is an engine.
            ["--engine", "youtube", "x"],
            ["--engine", "yt", "x"],
            ["--pages", "11", "x"],
            ["--pages", "0", "x"],
            ["--limit", "0", "x"],
            ["--timeout", "0", "x"],
            ["--time", "decade", "x"],
        ):
            with self.subTest(argv=argv), mock.patch("sys.stderr", io.StringIO()):
                with self.assertRaises(SystemExit) as caught:
                    run_main(argv, FakeFetcher([]))
                self.assertEqual(caught.exception.code, 2)

    def test_help_lists_every_engine(self):
        text = jws.build_parser().format_help()
        for name in jws.ENGINES:
            self.assertIn(name, text)
        self.assertIn("JINA_API_KEY", text)


@unittest.skipUnless(
    os.environ.get("JINA_WEB_SEARCH_LIVE") == "1", "set JINA_WEB_SEARCH_LIVE=1 to query the engines"
)
class LiveTest(unittest.TestCase):
    """One real search per engine, a few seconds apart (Jina allows 20 a minute)."""

    QUERIES = {
        "duckduckgo": "rust async runtime",
        "yahoo": "python asyncio tutorial",
        "wikipedia": "asynchronous programming",
        "github": "async runtime",
        "arxiv": "async runtime",
        "hackernews": "rust async runtime",
    }

    def test_every_engine_answers(self):
        import time

        options = jws.SearchOptions(api_key=(os.environ.get("JINA_API_KEY") or "").strip() or None)
        for name, query in self.QUERIES.items():
            with self.subTest(engine=name):
                started = time.monotonic()
                report = jws.search_engine(jws.ENGINES[name], query, options)
                elapsed = time.monotonic() - started
                print(
                    f"\n{name}: {len(report.results)} results in {elapsed:.1f}s {report.error or ''}",
                    file=sys.stderr,
                )
                self.assertIsNone(report.error)
                self.assertTrue(report.results)
                for result in report.results:
                    self.assertTrue(result.title)
                    self.assertTrue(result.url.startswith(("http://", "https://")), result.url)
            time.sleep(3)


if __name__ == "__main__":
    unittest.main()
