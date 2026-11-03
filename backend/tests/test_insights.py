"""Who can read which calls, transcripts kept for admins, and each rep's numbers."""

from uuid import uuid4

from fastapi.testclient import TestClient

from app.routers.insights import conversation
from tests.test_api import auth, client  # noqa: F401  (the fixture)

TRANSCRIPT = [
    {"channel": "me", "text": "Hi Priya, thanks for joining. What made you look at this now?"},
    {"channel": "them", "text": "Our AI bill tripled and nobody knows why."},
    {"channel": "me", "text": "That's common. Which models do you run, and who owns the budget?"},
    {"channel": "me", "text": "We usually start with a two-week audit."},
    {"channel": "them", "text": "Meera owns it."},
]


def call(call_id: str, **extra) -> dict:
    return {
        "call_id": call_id,
        "mode": "live",
        "duration_minutes": 12,
        "cost_usd": 0.2,
        "summary": {"sentiment": "positive"},
        "metrics": {
            "stt_minutes": {"me": 6, "them": 5},
            "hints_shown": 4,
            "latency_ms": {"speech_to_screen_p90": 2100},
        },
        "transcript": TRANSCRIPT,
        **extra,
    }


def member(client: TestClient, admin_headers: dict, email: str) -> dict:  # noqa: F811
    made = client.post(
        "/users", headers=admin_headers, json={"email": email, "name": "Priya", "is_admin": False}
    )
    assert made.status_code == 201, made.text
    login = client.post(
        "/auth/login", json={"email": email, "password": made.json()["temporary_password"]}
    )
    return {"Authorization": f"Bearer {login.json()['access_token']}"}


def test_hint_speed_is_a_p90_over_hints_not_a_median_of_calls() -> None:
    from app.models import Call
    from app.routers.insights import hint_latency, p90

    assert p90([1.0] * 6 + [9.0] * 4) == 9.0
    shown = [
        {"kind": "llm_call", "shown": True, "latency_ms": {"speech_to_hint": ms}}
        for ms in [1000] * 6 + [9000] * 4
    ]
    unshown = {"kind": "llm_call", "shown": False, "latency_ms": {"speech_to_hint": 99_999}}
    logged = Call(workspace_id=uuid4(), user_id=uuid4(), call_id="a", events=[*shown, unshown])
    assert hint_latency([logged]) == (9000.0, "hints")

    # Without logs: the slowest call's p90, and a real 0 isn't skipped for the fallback field.
    def per_call(latency: dict) -> Call:
        return Call(
            workspace_id=uuid4(), user_id=uuid4(), call_id="b", metrics={"latency_ms": latency}
        )

    calls = [
        per_call({"speech_to_screen_p90": 1000}),
        per_call({"speech_to_screen_p90": 0, "speech_to_hint_p90": 7000}),
        per_call({"speech_to_screen_p90": True}),
    ]
    assert hint_latency(calls) == (1000.0, "slowest call")


def test_conversation_measures_come_from_the_words() -> None:
    measured = conversation(TRANSCRIPT)
    assert measured is not None
    assert 0.6 < measured["talk_ratio"] < 0.8
    assert measured["questions"] == 2
    assert measured["switches"] == 3
    # The two rep lines in a row make one monologue.
    assert measured["longest_monologue_words"] == len(
        [
            "That's",
            "common.",
            "Which",
            "models",
            "do",
            "you",
            "run,",
            "and",
            "who",
            "owns",
            "the",
            "budget?",
            "We",
            "usually",
            "start",
            "with",
            "a",
            "two-week",
            "audit.",
        ]
    )
    assert conversation([]) is None


def test_reps_see_their_own_calls_and_admins_see_everyones(client: TestClient) -> None:  # noqa: F811
    admin_headers = auth(client)
    rep_headers = member(client, admin_headers, "priya@example.com")
    assert client.post("/calls", headers=admin_headers, json=call("call-admin")).status_code == 200
    assert client.post("/calls", headers=rep_headers, json=call("call-rep")).status_code == 200

    assert {c["call_id"] for c in client.get("/calls", headers=admin_headers).json()} == {
        "call-admin",
        "call-rep",
    }
    assert [c["call_id"] for c in client.get("/calls", headers=rep_headers).json()] == ["call-rep"]

    # Another rep's call reads as missing, transcript and logs included.
    for path in ("/calls/call-admin", "/calls/call-admin/transcript", "/calls/call-admin/logs"):
        assert client.get(path, headers=rep_headers).status_code == 404, path
    # The probe the app uses to see whether logs are kept still answers "no such call".
    assert "No such call" in client.get("/calls/-/logs", headers=rep_headers).json()["detail"]

    # Nor can a rep overwrite it by sending a call under its id.
    hijack = client.post(
        "/calls", headers=rep_headers, json={"call_id": "call-admin", "prospect": "x"}
    )
    assert hijack.status_code == 404
    admin_call = client.get("/calls/call-admin", headers=admin_headers).json()
    assert admin_call["prospect"] == "", "the admin's call is untouched"

    transcript = client.get("/calls/call-rep/transcript", headers=admin_headers).json()
    assert transcript["transcript"][1]["text"] == "Our AI bill tripled and nobody knows why."
    assert "transcript" not in client.get("/calls", headers=admin_headers).json()[0]


def test_admins_get_each_reps_numbers(client: TestClient) -> None:  # noqa: F811
    admin_headers = auth(client)
    rep_headers = member(client, admin_headers, "priya@example.com")
    client.post("/calls", headers=rep_headers, json=call("call-1"))
    deaf = call("call-2", metrics={"stt_minutes": {"me": 3, "them": 0}, "hints_shown": 0})
    client.post("/calls", headers=rep_headers, json=deaf)
    client.post("/calls", headers=rep_headers, json=call("call-3", mode="demo"))  # not counted
    client.post("/calls", headers=rep_headers, json=call("call-4", duration_minutes=0.1))

    assert client.get("/insights/reps", headers=rep_headers).status_code == 403
    rows = client.get("/insights/reps", headers=admin_headers, params={"days": 7}).json()["reps"]
    assert len(rows) == 1
    priya = rows[0]
    assert priya["email"] == "priya@example.com"
    assert priya["calls"] == 2
    assert priya["prospect_heard_rate"] == 0.5
    assert priya["hints_per_call"] == 2
    assert priya["hint_p90_ms"] == 2100
    assert priya["calls_with_transcript"] == 2
    assert priya["questions_per_call"] == 2
    assert priya["sentiment"] == {"positive": 2}
