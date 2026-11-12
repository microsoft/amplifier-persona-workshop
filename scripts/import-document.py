#!/usr/bin/env python3
"""Bounded local document conversion. Stdout is normalized, attributed JSON only."""
import base64
import email
from email import policy
from email.utils import parseaddr
from html.parser import HTMLParser
import io
import json
import pathlib
import re
import subprocess
import sys
import tempfile
import xml.etree.ElementTree as ET
import zipfile

LIMIT = 2 * 1024 * 1024


class TextHTML(HTMLParser):
    def __init__(self):
        super().__init__()
        self.parts, self.hidden, self.quote = [], 0, 0

    def handle_starttag(self, tag, attrs):
        if tag in ("script", "style", "head"):
            self.hidden += 1
        if tag == "blockquote":
            self.quote += 1
        if tag in ("p", "div", "br", "li", "h1", "h2", "h3", "tr"):
            self.parts.append("\n")

    def handle_endtag(self, tag):
        if tag in ("script", "style", "head"):
            self.hidden = max(0, self.hidden - 1)
        if tag == "blockquote":
            self.quote = max(0, self.quote - 1)
        if tag in ("p", "div", "li", "tr"):
            self.parts.append("\n")

    def handle_data(self, data):
        if not self.hidden and not self.quote:
            self.parts.append(data)


def html(text):
    p = TextHTML()
    p.feed(text)
    return "".join(p.parts)


def zip_text(data, member, paragraph, text_tag):
    with zipfile.ZipFile(io.BytesIO(data)) as archive:
        info = archive.getinfo(member)
        if info.file_size > 8 * LIMIT:
            raise ValueError("Document text is too large")
        root = ET.fromstring(archive.read(info))
        chunks = []
        for p in root.iter():
            if p.tag in paragraph:
                chunks.append("".join(p.itertext()) if text_tag is None else "".join(t.text or "" for t in p.iter(text_tag)))
        return "\n\n".join(chunks)


def pypdf_text(data):
    # Native packages use the pinned pure-Python reader. It is not a Poppler
    # compatibility promise: layout and font extraction require human review.
    from pypdf import PdfReader, apply_configuration
    with apply_configuration(maximum_declared_stream_length=16*LIMIT,
            array_based_stream_maximum_output_length=16*LIMIT,
            zlib_maximum_output_length=16*LIMIT, lzw_maximum_output_length=16*LIMIT,
            run_length_maximum_output_length=16*LIMIT, stream_decoding_work_maximum_length=32*LIMIT,
            page_tree_maximum_entries=2000, page_tree_maximum_depth=40,
            stream_parsing_work_maximum_count=500_000, xform_maximum_invocations_per_extraction=500,
            jbig2dec_binary=None):
        reader = PdfReader(io.BytesIO(data), strict=True)
        if reader.is_encrypted:
            raise ValueError("Encrypted PDFs are not supported; export a readable copy first")
        if len(reader.pages) > 1000:
            raise ValueError("Split this PDF into smaller documents")
        chunks, size = [], 0
        for page in reader.pages:
            text = page.extract_text(extraction_mode="layout")
            size += len(text.encode())
            if size > LIMIT:
                raise ValueError("Extracted PDF text exceeds 2 MB")
            chunks.append(text)
        return "\n\n".join(chunks)


def convert(req):
    data = base64.b64decode(req["content_base64"], validate=True)
    ext = pathlib.Path(req["filename"]).suffix.lower()
    author = req.get("author") or "Unattributed"
    if ext == ".eml":
        msg = email.message_from_bytes(data, policy=policy.default)
        sender, address = parseaddr(str(msg.get("From", "")))
        author = sender or address or "Unattributed"
        part = msg.get_body(preferencelist=("plain", "html"))
        if part is None:
            raise ValueError("This email has no readable body")
        text = part.get_content()
        if part.get_content_type() == "text/html":
            text = html(text)
        kind = "email"
    elif ext in (".html", ".htm"):
        text, kind = html(data.decode("utf-8-sig")), "document"
    elif ext == ".docx":
        text = zip_text(data, "word/document.xml", {"{http://schemas.openxmlformats.org/wordprocessingml/2006/main}p"}, "{http://schemas.openxmlformats.org/wordprocessingml/2006/main}t")
        kind = "document"
    elif ext == ".odt":
        text = zip_text(data, "content.xml", {"{urn:oasis:names:tc:opendocument:xmlns:text:1.0}p", "{urn:oasis:names:tc:opendocument:xmlns:text:1.0}h"}, None)
        kind = "document"
    elif ext == ".pdf":
        if req.get("pdf_backend", "pdftotext") == "pypdf":
            text = pypdf_text(data)
        elif req.get("pdf_backend", "pdftotext") == "pdftotext":
            with tempfile.TemporaryDirectory(prefix="persona-pdf-") as folder:
                source, target = pathlib.Path(folder) / "input.pdf", pathlib.Path(folder) / "output.txt"
                source.write_bytes(data)
                subprocess.run(["pdftotext", "-layout", "-enc", "UTF-8", str(source), str(target)], check=True, timeout=25, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
                if target.stat().st_size > LIMIT:
                    raise ValueError("Extracted PDF text exceeds 2 MB")
                text = target.read_text()
        else:
            raise ValueError("Unsupported configured PDF backend")
        kind = "document"
    else:
        raise ValueError("Unsupported document format")
    if not text.strip() or len(text.encode()) > LIMIT:
        raise ValueError("No readable text, or extracted text exceeds 2 MB; scanned PDFs need OCR first")
    messages = []
    quoted = False
    for para in re.split(r"\n\s*\n", text.strip()):
        # An email's quoted history belongs to earlier speakers, not its sender.
        if kind == "email" and re.search(r"(?im)^\s*(?:On .+ wrote:|[- ]*(?:Original Message|Forwarded message)[- ]*|From:)", para):
            quoted = True
        for chunk in re.split(r"(?m)(?=^\s*>)", para):
            if chunk.strip():
                messages.append({"author": author, "role": "author", "text": chunk.strip(), "quoted": quoted or chunk.lstrip().startswith(">")})
    if len(messages) > 5000:
        raise ValueError("Split this document into smaller sources")
    return {"kind": kind, "messages": messages, "notes": "Select the subject's actual speaker label before using this source. Email quoted history is excluded; document authorship is supplied by the importer."}


if __name__ == "__main__":
    try:
        result = convert(json.load(sys.stdin))
        output = json.dumps(result, ensure_ascii=False)
        if len(output.encode()) > 3 * LIMIT:
            raise ValueError("Normalized output is too large")
        print(output)
    except Exception:
        print(json.dumps({"error": "Document could not be converted. Check its format, size, and local converter availability. PDF files need readable text; encrypted or scanned PDFs are not supported by the bundled reader."}))
        sys.exit(1)
