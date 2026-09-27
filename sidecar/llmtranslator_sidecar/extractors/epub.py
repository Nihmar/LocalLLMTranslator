# pyright: reportUnknownMemberType=false, reportUnknownVariableType=false, reportUnknownArgumentType=false
"""EPUB ingestion via ``ebooklib`` and BeautifulSoup.

Each spine document is an XHTML fragment; the job here is to flatten that markup into
canonical Markdown while keeping the structures the rest of the pipeline depends on:
headings (the chapter skeleton), paragraphs, lists, blockquotes, tables, fenced code,
images as Markdown links and footnotes as ``[^n]`` references with their definitions.

Images live *inside* the EPUB archive, so a Markdown link pointing at the archive path
would dangle for every consumer. Each reference is therefore resolved to its manifest item
(the ``src`` is relative to the chapter, not to the archive root), the bytes are written to
``<work_dir>/assets/<name>`` and the href is rewritten to ``assets/<name>``, which is
relative to the ``document.md`` written next to it.

The parser is deliberately line-oriented downstream, so the converter emits plain,
well-behaved Markdown with blank-line separation and never relies on HTML surviving into
the document. Anything it cannot represent faithfully is turned into a warning instead of
being dropped silently.

The unknown-type checks are relaxed file-wide because ``ebooklib`` ships no type
information, so every value it hands back is opaque to pyright and is narrowed at run time.
"""

from __future__ import annotations

import posixpath
import re
from typing import Any

import ebooklib
from bs4 import BeautifulSoup, Comment, Tag
from bs4.element import NavigableString, PageElement
from ebooklib import epub

from .base import ASSETS_DIRNAME, ExtractionError, ExtractResult, assets_dir, clear_assets

_HEADING_LEVELS = {f"h{level}": level for level in range(1, 7)}

#: ``epub:type``/``role`` values that mark a footnote or endnote definition.
_FOOTNOTE_TYPES = frozenset({"footnote", "endnote", "rearnote"})
_FOOTNOTE_ROLES = frozenset({"doc-footnote", "doc-endnote"})

#: Elements that only wrap other blocks and carry no structure of their own.
_CONTAINERS = frozenset(
    {
        "body",
        "div",
        "section",
        "article",
        "header",
        "footer",
        "main",
        "aside",
        "nav",
        "center",
        "details",
    },
)

#: Inline elements that may show up where a block is expected; rendered in place.
_INLINE_TAGS = frozenset(
    {
        "a",
        "abbr",
        "b",
        "br",
        "cite",
        "code",
        "del",
        "em",
        "i",
        "img",
        "ins",
        "kbd",
        "mark",
        "q",
        "s",
        "samp",
        "small",
        "span",
        "sub",
        "sup",
        "time",
        "u",
        "var",
    },
)

_IGNORED = frozenset({"script", "style", "head", "title", "meta", "link"})

_WHITESPACE_RE = re.compile(r"\s+")
_BACKTICK_RUN_RE = re.compile(r"`+")


def _collapse(text: str) -> str:
    """Fold HTML whitespace runs into single spaces, as a browser would."""
    return _WHITESPACE_RE.sub(" ", text)


def _classes(tag: Tag) -> set[str]:
    """Class list as a set, accepting the string form the XML parser can produce."""
    raw = tag.get("class")
    if isinstance(raw, str):
        return set(raw.split())
    if raw is None:
        return set()
    return {str(entry) for entry in raw}


def _backticks(text: str, minimum: int) -> str:
    longest = max((len(match.group(0)) for match in _BACKTICK_RUN_RE.finditer(text)), default=0)
    return "`" * max(minimum, longest + 1)


def _code_span(text: str) -> str:
    fence = _backticks(text, 1)
    padding = " " if text.startswith("`") or text.endswith("`") else ""
    return f"{fence}{padding}{text}{padding}{fence}"


def _code_block(text: str, language: str) -> str:
    fence = _backticks(text, 3)
    return f"{fence}{language}\n{text}\n{fence}"


def _escape_alt(text: str) -> str:
    """Escape the brackets that would close an image's alt text early."""
    return text.replace("\\", "\\\\").replace("[", "\\[").replace("]", "\\]")


def _escape_destination(url: str) -> str:
    """Escape the parentheses that would end a link destination early."""
    return url.replace("\\", "\\\\").replace("(", "\\(").replace(")", "\\)")


