"""Plain text from the files a team uploads about a client, for the coach to read during calls.

Read once when the file arrives; the call only ever gets the text. Layout, images and styling are
dropped: the coach works from words.
"""

import io
import re
from html.parser import HTMLParser
from pathlib import PurePath
from typing import ClassVar

TEXT_TYPES = {".txt", ".md", ".markdown", ".csv"}
HTML_TYPES = {".html", ".htm"}
SUPPORTED = TEXT_TYPES | HTML_TYPES | {".pdf", ".docx"}


class UnreadableFile(ValueError):
    """The file's type isn't supported, or it couldn't be read."""


def extract_text(filename: str, data: bytes) -> str:
    suffix = PurePath(filename).suffix.lower()
    if suffix not in SUPPORTED:
        raise UnreadableFile("Upload a PDF, Word (.docx), HTML, text or Markdown file")
    try:
        if suffix == ".pdf":
            text = _pdf(data)
        elif suffix == ".docx":
            text = _docx(data)
        elif suffix in HTML_TYPES:
            text = _html(_decode(data))
        else:
            text = _decode(data)
    except UnreadableFile:
        raise
    except Exception as error:  # Each library raises its own errors for a broken file.
        raise UnreadableFile(f"Couldn't read {filename}; is it a valid {suffix} file?") from error
    return clean(text)


def clean(text: str) -> str:
    """Single spaces, at most one blank line in a row, no blank edges."""
    lines = [re.sub(r"[ \t ]+", " ", line).strip() for line in text.splitlines()]
    return re.sub(r"\n{3,}", "\n\n", "\n".join(lines)).strip()


def word_count(text: str) -> int:
    return len(text.split())


def _decode(data: bytes) -> str:
    try:
        return data.decode("utf-8-sig")
    except UnicodeDecodeError:
        # Every byte is valid Latin-1, so an old Windows file still reads.
        return data.decode("latin-1")


def _pdf(data: bytes) -> str:
    from pypdf import PdfReader

    reader = PdfReader(io.BytesIO(data))
    return "\n\n".join(page.extract_text() or "" for page in reader.pages)


def _docx(data: bytes) -> str:
    from docx import Document

    document = Document(io.BytesIO(data))
    parts = [p.text for p in document.paragraphs]
    for table in document.tables:
        for row in table.rows:
            # A merged cell appears once per column it spans; keep it once.
            cells = list(dict.fromkeys(cell.text.strip() for cell in row.cells))
            parts.append(" | ".join(c for c in cells if c))
    return "\n".join(parts)


class _HtmlText(HTMLParser):
    SKIP: ClassVar[set[str]] = {"script", "style", "noscript", "template", "svg", "head"}
    BLOCKS: ClassVar[set[str]] = {
        "p",
        "div",
        "br",
        "li",
        "tr",
        "h1",
        "h2",
        "h3",
        "h4",
        "h5",
        "h6",
        "section",
        "article",
        "header",
        "footer",
        "table",
        "ul",
        "ol",
        "button",
        "label",
    }

    def __init__(self) -> None:
        super().__init__(convert_charrefs=True)
        self.parts: list[str] = []
        self.skipping = 0

    def handle_starttag(self, tag: str, attrs) -> None:
        if tag in self.SKIP:
            self.skipping += 1
        elif tag in self.BLOCKS:
            self.parts.append("\n")

    def handle_endtag(self, tag: str) -> None:
        if tag in self.SKIP:
            self.skipping = max(0, self.skipping - 1)
        elif tag in self.BLOCKS:
            self.parts.append("\n")

    def handle_data(self, data: str) -> None:
        if not self.skipping:
            self.parts.append(data)


def _html(markup: str) -> str:
    parser = _HtmlText()
    parser.feed(markup)
    parser.close()
    return "".join(parser.parts)
