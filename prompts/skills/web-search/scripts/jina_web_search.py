#!/usr/bin/env python3
"""Search the web through Jina Reader (https://r.jina.ai).

A search engine's own results page is fetched through Jina Reader, which does
the fetching from its own servers, and parsed here into a title, a URL, a date
and a snippet per result. Standard library only (Python 3.8 or newer).

The engines are the ones that answered quickly and consistently through Jina
Reader without an API key: DuckDuckGo (the default), Yahoo, Wikipedia, GitHub,
arXiv and Hacker News. Google, Bing, Brave, Startpage, Mojeek, Ecosia, Yandex,
Qwant, AOL, Ask and others were tried and left out: they answer with bot checks
or script-only pages, or Jina refuses them without a key.

JINA_API_KEY, when set, authenticates every request (Jina allows 500 requests
a minute with a key, 20 without). It is sent only to r.jina.ai and never
printed.
"""

from __future__ import annotations

import argparse
import concurrent.futures
import datetime
import html
import http.client
import json
import os
import re
import socket
import sys
import time
import urllib.error
import urllib.parse
import urllib.request
import zlib
from dataclasses import dataclass, field
from html.parser import HTMLParser

JINA_READER = "https://r.jina.ai/"
USER_AGENT = "jina-web-search/1.0 (alter-zero built-in skill)"
TIME_WINDOWS = ("day", "week", "month", "year")
TIME_DAYS = {"day": 1, "week": 7, "month": 31, "year": 366}
MAX_PAGES = 10
DEFAULT_TIMEOUT = 90.0
MAX_BODY_BYTES = 16 * 1024 * 1024
SNIPPET_MAX_CHARS = 400
# How long a 429's Retry-After may ask us to wait before we give up instead.
MAX_RETRY_WAIT = 20.0
PROG = "jina_web_search.py"


# ===== errors =====


class SearchError(Exception):
    """A failure worth reporting for one engine."""


class SoftBlock(Exception):
    """The engine answered with a bot check or a temporary error page."""


class TransientError(SearchError):
    """A network failure that one retry may cure."""


class JinaHTTPError(SearchError):
    """Jina Reader answered with an HTTP error status."""

    def __init__(self, status, message="", code=None, retry_after=None, keyed=False):
        self.status = status
        self.message = message
        self.code = code
        self.retry_after = retry_after
        self.keyed = keyed
        super().__init__(self.describe())

    @classmethod
    def from_response(cls, status, body, retry_after=None, keyed=False):
        message, code = "", None
        try:
            data = json.loads(body)
        except ValueError:
            data = None
        if isinstance(data, dict):
            message = str(data.get("message") or data.get("readableMessage") or "")
            code = data.get("status")
        elif body:
            message = " ".join(body.split())
        return cls(status, message[:300], code, parse_retry_after(retry_after), keyed)

    def describe(self):
        detail = f"HTTP {self.status}" + (f": {self.message}" if self.message else "")
        if self.status == 429:
            return (
                f"Jina Reader's rate limit was reached ({detail}). Without a key it "
                "allows about 20 requests a minute: wait, fetch fewer pages or "
                "engines, or set JINA_API_KEY."
            )
        if self.status == 401 and self.keyed:
            return f"Jina Reader rejected JINA_API_KEY ({detail})."
        if self.status == 401:
            return (
                f"Jina Reader refused the anonymous request ({detail}). Set "
                "JINA_API_KEY to authenticate (keys: https://jina.ai/api-dashboard)."
            )
        if self.status == 402:
            return f"Jina Reader: the API key has no balance left ({detail})."
        if self.status == 422:
            return f"Jina Reader could not fetch the page ({detail})."
        return f"Jina Reader returned {detail}."

    def retry_wait(self):
        """Seconds to wait before the one retry, or None when retrying is pointless."""
        if self.status == 429:
            if self.retry_after is None:
                return 3.0
            return self.retry_after if self.retry_after <= MAX_RETRY_WAIT else None
        if self.status == 401 and not self.keyed and self.code == 40103:
            # Jina's anonymous IP-reputation refusal comes and goes.
            return 2.0
        if self.status == 422 or self.status >= 500:
            return 1.0
        return None


def parse_retry_after(value):
    """A Retry-After header's delay in seconds (delta-seconds only), else None."""
    if value is None:
        return None
    try:
        seconds = float(str(value).strip())
    except ValueError:
        return None
    return max(seconds, 0.0)


# ===== the request =====