#: Block markers Markdown would read at the start of a line instead of the paragraph the
#: source had. An ordered item escapes its delimiter, the others escape the marker itself.
_ORDERED_MARKER_RE = re.compile(r"^ {0,3}\d{1,9}([.)])(?:\s|$)")
_BULLET_MARKER_RE = re.compile(r"^ {0,3}([-+*#>|])(?:\s|$)")


def _escape_leading_marker(text: str) -> str:
    """Backslash-escape a leading marker so the text stays the paragraph the EPUB had.

    Publishers set dialogue as ``<p><span>-</span> <span>…</span></p>``: a paragraph whose
    text begins with a dash. Written as ``- Bonjour`` Markdown reads a *list*, and the chunk
    then declares list blocks the translator is asked to reproduce — a translation that
    renders the same dialogue as prose changes the block count and is refused. ``\\-`` keeps
    the dash, keeps the paragraph, and renders as the dash the book printed.
    """
    for pattern in (_ORDERED_MARKER_RE, _BULLET_MARKER_RE):
        match = pattern.match(text)
        if match is not None:
            start = match.start(1)
            return f"{text[:start]}\\{text[start:]}"
    return text


def _alignment(cell: Tag) -> str:
    classes = _classes(cell)
    if "right" in classes:
        return "r"
    if "center" in classes or "centre" in classes:
        return "c"
    if "left" in classes:
        return "l"
    return ""


def _delimiter_cell(alignment: str) -> str:
    return {"l": ":---", "r": "---:", "c": ":---:"}.get(alignment, "---")


def _image_items(book: epub.EpubBook) -> dict[str, bytes]:
    """Map every image item's archive-relative name to its bytes."""
    items: dict[str, bytes] = {}
    for item in book.get_items():
        media_type = str(getattr(item, "media_type", "") or "")
        if not media_type.startswith("image/"):
            continue
        items[posixpath.normpath(str(item.get_name()))] = bytes(item.get_content())
    return items


class _AssetStore:
    """Materialises EPUB media into ``<work_dir>/assets`` and hands back rewritten hrefs.

    The store is shared by every spine document so one image referenced from several
    chapters is written once, and a name collision between two distinct items is
    disambiguated deterministically (``harbour.png``, ``harbour-2.png``, ...) in first-seen
    order — never by hashing or a timestamp, which would break the byte-identical
    re-extraction guarantee.
    """

    def __init__(self, work_dir: str, items: dict[str, bytes]) -> None:
        self._directory = assets_dir(work_dir)
        self._items = items
        self._assigned: dict[str, str] = {}
        self._used: set[str] = set()
        self._hrefs: list[str] = []

    @property
    def directory(self) -> str:
        return str(self._directory)

    @property
    def hrefs(self) -> list[str]:
        return list(self._hrefs)

    def resolve(self, base: str, source: str) -> str | None:
        """Write ``source`` (relative to ``base``) and return its new href.

        ``None`` means the reference did not resolve to an item in the archive; the caller
        then keeps the original href and warns, so nothing is dropped silently.
        """
        target = posixpath.normpath(posixpath.join(posixpath.dirname(base), source))
        data = self._items.get(target)
        if data is None:
            return None

        name = self._assigned.get(target)
        if name is None:
            name = self._unique(posixpath.basename(target) or "image")
            self._assigned[target] = name
            self._directory.mkdir(parents=True, exist_ok=True)
            (self._directory / name).write_bytes(data)
            self._hrefs.append(f"{ASSETS_DIRNAME}/{name}")
        return f"{ASSETS_DIRNAME}/{name}"

    def _unique(self, name: str) -> str:
        candidate = name
        stem, suffix = posixpath.splitext(name)
        counter = 1
        while candidate in self._used:
            counter += 1
            candidate = f"{stem}-{counter}{suffix}"
        self._used.add(candidate)
        return candidate


