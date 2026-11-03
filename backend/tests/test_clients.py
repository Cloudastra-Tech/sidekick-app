"""Files and notes about the client companies a team meets: text pulled out of each type, the size
limits, and that one workspace never sees another's."""

import io

import pytest
from docx import Document
from fastapi.testclient import TestClient

from app.extract import UnreadableFile, extract_text
from tests.test_api import auth, client  # noqa: F401  (client is a fixture)

FINSTACK = {"client_key": "crm:c-1", "client_name": "FinStack"}


def pdf_with(text: str) -> bytes:
    """A one-page PDF showing `text`, built by hand so the test needs no PDF writer."""
    stream = f"BT /F1 12 Tf 72 720 Td ({text}) Tj ET".encode()
    objects = [
        b"<< /Type /Catalog /Pages 2 0 R >>",
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        (
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] "
            b"/Resources << /Font << /F1 5 0 R >> >> /Contents 4 0 R >>"
        ),
        b"<< /Length %d >>\nstream\n" % len(stream) + stream + b"\nendstream",
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>",
    ]
    out = io.BytesIO()
    out.write(b"%PDF-1.4\n")
    offsets = []
    for number, body in enumerate(objects, start=1):
        offsets.append(out.tell())
        out.write(b"%d 0 obj\n" % number + body + b"\nendobj\n")
    xref = out.tell()
    out.write(b"xref\n0 %d\n0000000000 65535 f \n" % (len(objects) + 1))
    for offset in offsets:
        out.write(b"%010d 00000 n \n" % offset)
    out.write(
        b"trailer\n<< /Size %d /Root 1 0 R >>\nstartxref\n%d\n%%%%EOF\n" % (len(objects) + 1, xref)
    )
    return out.getvalue()


def docx_with(paragraph: str, cell: str) -> bytes:
    document = Document()
    document.add_paragraph(paragraph)
    document.add_table(rows=1, cols=1).cell(0, 0).text = cell
    out = io.BytesIO()
    document.save(out)
    return out.getvalue()


# --- Reading each file type ---


def test_a_pdf_gives_its_text() -> None:
    assert "Audit costs 4200 USD" in extract_text("pricing.pdf", pdf_with("Audit costs 4200 USD"))


def test_a_word_file_gives_paragraphs_and_tables() -> None:
    text = extract_text("rfp.docx", docx_with("They need SOC 2.", "Go-live: March"))
    assert "They need SOC 2." in text
    assert "Go-live: March" in text


def test_html_gives_the_words_without_tags_scripts_or_styles() -> None:
    html = (
        b"<html><head><style>h1{color:red}</style><script>track()</script></head>"
        b"<body><h1>Loan dashboard</h1><p>Shows &amp; exports <b>EMI</b> plans</p></body></html>"
    )
    text = extract_text("prototype.html", html)
    assert "Loan dashboard" in text
    assert "Shows & exports EMI plans" in text
    assert "track()" not in text and "color" not in text


def test_text_and_markdown_are_read_as_they_are() -> None:
    assert extract_text("notes.md", b"# Goals\n\nCut cost by 40%") == "# Goals\n\nCut cost by 40%"
    # Not UTF-8 (an old Windows editor): still readable.
    assert extract_text("notes.txt", "Café pricing".encode("latin-1")) == "Café pricing"


def test_extra_blank_lines_and_spaces_are_squeezed() -> None:
    assert extract_text("a.txt", b"one   two\n\n\n\n three \n") == "one two\n\nthree"


@pytest.mark.parametrize(
    ("name", "data"),
    [("deck.pptx", b"PK..."), ("scan.pdf", b"not a pdf"), ("rfp.docx", b"not a zip")],
)
def test_unsupported_or_broken_files_are_refused(name: str, data: bytes) -> None:
    with pytest.raises(UnreadableFile):
        extract_text(name, data)


# --- The routes ---


def upload(client: TestClient, headers, name: str, data: bytes, who=FINSTACK):  # noqa: F811
    return client.post(
        "/clients/documents", headers=headers, data=who, files={"file": (name, data)}
    )


def test_files_and_notes_are_kept_per_client_and_served_for_the_call(
    client: TestClient,  # noqa: F811
) -> None:
    headers = auth(client)
    saved = upload(client, headers, "pricing.pdf", pdf_with("Audit costs 4200 USD"))
    assert saved.status_code == 200, saved.text
    assert saved.json()["filename"] == "pricing.pdf"
    assert saved.json()["words"] == 4

    note = client.post(
        "/clients/notes", headers=headers, json={**FINSTACK, "text": "Meera owns the budget."}
    )
    assert note.status_code == 200, note.text
    upload(
        client,
        headers,
        "other.txt",
        b"Acme only fact",
        who={"client_key": "crm:c-2", "client_name": "Acme"},
    )

    listed = client.get("/clients/documents", headers=headers, params={"client_key": "crm:c-1"})
    body = listed.json()
    assert [d["kind"] for d in body["documents"]] == ["note", "file"]  # newest first
    assert body["words"] == 8
    assert body["word_limit"] == 18_000
    assert body["documents"][0]["text"] == "Meera owns the budget.", "a note lists its text"
    assert body["documents"][1]["text"] == "", "a file's text only goes out as call context"

    context = client.get(
        "/clients/context", headers=headers, params={"client_key": "crm:c-1"}
    ).json()["text"]
    assert "Meera owns the budget." in context
    assert "Audit costs 4200 USD" in context
    assert "pricing.pdf" in context
    assert "Acme only fact" not in context