def jina_url(target):
    """The Jina Reader URL that fetches `target` unchanged.

    The whole target, its `?` and `&` included, is quoted into the path, for
    two reasons. Jina reads a query string of its own request as its own
    options, so a target's `page=0` (Hacker News pages from 0) was refused by
    Jina's `page` option. And Jina percent-decodes the path once before
    fetching it, so without the extra quoting `c%2B%2B` would reach the
    engine as `c++` (two spaces).
    """
    return JINA_READER + urllib.parse.quote(target, safe=":/")


def request_headers(respond_with, api_key=None, no_cache=False):
    # No `X-Engine`: forcing Jina's `curl` engine made it treat a JSON
    # response as HTML in text mode — tags stripped and `&quot;` decoded into
    # a bare `"` — which broke Wikipedia's JSON. Jina's default returns the
    # JSON verbatim and the engines' own HTML for every page here.
    headers = {
        "User-Agent": USER_AGENT,
        "Accept-Encoding": "gzip",
        "X-Respond-With": respond_with,
    }
    if no_cache:
        headers["X-No-Cache"] = "true"
    if api_key:
        headers["Authorization"] = "Bearer " + api_key
    return headers


def fetch(url, *, respond_with, timeout, api_key=None, no_cache=False):
    """Fetch `url` through Jina Reader and return the body as text."""
    request = urllib.request.Request(
        jina_url(url), headers=request_headers(respond_with, api_key, no_cache)
    )
    try:
        with urllib.request.urlopen(request, timeout=timeout) as response:
            return decode_body(read_limited(response), response.headers)
    except urllib.error.HTTPError as err:
        try:
            body = decode_body(read_limited(err, 65536), err.headers)
        except (SearchError, OSError, http.client.HTTPException):
            body = ""
        raise JinaHTTPError.from_response(
            err.code, body, err.headers.get("Retry-After"), keyed=bool(api_key)
        ) from None
    except urllib.error.URLError as err:
        if isinstance(err.reason, socket.timeout):
            raise TransientError(f"timed out after {timeout:g}s") from None
        raise TransientError(f"could not reach r.jina.ai: {err.reason}") from None
    except socket.timeout:
        raise TransientError(f"timed out after {timeout:g}s") from None
    except (http.client.HTTPException, OSError) as err:
        raise TransientError(f"the connection to r.jina.ai failed: {err}") from None


def read_limited(response, limit=MAX_BODY_BYTES):
    data = response.read(limit + 1)
    if len(data) > limit:
        raise SearchError(f"the response is larger than {limit // (1024 * 1024)} MB")
    return data


def decode_body(raw, headers):
    if (headers.get("Content-Encoding") or "").strip().lower() == "gzip":
        inflater = zlib.decompressobj(16 + zlib.MAX_WBITS)
        try:
            raw = inflater.decompress(raw, MAX_BODY_BYTES + 1)
        except zlib.error as err:
            raise SearchError(f"could not decompress the response: {err}") from None
        if len(raw) > MAX_BODY_BYTES or inflater.unconsumed_tail:
            raise SearchError("the decompressed response is too large")
    charset = headers.get_content_charset() if hasattr(headers, "get_content_charset") else None
    try:
        return raw.decode(charset or "utf-8", errors="replace")
    except LookupError:
        return raw.decode("utf-8", errors="replace")


# ===== a small HTML tree =====

VOID_TAGS = frozenset(
    "area base br col embed hr img input link meta param source track wbr".split()
)
SKIPPED_TEXT_TAGS = frozenset(("script", "style", "template"))
BLOCK_TAGS = frozenset(
    "address article aside blockquote br dd div dl dt figcaption footer form h1 h2 "
    "h3 h4 h5 h6 header hr li main nav ol p pre section table td th tr ul".split()
)


class Node:
    """One element; children are Nodes and text strings."""

    __slots__ = ("tag", "attrs", "children", "parent")

    def __init__(self, tag, attrs=(), parent=None):
        self.tag = tag
        self.attrs = {name: value or "" for name, value in attrs}
        self.children = []
        self.parent = parent

    def classes(self):
        return self.attrs.get("class", "").split()

    def iter(self):
        """Every descendant element, in document order."""
        stack = list(reversed(self.children))
        while stack:
            node = stack.pop()
            if isinstance(node, Node):
                yield node
                stack.extend(reversed(node.children))

    def matches(self, tag=None, cls=None, id=None):
        return (
            (tag is None or self.tag == tag)
            and (cls is None or cls in self.classes())
            and (id is None or self.attrs.get("id") == id)
        )

    def find_all(self, tag=None, cls=None, id=None):
        return [node for node in self.iter() if node.matches(tag, cls, id)]

    def find(self, tag=None, cls=None, id=None):
        for node in self.iter():
            if node.matches(tag, cls, id):
                return node
        return None

    def text(self):
        """The element's text, whitespace collapsed, blocks kept apart."""
        parts = []
        stack = list(reversed(self.children))
        while stack:
            node = stack.pop()
            if isinstance(node, str):
                parts.append(node)
            elif node.tag in BLOCK_TAGS:
                stack.append(" ")
                stack.extend(reversed(node.children))
                stack.append(" ")
            else:
                stack.extend(reversed(node.children))
        return clean("".join(parts))

    def remove(self):
        if self.parent is not None:
            self.parent.children = [c for c in self.parent.children if c is not self]
            self.parent = None


