"""Vendor access, so laptops never hold the keys.

Two different shapes, because the vendors differ:

- **Deepgram** can mint short-lived keys, so the app gets one per call and streams audio straight
  to Deepgram. No extra hop in the audio path, and the key dies within the hour.
- **Claude** has no such thing, so hint and summary requests come through here. That is also where
  a user's spend limit can be enforced, which is the point of paying centrally.

The keys live in the server's environment. They are never returned to the app.
"""

from collections.abc import AsyncIterator
from datetime import UTC, datetime, timedelta
from typing import Any

import httpx
from fastapi import APIRouter, Depends, HTTPException, Request, Response
from fastapi.responses import StreamingResponse
from sqlalchemy import func
from sqlmodel import Session, select

from app.auth import current_user
from app.config import settings
from app.db import session
from app.models import Call, User

router = APIRouter(tags=["vendors"])

DEEPGRAM_API = "https://api.deepgram.com"
ANTHROPIC_API = "https://api.anthropic.com"
# Long enough for a call that overruns, short enough that a leaked key is worthless by evening.
STT_KEY_TTL_SECONDS = 4 * 60 * 60
ANTHROPIC_VERSION = "2023-06-01"
TIMEOUT = httpx.Timeout(60.0, connect=10.0)
# One connection pool to Anthropic for the process, kept open between hints. A client per request
# paid for a TLS handshake on every hint; hints arrive seconds to minutes apart, so connections
# must outlive httpx's 5 s default idle limit to be reused.
_anthropic = httpx.AsyncClient(
    timeout=TIMEOUT,
    limits=httpx.Limits(max_keepalive_connections=16, keepalive_expiry=300),
)


# A pooled connection that Anthropic closed while it sat idle fails before the request reaches
# them, so one more try on a fresh connection is safe. Anything else becomes a gateway error.
_STALE_CONNECTION = (httpx.RemoteProtocolError, httpx.ConnectError, httpx.ConnectTimeout)


def _spent_this_month(user: User, db: Session) -> float:
    since = datetime.now(UTC).replace(day=1, hour=0, minute=0, second=0, microsecond=0)
    # Summed in the database: this runs on every hint, and loading the month's rows (summaries and
    # metrics included) to add up one column would slow every hint down as the month goes on.
    spent = db.exec(
        select(func.coalesce(func.sum(Call.cost_usd), 0.0)).where(
            Call.user_id == user.id, Call.created_at >= since.replace(tzinfo=None)
        )
    ).one()
    return float(spent)


def _check_spend_limit(user: User, db: Session) -> None:
    if user.monthly_spend_limit_usd is None:
        return
    spent = _spent_this_month(user, db)
    if spent >= user.monthly_spend_limit_usd:
        raise HTTPException(
            status_code=402,
            detail=(
                f"This month's spend limit is reached (${spent:.2f} of "
                f"${user.monthly_spend_limit_usd:.2f}). Ask an admin to raise it."
            ),
        )


@router.post("/stt/key")
async def temporary_stt_key(
    user: User = Depends(current_user),
    db: Session = Depends(session),
) -> dict[str, Any]:
    """A Deepgram key that works for a few hours, for one person's calls."""
    _check_spend_limit(user, db)
    key = settings().deepgram_api_key
    if not key:
        raise HTTPException(status_code=503, detail="No Deepgram key is set on the server")

    async with httpx.AsyncClient(timeout=TIMEOUT) as client:
        headers = {"Authorization": f"Token {key}"}
        projects = await client.get(f"{DEEPGRAM_API}/v1/projects", headers=headers)
        if projects.status_code != 200:
            raise HTTPException(status_code=502, detail="Deepgram refused the server's key")
        found = projects.json().get("projects") or []
        if not found:
            raise HTTPException(status_code=502, detail="That Deepgram account has no project")
        project_id = found[0]["project_id"]

        created = await client.post(
            f"{DEEPGRAM_API}/v1/projects/{project_id}/keys",
            headers=headers,
            json={
                "comment": f"sidekick {user.email}",
                # Transcription only: this key can't read the account or make more keys.
                "scopes": ["usage:write"],
                "time_to_live_in_seconds": STT_KEY_TTL_SECONDS,
            },
        )
    if created.status_code >= 300:
        detail = created.json().get("details") or created.text[:200]
        raise HTTPException(
            status_code=502,
            detail=(
                "Deepgram wouldn't issue a temporary key. The server's key needs the "
                f"'keys:write' scope. ({detail})"
            ),
        )
    body = created.json()
    return {
        "api_key": body["key"],
        "expires_at": body.get("expiration_date")
        or (datetime.now(UTC) + timedelta(seconds=STT_KEY_TTL_SECONDS)).isoformat(),
    }


