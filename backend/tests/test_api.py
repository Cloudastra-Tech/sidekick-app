"""End-to-end checks against a throwaway SQLite database: log in, read and change settings, and
push a call twice to prove a retry doesn't duplicate it."""

import os
from collections.abc import AsyncIterator, Iterator
from pathlib import Path

import httpx
import pytest
from fastapi.testclient import TestClient
from sqlmodel import Session, SQLModel, create_engine

os.environ.setdefault("JWT_SECRET", "test-secret")


@pytest.fixture
def client(tmp_path: Path) -> Iterator[TestClient]:
    os.environ["DATABASE_URL"] = f"sqlite:///{tmp_path.as_posix()}/test.db"

    from app import db as db_module
    from app.config import settings

    settings.cache_clear()
    engine = create_engine(os.environ["DATABASE_URL"])
    db_module.engine = engine
    SQLModel.metadata.create_all(engine)

    def session_override():
        with Session(engine) as db:
            yield db

    from app.db import session
    from app.main import app
    from app.seed import seed

    # The seed command writes through the module-level engine, which the line above replaced.
    seed("Test Workspace", "rep@example.com", "hunter2hunter2", "Test Rep")

    app.dependency_overrides[session] = session_override
    with TestClient(app) as test_client:
        yield test_client
    app.dependency_overrides.clear()
    # Windows won't delete the file while SQLite still holds it.
    engine.dispose()


def token(client: TestClient) -> str:
    response = client.post(
        "/auth/login", json={"email": "rep@example.com", "password": "hunter2hunter2"}
    )
    assert response.status_code == 200, response.text
    return response.json()["access_token"]


def auth(client: TestClient) -> dict[str, str]:
    return {"Authorization": f"Bearer {token(client)}"}


def test_health_needs_no_login(client: TestClient) -> None:
    assert client.get("/health").json() == {"status": "ok"}


def test_wrong_password_is_refused(client: TestClient) -> None:
    response = client.post("/auth/login", json={"email": "rep@example.com", "password": "nope"})
    assert response.status_code == 401


def test_settings_need_a_token(client: TestClient) -> None:
    assert client.get("/settings").status_code == 401


def test_settings_come_back_with_defaults_and_can_be_changed(client: TestClient) -> None:
    headers = auth(client)
    defaults = client.get("/settings", headers=headers).json()
    assert defaults["language"] == "en"
    assert defaults["max_call_cost_usd"] == 5
    assert defaults["playbook_version"] == 0

    playbook = "Acme sells AI engineering. " * 20
    updated = client.put(
        "/settings", headers=headers, json={"language": "en-IN", "playbook": playbook}
    ).json()
    assert updated["language"] == "en-IN"
    assert updated["playbook_version"] == 1
    assert updated["playbook"].startswith("Acme sells")

    # A second identical save doesn't create another version.
    again = client.put("/settings", headers=headers, json={"playbook": playbook}).json()
    assert again["playbook_version"] == 1


def test_a_call_sent_twice_stays_one_call(client: TestClient) -> None:
    headers = auth(client)
    payload = {
        "call_id": "call-1789722575903",
        "prospect": "Priya",
        "company": "Acme Robotics",
        "duration_minutes": 12.4,
        "cost_usd": 0.21,
        "summary": {"overview": "They want lower AI spend", "sentiment": "positive"},
        "metrics": {"latency_ms": {"keyword_turn_end_p90": 2300}},
        "consent_events": [{"event": "acknowledged", "acknowledged_by": "Alex"}],
    }
    first = client.post("/calls", headers=headers, json=payload)
    assert first.status_code == 200, first.text
    assert first.json()["prospect"] == "Priya"

    payload["duration_minutes"] = 18.9
    second = client.post("/calls", headers=headers, json=payload)
    assert second.status_code == 200
    assert second.json()["id"] == first.json()["id"]
    assert second.json()["duration_minutes"] == 18.9

    listed = client.get("/calls", headers=headers).json()
    assert len(listed) == 1
    assert listed[0]["user_email"] == "rep@example.com"
    assert listed[0]["summary"]["sentiment"] == "positive"