class _TreeBuilder(HTMLParser):
    def __init__(self):
        super().__init__(convert_charrefs=True)
        self.root = Node("#document")
        self.stack = [self.root]

    def handle_starttag(self, tag, attrs):
        node = Node(tag, attrs, self.stack[-1])
        self.stack[-1].children.append(node)
        if tag not in VOID_TAGS:
            self.stack.append(node)

    def handle_endtag(self, tag):
        # Close the nearest open element of that name; a stray end tag is
        # ignored rather than allowed to unwind the whole tree.
        for index in range(len(self.stack) - 1, 0, -1):
            if self.stack[index].tag == tag:
                del self.stack[index:]
                return

    def handle_data(self, data):
        if self.stack[-1].tag not in SKIPPED_TEXT_TAGS:
            self.stack[-1].children.append(data)


def parse_html(body):
    builder = _TreeBuilder()
    builder.feed(body)
    builder.close()
    return builder.root


# ===== text helpers =====


def clean(text):
    return " ".join((text or "").split())


def strip_tags(markup):
    """Plain text from a fragment of HTML carried inside JSON."""
    text = re.sub(r"(?i)<br\s*/?>|</?p\b[^>]*>", " ", markup or "")
    return clean(html.unescape(re.sub(r"<[^>]+>", "", text)))


def shorten(text, limit=SNIPPET_MAX_CHARS):
    text = clean(text)
    if len(text) <= limit:
        return text
    cut = text[: limit - 1]
    space = cut.rfind(" ")
    if space > limit // 2:
        cut = cut[:space]
    return cut.rstrip(" ,;:.-") + "…"


def with_params(url, **changes):
    """`url` with query parameters set (or removed, given None)."""
    parts = urllib.parse.urlsplit(url)
    query = [
        (name, value)
        for name, value in urllib.parse.parse_qsl(parts.query, keep_blank_values=True)
        if name not in changes
    ]
    query.extend((name, str(value)) for name, value in changes.items() if value is not None)
    return urllib.parse.urlunsplit(parts._replace(query=urllib.parse.urlencode(query)))


def load_json(body):
    """JSON from a Jina text response, tolerating Jina's `Title:` header."""
    text = (body or "").strip()
    start = text.find("{")
    if start < 0:
        raise SearchError("the response holds no JSON")
    try:
        value, _ = json.JSONDecoder(strict=False).raw_decode(text, start)
    except ValueError as err:
        raise SearchError(f"the response is not valid JSON: {err}") from None
    if not isinstance(value, dict):
        raise SearchError("the response is not a JSON object")
    return value


def since(time_window, now=None):
    """The UTC instant `time_window` reaches back to."""
    now = now or datetime.datetime.now(datetime.timezone.utc)
    return now - datetime.timedelta(days=TIME_DAYS[time_window])


# ===== results =====


@dataclass
class Result:
    engine: str
    title: str
    url: str
    date: str = ""
    snippet: str = ""
    info: str = ""

    def as_json(self):
        return {
            "engine": self.engine,
            "title": self.title,
            "url": self.url,
            "date": self.date or None,
            "snippet": self.snippet or None,
            "info": self.info or None,
        }


@dataclass
class Page:
    results: list
    next_url: str | None = None


@dataclass
class EngineReport:
    engine: str
    results: list = field(default_factory=list)
    pages: int = 0
    error: str | None = None
    warnings: list = field(default_factory=list)


def dedupe_key(url):
    parts = urllib.parse.urlsplit(url)
    host = parts.netloc.lower()
    if host.startswith("www."):
        host = host[4:]
    return (host, parts.path.rstrip("/") or "/", parts.query)


# ===== the engines =====


class Engine:
    name = ""
    aliases = ()
    summary = ""
    respond_with = "html"
    paged = True
    times = TIME_WINDOWS

    def first_url(self, query, time_window=None, now=None):
        raise NotImplementedError

    def parse(self, body, url):
        raise NotImplementedError