class _ChapterRenderer:
    """Flattens one spine document to Markdown, resolving footnotes as it goes."""

    def __init__(
        self,
        definitions: dict[str, Tag],
        numbers: dict[str, int],
        warnings: list[str],
        assets: _AssetStore,
        base_name: str,
    ) -> None:
        self._definitions = definitions
        self._numbers = numbers
        self._warnings = warnings
        self._assets = assets
        self._base_name = base_name
        self._in_definition = False

    #: Recorded the first time a bare text node has to be promoted to a paragraph.
    _FLATTEN_WARNING = "text outside a block element was flattened into a paragraph"

    def render(self, root: Tag) -> str:
        return self._children_blocks(root)

    # -- block level -----------------------------------------------------------

    def _block(self, tag: Tag) -> str:
        if self._is_definition(tag):
            return self._definition(tag)
        name = tag.name.lower()
        handler_name = _BLOCK_DISPATCH.get(name)
        if handler_name is not None:
            handler: Any = getattr(self, handler_name)
            return str(handler(tag))
        if name in _CONTAINERS:
            return self._children_blocks(tag)
        if name in _INLINE_TAGS:
            return self._inline(tag)
        self._warnings.append(f"rendered unsupported element <{name}> as plain text")
        return self._inline_children(tag).strip()

    def _children_blocks(self, tag: Tag) -> str:
        blocks: list[str] = []
        for child in tag.children:
            if isinstance(child, Comment):
                continue
            if isinstance(child, NavigableString):
                # A non-whitespace text node directly inside a container is not valid
                # block content, but dropping it would silently lose the source text;
                # promote it to a paragraph instead and say the HTML had to be flattened.
                text = _collapse(str(child)).strip()
                if text:
                    self._warn(self._FLATTEN_WARNING)
                    blocks.append(_escape_leading_marker(text))
                continue
            if isinstance(child, Tag) and child.name.lower() not in _IGNORED:
                blocks.append(self._block(child))
        return "\n\n".join(block for block in blocks if block)

    def _warn(self, message: str) -> None:
        """Record ``message`` once, so a repeated degradation does not flood the caller."""
        if message not in self._warnings:
            self._warnings.append(message)

    def _heading(self, tag: Tag) -> str:
        level = _HEADING_LEVELS[tag.name.lower()]
        return f"{'#' * level} {self._inline_children(tag).strip()}"

    def _paragraph(self, tag: Tag) -> str:
        return _escape_leading_marker(self._inline_children(tag).strip())

    def _horizontal_rule(self, _tag: Tag) -> str:
        return "---"

    def _list(self, tag: Tag) -> str:
        ordered = tag.name.lower() == "ol"
        raw_start = tag.get("start")
        try:
            start = int(str(raw_start)) if raw_start is not None else 1
        except ValueError:
            start = 1
        lines: list[str] = []
        for offset, item in enumerate(tag.find_all("li", recursive=False)):
            marker = f"{start + offset}." if ordered else "-"
            text, nested = self._list_item(item)
            lines.append(f"{marker} {text}")
            for child in nested:
                lines.extend(f"  {line}" for line in self._list(child).split("\n"))
        return "\n".join(lines)

    def _list_item(self, item: Tag) -> tuple[str, list[Tag]]:
        parts: list[str] = []
        nested: list[Tag] = []
        for child in item.children:
            if isinstance(child, Tag) and child.name.lower() in {"ul", "ol"}:
                nested.append(child)
            else:
                parts.append(self._inline(child))
        return _collapse("".join(parts)).strip(), nested

    def _blockquote(self, tag: Tag) -> str:
        inner = self._children_blocks(tag)
        return "\n".join(f"> {line}" if line.strip() else ">" for line in inner.split("\n"))

    def _code(self, tag: Tag) -> str:
        code = tag.find("code")
        language = ""
        if code is not None:
            # Sorted so the chosen ``language-*`` class does not depend on set iteration
            # order: two extractions must produce byte-identical blocks.
            for name in sorted(_classes(code)):
                if name.startswith("language-"):
                    language = name[len("language-") :]
                    break
            text = code.get_text()
        else:
            text = tag.get_text()
        return _code_block(text.rstrip("\n"), language)

    def _table(self, tag: Tag) -> str:
        rows = tag.find_all("tr")
        if not rows:
            self._warnings.append("table without rows was dropped")
            return ""
        header = rows[0].find_all(["th", "td"])
        alignments = [_alignment(cell) for cell in header]
        lines = [
            self._table_row(header),
            "| " + " | ".join(_delimiter_cell(align) for align in alignments) + " |",
        ]
        lines.extend(self._table_row(row.find_all(["th", "td"])) for row in rows[1:])
        caption = tag.find("caption")
        if caption is not None:
            text = self._inline_children(caption).strip()
            if text:
                # The blank line keeps the caption its own paragraph: with the caption
                # glued to the header row a GFM parser folds both into one paragraph and
                # never recognises the table at all. The round-trip is unaffected.
                lines.insert(0, "")
                lines.insert(0, text)
        return "\n".join(lines)

    def _table_row(self, cells: list[Tag]) -> str:
        return "| " + " | ".join(self._table_cell(cell) for cell in cells) + " |"

    def _table_cell(self, cell: Tag) -> str:
        text = self._inline_children(cell).strip()
        return _collapse(text).replace("|", "\\|")

    def _figure(self, tag: Tag) -> str:
        parts: list[str] = []
        image = tag.find("img")
        alt = _collapse(str(image.get("alt") or "")).strip() if image is not None else ""
        if image is not None:
            parts.append(self._inline(image))
        caption = tag.find("figcaption")
        if caption is not None:
            text = self._inline_children(caption).strip()
            # A caption identical to the alt text adds nothing the image link lacks.
            if text and text != alt:
                parts.append(f"*{text}*")
        return "\n\n".join(part for part in parts if part)

    def _definition_list(self, tag: Tag) -> str:
        parts: list[str] = []
        for child in tag.find_all(["dt", "dd"], recursive=False):
            text = self._inline_children(child).strip()
            parts.append(f"**{text}**" if child.name.lower() == "dt" else text)
        return "\n\n".join(part for part in parts if part)

    def _is_definition(self, tag: Tag) -> bool:
        identifier = tag.get("id")
        return isinstance(identifier, str) and identifier in self._definitions

    def _definition(self, tag: Tag) -> str:
        number = self._numbers.get(str(tag.get("id")))
        self._in_definition = True
        try:
            body = _collapse(self._inline_children(tag)).strip()
        finally:
            self._in_definition = False
        return f"[^{number}]: {body}"

    # -- inline level ----------------------------------------------------------

    def _inline_children(self, tag: Tag) -> str:
        return "".join(self._inline(child) for child in tag.children)

    def _inline(self, node: PageElement) -> str:
        if isinstance(node, Comment):
            return ""
        if isinstance(node, NavigableString):
            return _collapse(str(node))
        if not isinstance(node, Tag):
            return ""
        name = node.name.lower()
        handler_name = _INLINE_DISPATCH.get(name)
        if handler_name is not None:
            handler: Any = getattr(self, handler_name)
            return str(handler(node))
        if name in _IGNORED:
            return ""
        return self._inline_children(node)

    def _bold(self, tag: Tag) -> str:
        return f"**{self._inline_children(tag)}**"

    def _italic(self, tag: Tag) -> str:
        return f"*{self._inline_children(tag)}*"

    def _inline_code(self, tag: Tag) -> str:
        return _code_span(_collapse(tag.get_text()))

    def _line_break(self, _tag: Tag) -> str:
        return "  \n"

    def _span(self, tag: Tag) -> str:
        classes = _classes(tag)
        if "math" in classes or "maths" in classes:
            return f"${tag.get_text().strip()}$"
        return self._inline_children(tag)

    def _image(self, tag: Tag) -> str:
        alt = _collapse(str(tag.get("alt") or "")).strip()
        src = tag.get("src")
        if not isinstance(src, str) or not src.strip():
            self._warnings.append("image without a source was replaced by its alt text")
            return alt
        reference = src.strip()
        href = self._assets.resolve(self._base_name, reference)
        if href is None:
            self._warn(f"image {reference!r} could not be resolved; its href was left unchanged")
            href = reference
        return f"![{_escape_alt(alt)}]({_escape_destination(href)})"

    def _anchor(self, tag: Tag) -> str:
        text = self._inline_children(tag)
        href = tag.get("href")
        if isinstance(href, str) and href.startswith("#"):
            target = href[1:]
            number = self._numbers.get(target)
            if target in self._definitions and number is not None:
                return f"[^{number}]"
            if self._in_definition:
                # Back-reference from the note body to its marker: Markdown footnotes
                # carry the link implicitly, so the marker itself is dropped.
                return ""
            return f"[{text}]({_escape_destination(href)})" if text.strip() else text
        if isinstance(href, str) and href.strip():
            return f"[{text}]({_escape_destination(href)})" if text.strip() else text
        return text