def test_one_workspace_cannot_read_another(client: TestClient) -> None:
    from app.seed import seed

    seed("Other Co", "other@example.com", "hunter2hunter2", "Other Rep")
    client.post(
        "/calls",
        headers=auth(client),
        json={"call_id": "call-mine", "prospect": "Priya"},
    )
    other = client.post(
        "/auth/login", json={"email": "other@example.com", "password": "hunter2hunter2"}
    ).json()["access_token"]
    listed = client.get("/calls", headers={"Authorization": f"Bearer {other}"}).json()
    assert listed == []


def test_admin_adds_a_member_who_can_then_sign_in(client: TestClient) -> None:
    headers = auth(client)
    created = client.post(
        "/users", headers=headers, json={"email": "Rep2@Example.com", "name": "Second Rep"}
    )
    assert created.status_code == 201, created.text
    body = created.json()
    assert body["member"]["email"] == "rep2@example.com"
    assert body["member"]["is_admin"] is False
    password = body["temporary_password"]

    signed_in = client.post("/auth/login", json={"email": "rep2@example.com", "password": password})
    assert signed_in.status_code == 200

    members = client.get("/users", headers=headers).json()
    assert {m["email"] for m in members} == {"rep@example.com", "rep2@example.com"}


def test_the_same_email_is_refused(client: TestClient) -> None:
    headers = auth(client)
    client.post("/users", headers=headers, json={"email": "rep2@example.com"})
    again = client.post("/users", headers=headers, json={"email": "rep2@example.com"})
    assert again.status_code == 409


def test_a_rep_cannot_manage_members(client: TestClient) -> None:
    headers = auth(client)
    password = client.post("/users", headers=headers, json={"email": "rep2@example.com"}).json()[
        "temporary_password"
    ]
    rep_token = client.post(
        "/auth/login", json={"email": "rep2@example.com", "password": password}
    ).json()["access_token"]
    rep = {"Authorization": f"Bearer {rep_token}"}
    assert client.get("/users", headers=rep).status_code == 403
    assert (
        client.post("/users", headers=rep, json={"email": "third@example.com"}).status_code == 403
    )
    assert client.put("/settings", headers=rep, json={"language": "hi"}).status_code == 403


def test_a_deactivated_member_cannot_sign_in(client: TestClient) -> None:
    headers = auth(client)
    created = client.post("/users", headers=headers, json={"email": "rep2@example.com"}).json()
    member_id, password = created["member"]["id"], created["temporary_password"]

    off = client.patch(f"/users/{member_id}", headers=headers, json={"is_active": False})
    assert off.status_code == 200 and off.json()["is_active"] is False
    assert (
        client.post(
            "/auth/login", json={"email": "rep2@example.com", "password": password}
        ).status_code
        == 401
    )

    client.patch(f"/users/{member_id}", headers=headers, json={"is_active": True})
    assert (
        client.post(
            "/auth/login", json={"email": "rep2@example.com", "password": password}
        ).status_code
        == 200
    )


def test_an_admin_cannot_lock_themselves_out(client: TestClient) -> None:
    headers = auth(client)
    me = client.get("/me", headers=headers).json()
    assert (
        client.patch(f"/users/{me['id']}", headers=headers, json={"is_active": False}).status_code
        == 400
    )
    assert client.delete(f"/users/{me['id']}", headers=headers).status_code == 400


def test_a_member_with_calls_is_deactivated_not_removed(client: TestClient) -> None:
    headers = auth(client)
    created = client.post("/users", headers=headers, json={"email": "rep2@example.com"}).json()
    member_id, password = created["member"]["id"], created["temporary_password"]
    rep_token = client.post(
        "/auth/login", json={"email": "rep2@example.com", "password": password}
    ).json()["access_token"]

    # With no calls, the member can be removed outright.
    fresh = client.post("/users", headers=headers, json={"email": "third@example.com"}).json()
    assert client.delete(f"/users/{fresh['member']['id']}", headers=headers).status_code == 204

    client.post(
        "/calls",
        headers={"Authorization": f"Bearer {rep_token}"},
        json={"call_id": "call-rep2", "prospect": "Priya"},
    )
    refused = client.delete(f"/users/{member_id}", headers=headers)
    assert refused.status_code == 409
    assert "deactivate" in refused.json()["detail"]