class DuckDuckGo(Engine):
    name = "duckduckgo"
    aliases = ("ddg",)
    summary = "web results (the default)"
    BASE = "https://html.duckduckgo.com/html/"
    TIME = {"day": "d", "week": "w", "month": "m", "year": "y"}

    def first_url(self, query, time_window=None, now=None):
        params = {"q": query}
        if time_window:
            params["df"] = self.TIME[time_window]
        return self.BASE + "?" + urllib.parse.urlencode(params)

    def parse(self, body, url):
        if (
            "anomaly-modal" in body
            or 'id="challenge-form"' in body
            or "bots use DuckDuckGo too" in body
        ):
            raise SoftBlock("DuckDuckGo answered with a bot check")
        doc = parse_html(body)
        links = doc.find(id="links")
        if links is None:
            if doc.find(cls="no-results") is not None:
                return Page([])
            raise SearchError("DuckDuckGo returned a page this script does not recognise")
        results = []
        for block in links.find_all("div", cls="result"):
            if "result--ad" in block.classes():
                continue
            anchor = block.find("a", cls="result__a")
            if anchor is None:
                continue
            target = self.target(anchor.attrs.get("href", ""))
            if not target:
                continue
            snippet = block.find(cls="result__snippet")
            results.append(
                Result(
                    self.name,
                    anchor.text() or target,
                    target,
                    date=self.date(block),
                    snippet=shorten(snippet.text()) if snippet is not None else "",
                )
            )
        return Page(results, self.next_url(doc))

    @staticmethod
    def target(href):
        """The result's own URL, out of DuckDuckGo's `/l/?uddg=` redirect."""
        if href.startswith("//"):
            href = "https:" + href
        parts = urllib.parse.urlsplit(href)
        if parts.netloc.endswith("duckduckgo.com"):
            if parts.path.startswith("/l/"):
                found = urllib.parse.parse_qs(parts.query).get("uddg")
                return found[0] if found else ""
            return ""  # an ad (`/y.js`) or a DuckDuckGo page of its own
        return href if parts.scheme in ("http", "https") else ""

    @staticmethod
    def date(block):
        extras = block.find(cls="result__extras__url")
        if extras is None:
            return ""
        for span in extras.find_all("span"):
            if "result__icon" in span.classes():
                continue
            text = span.text()
            if text:
                return text
        return ""

    def next_url(self, doc):
        """Page 2 onwards come from the "Next" form, replayed as a GET."""
        for form in doc.find_all("form"):
            inputs = form.find_all("input")
            if not any(
                field.attrs.get("type") == "submit" and field.attrs.get("value") == "Next"
                for field in inputs
            ):
                continue
            params = [
                (field.attrs["name"], field.attrs.get("value", ""))
                for field in inputs
                if field.attrs.get("name") and field.attrs.get("type") != "submit"
            ]
            if params:
                return self.BASE + "?" + urllib.parse.urlencode(params)
        return None


class Yahoo(Engine):
    name = "yahoo"
    summary = "web results (Bing's index)"
    BASE = "https://search.yahoo.com/search"
    TIME = {"day": "d", "week": "w", "month": "m", "year": "y"}

    def first_url(self, query, time_window=None, now=None):
        # Without `fr`, Yahoo answered most requests with "We had temporary
        # problems searching"; with it, nearly all of them came back.
        params = {"p": query, "ei": "UTF-8", "fr": "fp-tts"}
        if time_window:
            params["btf"] = self.TIME[time_window]
        return self.BASE + "?" + urllib.parse.urlencode(params)

    def parse(self, body, url):
        if "We had temporary problems searching" in body:
            raise SoftBlock("Yahoo answered with its temporary-problems page")
        doc = parse_html(body)
        web = doc.find(id="web")
        if web is None:
            if "We did not find results" in body:
                return Page([])
            raise SearchError("Yahoo returned a page this script does not recognise")
        results = []
        for block in web.find_all("div", cls="algo"):
            heading = block.find("h3")
            title_box = block.find(cls="compTitle")
            anchor = title_box.find("a") if title_box is not None else block.find("a")
            if heading is None or anchor is None:
                continue
            target = self.target(anchor.attrs.get("href", ""))
            if not target:
                continue
            date, snippet = "", ""
            text_box = block.find(cls="compText")
            if text_box is not None:
                stamp = text_box.find("span", cls="fc-smoke")
                if stamp is not None:
                    date = stamp.text().rstrip("·").strip()
                    stamp.remove()
                snippet = shorten(text_box.text())
            results.append(
                Result(self.name, heading.text() or target, target, date=date, snippet=snippet)
            )
        following = doc.find("a", cls="next")
        next_url = following.attrs.get("href") if following is not None else None
        return Page(results, next_url or None)

    @staticmethod
    def target(href):
        """The result's own URL, out of Yahoo's `/RU=…/RK=` redirect."""
        found = re.search(r"/RU=([^/]+)", href)
        if found:
            return urllib.parse.unquote(found.group(1))
        parts = urllib.parse.urlsplit(href)
        if parts.scheme not in ("http", "https") or parts.netloc.endswith("search.yahoo.com"):
            return ""
        return href