#: Block tag name -> renderer method, so :meth:`_ChapterRenderer._block` stays a lookup.
_BLOCK_DISPATCH: dict[str, str] = {
    "h1": "_heading",
    "h2": "_heading",
    "h3": "_heading",
    "h4": "_heading",
    "h5": "_heading",
    "h6": "_heading",
    "p": "_paragraph",
    "hr": "_horizontal_rule",
    "ul": "_list",
    "ol": "_list",
    "blockquote": "_blockquote",
    "pre": "_code",
    "table": "_table",
    "figure": "_figure",
    "dl": "_definition_list",
}

#: Inline tag name -> renderer method, the inline counterpart of ``_BLOCK_DISPATCH``.
_INLINE_DISPATCH: dict[str, str] = {
    "strong": "_bold",
    "b": "_bold",
    "em": "_italic",
    "i": "_italic",
    "code": "_inline_code",
    "br": "_line_break",
    "img": "_image",
    "a": "_anchor",
    "span": "_span",
}


class _BookConverter:
    """Renders a whole book, keeping footnote numbering unique across documents."""

    def __init__(self, warnings: list[str], assets: _AssetStore) -> None:
        self._warnings = warnings
        self._assets = assets
        self._counter = 0

    def convert(self, soup: BeautifulSoup, base_name: str) -> str:
        definitions = self._collect_definitions(soup)
        numbers = self._number_footnotes(soup, definitions)
        renderer = _ChapterRenderer(
            definitions,
            numbers,
            self._warnings,
            self._assets,
            base_name,
        )
        root = soup.body if soup.body is not None else soup
        return renderer.render(root)

    def _collect_definitions(self, soup: BeautifulSoup) -> dict[str, Tag]:
        definitions: dict[str, Tag] = {}
        for tag in soup.find_all(name=True):
            identifier = tag.get("id")
            if not isinstance(identifier, str) or not identifier:
                continue
            epub_type = tag.get("epub:type")
            role = tag.get("role")
            typed = isinstance(epub_type, str) and bool(set(epub_type.split()) & _FOOTNOTE_TYPES)
            role_marked = isinstance(role, str) and role in _FOOTNOTE_ROLES
            if typed or role_marked:
                definitions[identifier] = tag
        return definitions

    def _number_footnotes(self, soup: BeautifulSoup, definitions: dict[str, Tag]) -> dict[str, int]:
        numbers: dict[str, int] = {}
        for anchor in soup.find_all("a", href=True):
            href = anchor.get("href")
            if isinstance(href, str) and href.startswith("#") and href[1:] in definitions:
                target = href[1:]
                if target not in numbers:
                    self._counter += 1
                    numbers[target] = self._counter
        for identifier in definitions:
            if identifier not in numbers:
                self._counter += 1
                numbers[identifier] = self._counter
        return numbers