def test_password_reset_gives_a_new_one_time_password(client: TestClient) -> None:
    headers = auth(client)
    created = client.post("/users", headers=headers, json={"email": "rep2@example.com"}).json()
    member_id, old = created["member"]["id"], created["temporary_password"]
    new = client.post(f"/users/{member_id}/password", headers=headers).json()["temporary_password"]
    assert new != old
    assert (
        client.post("/auth/login", json={"email": "rep2@example.com", "password": old}).status_code
        == 401
    )
    assert (
        client.post("/auth/login", json={"email": "rep2@example.com", "password": new}).status_code
        == 200
    )


def test_a_member_can_be_promoted_and_demoted(client: TestClient) -> None:
    headers = auth(client)
    created = client.post("/users", headers=headers, json={"email": "rep2@example.com"}).json()
    member_id, password = created["member"]["id"], created["temporary_password"]
    rep = {
        "Authorization": "Bearer "
        + client.post(
            "/auth/login", json={"email": "rep2@example.com", "password": password}
        ).json()["access_token"]
    }
    assert client.get("/users", headers=rep).status_code == 403

    promoted = client.patch(f"/users/{member_id}", headers=headers, json={"is_admin": True})
    assert promoted.status_code == 200 and promoted.json()["is_admin"] is True
    # The role is read per request, so the token they already hold now works.
    assert client.get("/users", headers=rep).status_code == 200

    demoted = client.patch(f"/users/{member_id}", headers=headers, json={"is_admin": False})
    assert demoted.status_code == 200 and demoted.json()["is_admin"] is False
    assert client.get("/users", headers=rep).status_code == 403


def test_an_admin_cannot_demote_themselves(client: TestClient) -> None:
    headers = auth(client)
    me = client.get("/me", headers=headers).json()
    refused = client.patch(f"/users/{me['id']}", headers=headers, json={"is_admin": False})
    assert refused.status_code == 400


def test_previous_calls_find_the_prospect_by_crm_id_or_name(client: TestClient) -> None:
    headers = auth(client)
    summary = {"overview": "Asked for an audit proposal", "next_step": "Send proposal"}
    calls = [
        # Before the app sent CRM ids: found by name and company.
        {"call_id": "call-1", "prospect": "Priya", "company": "Acme", "summary": summary},
        {"call_id": "call-2", "crm_person_id": "p-1", "prospect": "Priya S", "summary": summary},
        # No summary, a demo, and someone else: never returned.
        {"call_id": "call-3", "crm_person_id": "p-1", "prospect": "Priya"},
        {"call_id": "call-4", "crm_person_id": "p-1", "mode": "demo", "summary": summary},
        {"call_id": "call-5", "crm_person_id": "p-2", "prospect": "Priya", "summary": summary},
    ]
    for call in calls:
        assert client.post("/calls", headers=headers, json=call).status_code == 200

    found = client.get(
        "/calls/previous",
        headers=headers,
        params={"crm_person_id": "p-1", "prospect": "priya", "company": "ACME"},
    ).json()
    assert [call["call_id"] for call in found] == ["call-2", "call-1"]
    assert found[0]["crm_person_id"] == "p-1"

    typed = client.get(
        "/calls/previous", headers=headers, params={"prospect": "Priya", "company": "Acme"}
    ).json()
    assert [call["call_id"] for call in typed] == ["call-1"]

    # A name alone matches only calls that had no company either, and stray spaces don't matter.
    client.post(
        "/calls",
        headers=headers,
        json={"call_id": "call-6", "prospect": " Priya ", "summary": summary},
    )
    name_only = client.get("/calls/previous", headers=headers, params={"prospect": "priya"}).json()
    assert [call["call_id"] for call in name_only] == ["call-6"]
    assert client.get("/calls/previous", headers=headers, params={"limit": 0}).status_code == 422
    nothing = client.get("/calls/previous", headers=headers).json()
    assert nothing == []