class Wikipedia(Engine):
    name = "wikipedia"
    aliases = ("wiki",)
    summary = "English Wikipedia articles"
    respond_with = "text"
    times = ()
    API = "https://en.wikipedia.org/w/api.php"
    PAGE_SIZE = 10

    def first_url(self, query, time_window=None, now=None):
        params = {
            "action": "query",
            "list": "search",
            "srsearch": query,
            "srlimit": self.PAGE_SIZE,
            "sroffset": 0,
            "srprop": "snippet|timestamp|wordcount",
            "format": "json",
            "formatversion": 2,
            "utf8": 1,
        }
        return self.API + "?" + urllib.parse.urlencode(params)

    def parse(self, body, url):
        data = load_json(body)
        if "error" in data:
            error = data["error"]
            info = error.get("info") if isinstance(error, dict) else error
            raise SearchError(f"Wikipedia: {info}")
        results = []
        for hit in (data.get("query") or {}).get("search") or []:
            title = hit.get("title") or ""
            if not title:
                continue
            # The characters Wikipedia's own article URLs leave unquoted.
            path = urllib.parse.quote(title.replace(" ", "_"), safe="/;:@$!*(),'~")
            words = hit.get("wordcount")
            results.append(
                Result(
                    self.name,
                    title,
                    "https://en.wikipedia.org/wiki/" + path,
                    date=hit.get("timestamp") or "",
                    snippet=shorten(strip_tags(hit.get("snippet"))),
                    info=f"{words:,} words" if isinstance(words, int) else "",
                )
            )
        offset = (data.get("continue") or {}).get("sroffset")
        return Page(results, with_params(url, sroffset=offset) if offset is not None else None)


class GitHub(Engine):
    name = "github"
    aliases = ("gh",)
    summary = "repositories (--time: pushed within the window)"
    BASE = "https://github.com/search"

    def first_url(self, query, time_window=None, now=None):
        if time_window:
            query += " pushed:>" + since(time_window, now).strftime("%Y-%m-%d")
        return self.BASE + "?" + urllib.parse.urlencode({"q": query, "type": "repositories"})

    def parse(self, body, url):
        found = re.search(
            r'<script[^>]*data-target="react-app\.embeddedData"[^>]*>(.*?)</script>', body, re.S
        )
        if found is None:
            if "rate limit" in body.lower():
                raise SoftBlock("GitHub answered with its rate-limit page")
            raise SearchError("GitHub returned a page this script does not recognise")
        try:
            data = json.loads(found.group(1))
        except ValueError as err:
            raise SearchError(f"GitHub's search data is not valid JSON: {err}") from None
        route = (data.get("payload") or {}).get("blackbirdSearchRoute")
        if not isinstance(route, dict):
            raise SearchError("GitHub returned a page this script does not recognise")
        results = []
        for item in route.get("results") or []:
            repo = (item.get("repo") or {}).get("repository") or {}
            owner, name = repo.get("owner_login"), repo.get("name")
            full = f"{owner}/{name}" if owner and name else strip_tags(item.get("hl_name"))
            if not full:
                continue
            stars = item.get("followers")
            info = [
                item.get("language") or "",
                f"★ {stars:,}" if isinstance(stars, int) else "",
                "archived" if item.get("archived") else "",
            ]
            results.append(
                Result(
                    self.name,
                    full,
                    "https://github.com/" + full,
                    date=repo.get("updated_at") or "",
                    snippet=shorten(strip_tags(item.get("hl_trunc_description"))),
                    info=" · ".join(part for part in info if part),
                )
            )
        page, count = route.get("page"), route.get("page_count")
        has_next = isinstance(page, int) and isinstance(count, int) and page < count
        return Page(results, with_params(url, p=page + 1) if has_next else None)