def _metadata(book: epub.EpubBook) -> dict[str, Any]:
    def values(name: str) -> list[str]:
        collected: list[str] = []
        for entry in book.get_metadata("DC", name):
            value = entry[0] if entry else None
            if value is not None:
                collected.append(str(value))
        return collected

    metadata: dict[str, Any] = {}
    title = values("title")
    if title:
        metadata["title"] = title[0]
    creators = values("creator")
    if creators:
        metadata["author"] = ", ".join(creators)
    languages = values("language")
    if languages:
        metadata["language"] = languages[0]
    publishers = values("publisher")
    if publishers:
        metadata["publisher"] = ", ".join(publishers)
    dates = values("date")
    if dates:
        metadata["date"] = dates[0]
    identifiers = values("identifier")
    if identifiers:
        metadata["identifier"] = identifiers[0]
    return metadata


def _reading_order(book: epub.EpubBook) -> list[epub.EpubItem]:
    ordered: list[epub.EpubItem] = []
    seen: set[str] = set()

    def visit(item: epub.EpubItem | None) -> None:
        if item is None or isinstance(item, epub.EpubNav):
            return
        name = str(item.get_name())
        if name in seen:
            return
        seen.add(name)
        ordered.append(item)

    for entry in book.spine:
        identifier = entry[0] if isinstance(entry, tuple) else entry
        visit(book.get_item_with_id(str(identifier)))
    for item in book.get_items_of_type(ebooklib.ITEM_DOCUMENT):
        visit(item)
    return ordered


class EpubExtractor:
    """Backend for EPUB 2/3 documents."""

    format = "epub"

    def extract(self, path: str, work_dir: str) -> ExtractResult:
        try:
            book = epub.read_epub(path)
        except Exception as exc:
            message = f"cannot read EPUB {path}: {exc}"
            raise ExtractionError(message) from exc

        # Reset first: a re-extraction of the same (or another) source must never leave a
        # previously written image behind, so the assets directory matches the result.
        clear_assets(work_dir)

        warnings: list[str] = []
        assets = _AssetStore(work_dir, _image_items(book))
        converter = _BookConverter(warnings, assets)
        documents: list[str] = []
        for item in _reading_order(book):
            soup = BeautifulSoup(item.get_content(), "xml")
            rendered = converter.convert(soup, str(item.get_name()))
            if rendered.strip():
                documents.append(rendered)

        if not documents:
            warnings.append("EPUB contains no readable text documents")

        hrefs = assets.hrefs
        return ExtractResult(
            markdown="\n\n".join(documents),
            metadata=_metadata(book),
            warnings=warnings,
            assets_dir=assets.directory if hrefs else None,
            assets=hrefs,
        )