def test_a_column_added_later_is_added_to_an_existing_table(tmp_path: Path) -> None:
    from sqlalchemy import inspect, text

    from app import db as db_module

    engine = create_engine(f"sqlite:///{tmp_path.as_posix()}/old.db")
    with engine.begin() as connection:
        connection.execute(text("CREATE TABLE calls (id VARCHAR PRIMARY KEY)"))
    original, db_module.engine = db_module.engine, engine
    try:
        db_module.add_new_columns()
        db_module.add_new_columns()  # A second start changes nothing.
    finally:
        db_module.engine = original
    assert "crm_person_id" in {c["name"] for c in inspect(engine).get_columns("calls")}
    engine.dispose()


def test_claude_requests_pass_through_unchanged(
    client: TestClient, monkeypatch: pytest.MonkeyPatch
) -> None:
    import httpx

    from app.config import settings
    from app.routers import vendors

    monkeypatch.setenv("ANTHROPIC_API_KEY", "sk-test")
    settings.cache_clear()
    seen: dict[str, bytes] = {}

    async def fake_post(url: str, headers: dict[str, str], content: bytes) -> httpx.Response:
        seen["body"] = content
        return httpx.Response(200, content=b'{"content": [], "usage": {"input_tokens": 3}}')

    monkeypatch.setattr(vendors._anthropic, "post", fake_post)
    body = b'{"model": "claude-haiku-4-5", "messages": []}'
    try:
        response = client.post(
            "/llm/messages",
            headers={**auth(client), "content-type": "application/json"},
            content=body,
        )
        assert response.status_code == 200, response.text
        assert response.json() == {"content": [], "usage": {"input_tokens": 3}}
        assert seen["body"] == body
    finally:
        settings.cache_clear()


def test_a_dropped_claude_connection_is_retried_once_then_reported(
    client: TestClient, monkeypatch: pytest.MonkeyPatch
) -> None:
    import httpx

    from app.config import settings
    from app.routers import vendors

    monkeypatch.setenv("ANTHROPIC_API_KEY", "sk-test")
    settings.cache_clear()
    attempts: list[int] = []

    async def stale_then_ok(url: str, headers: dict[str, str], content: bytes) -> httpx.Response:
        attempts.append(1)
        if len(attempts) == 1:
            raise httpx.RemoteProtocolError("Server disconnected without sending a response")
        return httpx.Response(200, content=b"{}")

    async def unreachable(url: str, headers: dict[str, str], content: bytes) -> httpx.Response:
        raise httpx.ConnectError("no route to host")

    headers = {**auth(client), "content-type": "application/json"}
    try:
        monkeypatch.setattr(vendors._anthropic, "post", stale_then_ok)
        assert client.post("/llm/messages", headers=headers, content=b"{}").status_code == 200
        assert len(attempts) == 2

        monkeypatch.setattr(vendors._anthropic, "post", unreachable)
        response = client.post("/llm/messages", headers=headers, content=b"{}")
        assert response.status_code == 502
        assert "Couldn't reach Claude" in response.json()["detail"]
    finally:
        settings.cache_clear()