class ArXiv(Engine):
    name = "arxiv"
    summary = "research papers"
    times = ()
    BASE = "https://arxiv.org/search/"
    PAGE_SIZE = 25

    def first_url(self, query, time_window=None, now=None):
        params = {
            "query": query,
            "searchtype": "all",
            "abstracts": "show",
            "order": "",  # relevance; arXiv's own default is newest first
            "size": self.PAGE_SIZE,
        }
        return self.BASE + "?" + urllib.parse.urlencode(params)

    def parse(self, body, url):
        doc = parse_html(body)
        items = doc.find_all("li", cls="arxiv-result")
        if not items:
            if "produced no results" in body:
                return Page([])
            raise SearchError("arXiv returned a page this script does not recognise")
        results = []
        for item in items:
            heading = item.find("p", cls="title")
            label = item.find("p", cls="list-title")
            anchor = label.find("a") if label is not None else None
            if heading is None or anchor is None:
                continue
            authors = item.find("p", cls="authors")
            names = [a.text() for a in authors.find_all("a")] if authors is not None else []
            byline = ", ".join(names[:3]) + (" et al." if len(names) > 3 else "")
            tags = [tag.text() for tag in item.find_all("span", cls="tag")]
            abstract = item.find("span", cls="abstract-full") or item.find(
                "span", cls="abstract-short"
            )
            summary = abstract.text() if abstract is not None else ""
            summary = re.sub(r"\s*[△▽] (Less|More)\s*$", "", summary)
            results.append(
                Result(
                    self.name,
                    heading.text(),
                    urllib.parse.urljoin("https://arxiv.org/", anchor.attrs.get("href", "")),
                    date=self.submitted(item),
                    snippet=shorten(summary),
                    info=" · ".join(part for part in (byline, ", ".join(tags[:3])) if part),
                )
            )
        following = doc.find("a", cls="pagination-next")
        has_next = (
            following is not None
            and following.attrs.get("href")
            and "is-invisible" not in following.classes()
        )
        if not has_next:
            return Page(results)
        query = dict(urllib.parse.parse_qsl(urllib.parse.urlsplit(url).query))
        start = int(query.get("start") or 0) + self.PAGE_SIZE
        return Page(results, with_params(url, start=start))

    @staticmethod
    def submitted(item):
        for line in item.find_all("p", cls="is-size-7"):
            text = line.text()
            if text.startswith("Submitted"):
                return text[len("Submitted") :].split(";")[0].strip()
        return ""


class HackerNews(Engine):
    name = "hackernews"
    aliases = ("hn",)
    summary = "Hacker News stories"
    respond_with = "text"
    API = "https://hn.algolia.com/api/v1/search"
    PAGE_SIZE = 10

    def first_url(self, query, time_window=None, now=None):
        params = {"query": query, "tags": "story", "hitsPerPage": self.PAGE_SIZE, "page": 0}
        if time_window:
            params["numericFilters"] = f"created_at_i>{int(since(time_window, now).timestamp())}"
        return self.API + "?" + urllib.parse.urlencode(params)

    def parse(self, body, url):
        data = load_json(body)
        if "hits" not in data:
            raise SearchError(f"Hacker News search failed: {data.get('message') or 'no hits'}")
        results = []
        for hit in data.get("hits") or []:
            item_id = hit.get("objectID")
            title = hit.get("title") or hit.get("story_title") or ""
            if not item_id or not title:
                continue
            discussion = "https://news.ycombinator.com/item?id=" + str(item_id)
            points, comments = hit.get("points"), hit.get("num_comments")
            info = [
                f"{points} points" if points is not None else "",
                f"{comments} comments" if comments is not None else "",
                "by " + hit["author"] if hit.get("author") else "",
                discussion if hit.get("url") else "",
            ]
            results.append(
                Result(
                    self.name,
                    clean(title),
                    hit.get("url") or discussion,
                    date=hit.get("created_at") or "",
                    snippet=shorten(strip_tags(hit.get("story_text"))),
                    info=" · ".join(part for part in info if part),
                )
            )
        page, pages = data.get("page"), data.get("nbPages")
        has_next = isinstance(page, int) and isinstance(pages, int) and page + 1 < pages
        return Page(results, with_params(url, page=page + 1) if has_next else None)


ENGINES = {
    engine.name: engine
    for engine in (
        DuckDuckGo(),
        Yahoo(),
        Wikipedia(),
        GitHub(),
        ArXiv(),
        HackerNews(),
    )
}
DEFAULT_ENGINE = "duckduckgo"
ALIASES = {alias: name for name, engine in ENGINES.items() for alias in engine.aliases}


def resolve_engines(values):
    """Engine names from repeated and comma-separated `--engine` values."""
    names = []
    for value in values or [DEFAULT_ENGINE]:
        for part in value.split(","):
            word = part.strip().lower()
            if not word:
                continue
            if word == "all":
                chosen = list(ENGINES)
            elif word in ENGINES or word in ALIASES:
                chosen = [ALIASES.get(word, word)]
            else:
                raise ValueError(f"unknown engine {part.strip()!r} (choose from {engine_names()})")
            names.extend(name for name in chosen if name not in names)
    if not names:
        raise ValueError(f"no engine named (choose from {engine_names()})")
    return [ENGINES[name] for name in names]