def test_a_note_can_be_changed_and_anything_removed(client: TestClient) -> None:  # noqa: F811
    headers = auth(client)
    note = client.post("/clients/notes", headers=headers, json={**FINSTACK, "text": "Old"}).json()
    changed = client.put(
        f"/clients/notes/{note['id']}", headers=headers, json={"text": "New detail"}
    )
    assert changed.status_code == 200
    assert changed.json()["words"] == 2
    listed = client.get("/clients/documents", headers=headers, params={"client_key": "crm:c-1"})
    assert listed.json()["documents"][0]["text"] == "New detail", "a note comes back to edit"

    assert client.delete(f"/clients/documents/{note['id']}", headers=headers).status_code == 204
    left = client.get("/clients/documents", headers=headers, params={"client_key": "crm:c-1"})
    assert left.json()["documents"] == []


def test_a_client_is_capped_at_the_word_limit(client: TestClient) -> None:  # noqa: F811
    headers = auth(client)
    assert upload(client, headers, "a.txt", b"word " * 17_000).status_code == 200
    too_much = upload(client, headers, "b.txt", b"word " * 1_001)
    assert too_much.status_code == 413
    assert "18,000" in too_much.json()["detail"]
    note = client.post("/clients/notes", headers=headers, json={**FINSTACK, "text": "w " * 1_001})
    assert note.status_code == 413


def test_bad_uploads_are_refused_with_a_reason(client: TestClient) -> None:  # noqa: F811
    headers = auth(client)
    assert upload(client, headers, "deck.pptx", b"PK").status_code == 422
    assert upload(client, headers, "empty.txt", b"  \n ").status_code == 422
    assert upload(client, headers, "big.txt", b"x" * (10 * 1024 * 1024 + 1)).status_code == 413
    no_client = upload(client, headers, "a.txt", b"hi", who={"client_key": " ", "client_name": ""})
    assert no_client.status_code == 422


def test_another_workspace_never_sees_or_changes_a_clients_files(
    client: TestClient,  # noqa: F811
) -> None:
    from app.seed import seed

    headers = auth(client)
    note = client.post("/clients/notes", headers=headers, json={**FINSTACK, "text": "Secret"})
    seed("Other Co", "other@example.com", "hunter2hunter2", "Other Rep")
    login = client.post(
        "/auth/login", json={"email": "other@example.com", "password": "hunter2hunter2"}
    )
    other = {"Authorization": f"Bearer {login.json()['access_token']}"}

    params = {"client_key": "crm:c-1"}
    assert client.get("/clients/documents", headers=other, params=params).json()["documents"] == []
    assert client.get("/clients/context", headers=other, params=params).json()["text"] == ""
    note_id = note.json()["id"]
    assert client.delete(f"/clients/documents/{note_id}", headers=other).status_code == 404
    assert (
        client.put(f"/clients/notes/{note_id}", headers=other, json={"text": "x"}).status_code
        == 404
    )


def test_client_files_need_a_login(client: TestClient) -> None:  # noqa: F811
    assert client.get("/clients/documents", params={"client_key": "crm:c-1"}).status_code == 401


# --- Earlier calls with anyone at the same company ---


def test_previous_calls_also_find_colleagues_at_the_same_company(
    client: TestClient,  # noqa: F811
) -> None:
    headers = auth(client)
    summary = {"overview": "Discussed audit"}
    calls = [
        {"call_id": "rahul", "crm_person_id": "p-1", "crm_company_id": "c-1", "summary": summary},
        {"call_id": "meera", "crm_person_id": "p-2", "crm_company_id": "c-1", "summary": summary},
        {"call_id": "acme", "crm_person_id": "p-3", "crm_company_id": "c-2", "summary": summary},
    ]
    for call in calls:
        assert client.post("/calls", headers=headers, json=call).status_code == 200

    found = client.get(
        "/calls/previous",
        headers=headers,
        params={"crm_person_id": "p-2", "crm_company_id": "c-1"},
    ).json()
    assert [call["call_id"] for call in found] == ["meera", "rahul"]
    assert found[0]["crm_company_id"] == "c-1"

    # An app that doesn't send the company still gets only that person.
    only_person = client.get(
        "/calls/previous", headers=headers, params={"crm_person_id": "p-2"}
    ).json()
    assert [call["call_id"] for call in only_person] == ["meera"]