def test_each_os_is_offered_only_its_own_newest_build(
    client: TestClient, tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    from app.config import settings

    monkeypatch.setenv("UPDATES_DIR", str(tmp_path / "updates"))
    settings.cache_clear()
    headers = auth(client)

    def publish(version: str, target: str) -> None:
        response = client.post(
            "/updates",
            headers=headers,
            data={"version": version, "signature": f"sig-{version}-{target}", "target": target},
            files={"installer": (f"Sidekick-{version}.bin", b"build")},
        )
        assert response.status_code == 201, response.text

    publish("0.3.1", "darwin-aarch64")
    publish("0.3.1", "darwin-x86_64")
    publish("0.3.2", "windows-x86_64")

    mac = client.get("/updates/latest?target=darwin", headers=headers).json()
    assert mac["version"] == "0.3.1"
    assert set(mac["platforms"]) == {"darwin-aarch64", "darwin-x86_64"}

    windows = client.get("/updates/latest?target=windows", headers=headers).json()
    assert windows["version"] == "0.3.2"
    assert set(windows["platforms"]) == {"windows-x86_64"}

    assert client.get("/updates/latest?target=linux", headers=headers).status_code == 204
    # Older apps that send no target still get the newest build of all.
    assert client.get("/updates/latest", headers=headers).json()["version"] == "0.3.2"

    download = client.get("/updates/download/0.3.1?target=darwin-aarch64", headers=headers)
    assert download.status_code == 200
    assert download.content == b"build"


def _streaming_upstream(chunks: list[bytes], status: int = 200) -> httpx.Response:
    """A fake Anthropic answer whose body arrives in pieces, like a real event stream."""

    async def body() -> AsyncIterator[bytes]:
        for chunk in chunks:
            yield chunk

    return httpx.Response(status, headers={"content-type": "text/event-stream"}, content=body())


SSE_CHUNKS = [
    b'event: message_start\ndata: {"type":"message_start"}\n\n',
    b'event: content_block_delta\ndata: {"type":"content_block_delta",',
    b'"delta":{"type":"text_delta","text":"Ask about"}}\n\n',
    b'event: message_stop\ndata: {"type":"message_stop"}\n\n',
]


def test_claude_streams_pass_through_as_they_arrive(
    client: TestClient, monkeypatch: pytest.MonkeyPatch
) -> None:
    from app.config import settings
    from app.routers import vendors

    monkeypatch.setenv("ANTHROPIC_API_KEY", "sk-test")
    settings.cache_clear()
    seen: dict[str, bytes] = {}

    async def fake_send(request: httpx.Request, stream: bool = False) -> httpx.Response:
        seen["body"] = request.content
        seen["stream"] = stream
        return _streaming_upstream(SSE_CHUNKS)

    monkeypatch.setattr(vendors._anthropic, "send", fake_send)
    body = b'{"model": "claude-haiku-4-5", "stream": true, "messages": []}'
    try:
        response = client.post(
            "/llm/messages/stream",
            headers={**auth(client), "content-type": "application/json"},
            content=body,
        )
        assert response.status_code == 200, response.text
        assert response.headers["content-type"].startswith("text/event-stream")
        assert response.headers["x-accel-buffering"] == "no"
        assert response.content == b"".join(SSE_CHUNKS)
        assert seen == {"body": body, "stream": True}
    finally:
        settings.cache_clear()


def test_a_compressed_claude_stream_is_passed_on_readable(
    client: TestClient, monkeypatch: pytest.MonkeyPatch
) -> None:
    """Anthropic may gzip its answer. The app gets the events themselves, never compressed bytes
    without a header saying so (which it can't read: every streamed hint failed that way)."""
    import gzip

    from app.config import settings
    from app.routers import vendors

    monkeypatch.setenv("ANTHROPIC_API_KEY", "sk-test")
    settings.cache_clear()
    seen: dict[str, str] = {}
    compressed = gzip.compress(b"".join(SSE_CHUNKS))

    async def fake_send(request: httpx.Request, stream: bool = False) -> httpx.Response:
        seen["accept-encoding"] = request.headers.get("accept-encoding", "")

        async def body() -> AsyncIterator[bytes]:
            for start in range(0, len(compressed), 7):
                yield compressed[start : start + 7]

        return httpx.Response(
            200,
            headers={"content-type": "text/event-stream", "content-encoding": "gzip"},
            content=body(),
        )

    monkeypatch.setattr(vendors._anthropic, "send", fake_send)
    try:
        response = client.post(
            "/llm/messages/stream",
            headers={**auth(client), "content-type": "application/json"},
            content=b'{"stream": true}',
        )
        assert response.status_code == 200, response.text
        assert "content-encoding" not in response.headers
        assert response.content == b"".join(SSE_CHUNKS)
        # Asked uncompressed in the first place, so no event waits on a compressor.
        assert seen["accept-encoding"] == "identity"
    finally:
        settings.cache_clear()


def test_a_claude_stream_respects_the_spend_limit_and_errors(
    client: TestClient, monkeypatch: pytest.MonkeyPatch
) -> None:
    import httpx
    from sqlmodel import Session, select

    from app import db as db_module
    from app.config import settings
    from app.models import User
    from app.routers import vendors

    monkeypatch.setenv("ANTHROPIC_API_KEY", "sk-test")
    settings.cache_clear()
    headers = {**auth(client), "content-type": "application/json"}
    calls: list[int] = []

    async def overloaded(request: httpx.Request, stream: bool = False) -> httpx.Response:
        calls.append(1)
        return _streaming_upstream([b'{"error":{"message":"Overloaded"}}'], status=529)

    async def stale_then_ok(request: httpx.Request, stream: bool = False) -> httpx.Response:
        calls.append(1)
        if len(calls) == 1:
            raise httpx.RemoteProtocolError("Server disconnected without sending a response")
        return _streaming_upstream(SSE_CHUNKS)

    try:
        monkeypatch.setattr(vendors._anthropic, "send", overloaded)
        response = client.post("/llm/messages/stream", headers=headers, content=b"{}")
        assert response.status_code == 529
        assert "Overloaded" in response.json()["detail"]

        calls.clear()
        monkeypatch.setattr(vendors._anthropic, "send", stale_then_ok)
        response = client.post("/llm/messages/stream", headers=headers, content=b"{}")
        assert response.status_code == 200
        assert len(calls) == 2

        with Session(db_module.engine) as db:
            user = db.exec(select(User).where(User.email == "rep@example.com")).one()
            user.monthly_spend_limit_usd = 0.0
            db.add(user)
            db.commit()
        calls.clear()
        response = client.post("/llm/messages/stream", headers=headers, content=b"{}")
        assert response.status_code == 402
        assert calls == [], "Claude isn't asked once the limit is reached"
    finally:
        settings.cache_clear()


def test_crm_people_are_served_from_memory_and_slimmed(
    client: TestClient, monkeypatch: pytest.MonkeyPatch
) -> None:
    from app.config import settings
    from app.routers import crm

    monkeypatch.setenv("TWENTY_API_KEY", "key")
    monkeypatch.setenv("TWENTY_BASE_URL", "https://crm.example.com")
    settings.cache_clear()
    monkeypatch.setattr(crm, "_people", None)
    reads: list[str] = []

    async def fake_all(collection: str) -> list[dict]:
        reads.append(collection)
        if collection == "companyCustoms":
            return [{"id": "c1", "name": "Northwind"}]
        return [
            {
                "id": "p1",
                "name": "Arseniy",
                "workEmail": "a@t.io",
                "companyId": "c1",
                "title": "CTO",
            },
            {"id": "p2", "name": "Kshetry & co", "companyId": "deleted"},
            {"id": "p3", "name": "", "workEmail": ""},  # Nothing to search by: left out.
        ]

    monkeypatch.setattr(crm, "_all", fake_all)
    headers = auth(client)

    first = client.get("/crm/people", headers=headers)
    assert first.status_code == 200, first.text
    people = {p["id"]: p for p in first.json()}
    assert set(people) == {"p1", "p2"}
    assert people["p1"]["company_name"] == "Northwind"
    assert people["p1"]["job_title"] == "CTO"
    assert people["p2"]["company_name"] == ""

    client.get("/crm/people", headers=headers)
    assert sorted(reads) == ["companyCustoms", "personsCustoms"], "the second read is from memory"
    settings.cache_clear()


def test_crm_people_need_a_login(client: TestClient) -> None:
    assert client.get("/crm/people").status_code == 401


def test_a_calls_logs_are_kept_and_read_back(client: TestClient) -> None:
    headers = auth(client)
    events = [
        {"kind": "opening_hints", "at_unix_ms": 1},
        {"kind": "llm_call", "reason": {"kind": "turn_end"}, "shown": False},
    ]
    consent = [
        {"event": "consent_acknowledged", "acknowledged_by": "Priya", "at": "2026-10-22T10:02:08"},
        {"event": "capture_started", "audio_source": "Chrome only", "at": "2026-10-22T10:02:09"},
    ]
    payload = {"call_id": "session-1", "events": events, "consent_events": consent}
    assert client.post("/calls", headers=headers, json=payload).status_code == 200
    # A retry sends the same consent events again; they are kept once.
    assert client.post("/calls", headers=headers, json=payload).status_code == 200

    logs = client.get("/calls/session-1/logs", headers=headers).json()
    assert [e["kind"] for e in logs["events"]] == ["opening_hints", "llm_call"]
    assert [e["event"] for e in logs["consent_events"]] == [
        "consent_acknowledged",
        "capture_started",
    ]
    assert logs["consent_events"][1]["audio_source"] == "Chrome only"
    assert "events" not in client.get("/calls", headers=headers).json()[0], "the list stays light"
    assert client.get("/calls/nope/logs", headers=headers).status_code == 404