def engine_names():
    return ", ".join(list(ENGINES) + ["all"])


# ===== the search =====


@dataclass
class SearchOptions:
    pages: int = 1
    limit: int | None = None
    time: str | None = None
    timeout: float = DEFAULT_TIMEOUT
    api_key: str | None = None
    no_cache: bool = False


def fetch_page(engine, url, options, fetcher, sleep):
    """One results page, with one retry for anything a retry may cure."""
    no_cache = options.no_cache
    for attempt in (1, 2):
        try:
            body = fetcher(
                url,
                respond_with=engine.respond_with,
                timeout=options.timeout,
                api_key=options.api_key,
                no_cache=no_cache,
            )
            return engine.parse(body, url)
        except SoftBlock as err:
            if attempt == 2:
                raise SearchError(
                    f"{err}, twice (the second time past Jina's cache); try again "
                    "later or use another engine"
                ) from None
            # Jina caches a page for an hour, the block page included.
            no_cache = True
            sleep(1.0)
        except JinaHTTPError as err:
            wait = err.retry_wait()
            if attempt == 2 or wait is None:
                raise
            sleep(wait)
        except TransientError:
            if attempt == 2:
                raise
            sleep(1.0)
    raise AssertionError("unreachable")


def search_engine(engine, query, options, fetcher=fetch, sleep=time.sleep):
    report = EngineReport(engine.name)
    if options.time and options.time not in engine.times:
        report.warnings.append("--time is not supported here; showing results from any time")
        time_window = None
    else:
        time_window = options.time
    if options.pages > 1 and not engine.paged:
        report.warnings.append("returns a single page; --pages does not apply")
    seen = set()
    url = engine.first_url(query, time_window)
    while url and report.pages < (options.pages if engine.paged else 1):
        try:
            page = fetch_page(engine, url, options, fetcher, sleep)
        except Exception as err:  # noqa: BLE001 — one engine's failure, not the run's
            if isinstance(err, SearchError):
                message = str(err)
            else:
                # A parser meeting markup the engine changed since this was
                # written: fail this engine, keep the others' results.
                message = f"could not read the page ({type(err).__name__}: {err})"
            message = sanitize(message, options.api_key)
            if report.pages == 0:
                report.error = message
            else:
                report.warnings.append(f"stopped after page {report.pages}: {message}")
            break
        report.pages += 1
        for result in page.results:
            key = dedupe_key(result.url)
            if key in seen:
                continue
            seen.add(key)
            report.results.append(result)
            if options.limit and len(report.results) >= options.limit:
                return report
        url = page.next_url
    return report


def sanitize(message, api_key):
    return message.replace(api_key, "***") if api_key else message


def run_search(query, engines, options, fetcher=fetch, sleep=time.sleep):
    """Every engine at once, each paging on its own; reports in `engines` order."""
    if len(engines) == 1:
        return [search_engine(engines[0], query, options, fetcher, sleep)]
    with concurrent.futures.ThreadPoolExecutor(max_workers=len(engines)) as pool:
        futures = [
            pool.submit(search_engine, engine, query, options, fetcher, sleep) for engine in engines
        ]
        return [future.result() for future in futures]


# ===== output =====


def format_result(result):
    lines = [result.title or result.url, result.url]
    if result.date:
        lines.append("Date: " + result.date)
    tail = " — ".join(part for part in (result.info, result.snippet) if part)
    if tail:
        lines.append(tail)
    return "\n".join(lines)


def render_text(reports, grouped):
    blocks = []
    for report in reports:
        if report.error is not None:
            continue
        if grouped:
            count = len(report.results)
            blocks.append(f"## {report.engine} ({count} result{'' if count == 1 else 's'})")
        if not report.results:
            blocks.append("No results.")
        blocks.extend(format_result(result) for result in report.results)
    return "\n\n".join(blocks)


def render_json(query, reports):
    document = {
        "query": query,
        "engines": [
            {
                "engine": report.engine,
                "pages": report.pages,
                "results": len(report.results),
                "error": report.error,
                "warnings": report.warnings,
            }
            for report in reports
        ],
        "results": [result.as_json() for report in reports for result in report.results],
    }
    return json.dumps(document, ensure_ascii=False, indent=2)


# ===== the command line =====