@router.post("/llm/messages")
async def claude_messages(
    request: Request,
    user: User = Depends(current_user),
    db: Session = Depends(session),
) -> Any:
    """Passes a Claude request through with the server's key.

    The body is the app's, unchanged, so prompt caching and model choice stay where they belong:
    in the app, next to the coaching logic.
    """
    _check_spend_limit(user, db)
    # Give the database connection back before waiting on Claude, which can take many seconds:
    # held for the whole request, a few calls' worth of hints would use up the pool and lock
    # everyone out of the backend.
    db.close()
    key = settings().anthropic_api_key
    if not key:
        raise HTTPException(status_code=503, detail="No Claude key is set on the server")

    # Passed through as bytes both ways: nothing here needs to read it.
    body = await request.body()
    headers = {
        "x-api-key": key,
        "anthropic-version": ANTHROPIC_VERSION,
        "content-type": "application/json",
    }
    try:
        try:
            response = await _anthropic.post(
                f"{ANTHROPIC_API}/v1/messages", headers=headers, content=body
            )
        except _STALE_CONNECTION:
            response = await _anthropic.post(
                f"{ANTHROPIC_API}/v1/messages", headers=headers, content=body
            )
    except httpx.TimeoutException as problem:
        raise HTTPException(
            status_code=504, detail=f"Claude didn't answer: {problem!r}"
        ) from problem
    except httpx.HTTPError as problem:
        raise HTTPException(
            status_code=502, detail=f"Couldn't reach Claude: {problem!r}"
        ) from problem
    if response.status_code >= 300:
        raise HTTPException(status_code=response.status_code, detail=response.text[:400])
    return Response(content=response.content, media_type="application/json")


@router.post("/llm/messages/stream")
async def claude_messages_stream(
    request: Request,
    user: User = Depends(current_user),
    db: Session = Depends(session),
) -> StreamingResponse:
    """Like `/llm/messages`, for a request with `"stream": true`: Claude's server-sent events are
    passed on as they arrive, so a hint can appear word by word.

    Errors before the first event come back as ordinary HTTP errors, like the non-streaming route.
    """
    _check_spend_limit(user, db)
    db.close()  # as above: never hold a database connection while Claude writes
    key = settings().anthropic_api_key
    if not key:
        raise HTTPException(status_code=503, detail="No Claude key is set on the server")

    body = await request.body()
    headers = {
        "x-api-key": key,
        "anthropic-version": ANTHROPIC_VERSION,
        "content-type": "application/json",
        # Uncompressed, so each event is passed on the moment it arrives.
        "accept-encoding": "identity",
    }

    async def send() -> httpx.Response:
        upstream = _anthropic.build_request(
            "POST", f"{ANTHROPIC_API}/v1/messages", headers=headers, content=body
        )
        return await _anthropic.send(upstream, stream=True)

    try:
        try:
            response = await send()
        except _STALE_CONNECTION:
            response = await send()
    except httpx.TimeoutException as problem:
        raise HTTPException(
            status_code=504, detail=f"Claude didn't answer: {problem!r}"
        ) from problem
    except httpx.HTTPError as problem:
        raise HTTPException(
            status_code=502, detail=f"Couldn't reach Claude: {problem!r}"
        ) from problem
    if response.status_code >= 300:
        detail = (await response.aread()).decode(errors="replace")[:400]
        await response.aclose()
        raise HTTPException(status_code=response.status_code, detail=detail)

    async def relay() -> AsyncIterator[bytes]:
        try:
            # Decoded: if Claude compressed the answer anyway, the app still gets the events
            # (the raw bytes would go out compressed with no header saying so).
            async for chunk in response.aiter_bytes():
                yield chunk
        finally:
            await response.aclose()

    # nginx buffers responses by default, which would hold the whole stream back; this header
    # turns that off for this response only.
    return StreamingResponse(
        relay(),
        media_type="text/event-stream",
        headers={"Cache-Control": "no-cache", "X-Accel-Buffering": "no"},
    )