def positive_int(text):
    try:
        value = int(text)
    except ValueError:
        raise argparse.ArgumentTypeError(f"not a whole number: {text!r}") from None
    if value < 1:
        raise argparse.ArgumentTypeError("must be at least 1")
    return value


def page_count(text):
    value = positive_int(text)
    if value > MAX_PAGES:
        raise argparse.ArgumentTypeError(f"at most {MAX_PAGES}")
    return value


def positive_seconds(text):
    try:
        value = float(text)
    except ValueError:
        raise argparse.ArgumentTypeError(f"not a number: {text!r}") from None
    if not value > 0:
        raise argparse.ArgumentTypeError("must be more than 0")
    return value


def engines_epilog():
    rows = []
    for engine in ENGINES.values():
        names = engine.name + (f" ({', '.join(engine.aliases)})" if engine.aliases else "")
        paging = "pages" if engine.paged else "one page"
        times = "--time" if engine.times else "no --time"
        rows.append(f"  {names:<20} {engine.summary}; {paging}; {times}")
    return (
        "engines:\n"
        + "\n".join(rows)
        + "\n  all                  every engine above, searched at once\n\n"
        "environment:\n"
        "  JINA_API_KEY         optional Jina Reader key: 500 requests a minute\n"
        "                       instead of 20 (https://jina.ai/reader)\n\n"
        "Each result prints as its title, its URL, a Date: line when the engine\n"
        "dates it, and a line of details and snippet. With several engines the\n"
        "results are grouped under a heading per engine."
    )


def build_parser():
    parser = argparse.ArgumentParser(
        prog=PROG,
        description="Search the web through Jina Reader (r.jina.ai) and print\n"
        "each result's title, URL, date and snippet.",
        epilog=engines_epilog(),
        formatter_class=argparse.RawDescriptionHelpFormatter,
    )
    parser.add_argument(
        "--engine",
        action="append",
        metavar="ENGINE",
        help=f"search engine: {engine_names()}; default {DEFAULT_ENGINE}. "
        "Repeat it or separate names with commas to search several at once.",
    )
    parser.add_argument(
        "--limit",
        type=positive_int,
        help="maximum number of unique results per engine (default: every "
        "result on the pages fetched)",
    )
    parser.add_argument(
        "--pages",
        type=page_count,
        default=1,
        help=f"result pages to fetch from engines that page (default: 1; maximum: {MAX_PAGES})",
    )
    parser.add_argument(
        "--timeout",
        type=positive_seconds,
        default=DEFAULT_TIMEOUT,
        help=f"per-page request timeout in seconds (default: {DEFAULT_TIMEOUT:g})",
    )
    parser.add_argument(
        "--time",
        choices=TIME_WINDOWS,
        help="only results from the past day, week, month or year, on engines that support it",
    )
    parser.add_argument(
        "--no-cache",
        action="store_true",
        help="bypass Jina Reader's cache (pages are otherwise cached for up to an hour)",
    )
    parser.add_argument(
        "--json",
        action="store_true",
        help="output results as structured JSON instead of plain text",
    )
    parser.add_argument("query", nargs="*", help="the search terms to look up")
    return parser


def main(argv=None, *, env=None, fetcher=fetch, stdout=None, stderr=None, sleep=time.sleep):
    env = os.environ if env is None else env
    stdout = stdout or sys.stdout
    stderr = stderr or sys.stderr
    parser = build_parser()
    args = parser.parse_args(argv)
    query = " ".join(args.query).strip()
    if not query:
        parser.error("a search query is required")
    try:
        engines = resolve_engines(args.engine)
    except ValueError as err:
        parser.error(str(err))
    options = SearchOptions(
        pages=args.pages,
        limit=args.limit,
        time=args.time,
        timeout=args.timeout,
        api_key=(env.get("JINA_API_KEY") or "").strip() or None,
        no_cache=args.no_cache,
    )
    reports = run_search(query, engines, options, fetcher, sleep)
    if args.json:
        stdout.write(render_json(query, reports) + "\n")
    else:
        text = render_text(reports, grouped=len(engines) > 1)
        if text:
            stdout.write(text + "\n")
        for report in reports:
            for warning in report.warnings:
                stderr.write(f"{PROG}: {report.engine}: {warning}\n")
            if report.error is not None:
                stderr.write(f"{PROG}: {report.engine}: {report.error}\n")
    return 0 if any(report.error is None for report in reports) else 1


if __name__ == "__main__":
    for stream in (sys.stdout, sys.stderr):
        if hasattr(stream, "reconfigure"):
            stream.reconfigure(errors="replace")
    try:
        sys.exit(main())
    except KeyboardInterrupt:
        sys.exit(130)
