"""The workspace's CRM, reached through this server.

The desktop app has no CRM key: it asks here, and the server talks to Twenty with the
workspace's own credentials. That way a rep who installs the app can read and write the CRM
without a key on their laptop, and revoking their account revokes their CRM access with it.

This is a narrow door, not a general proxy: only the record types Sidekick uses, only the
methods it needs, and never a delete. A signed-in rep can create a person and update an
opportunity; they cannot wipe the pipeline through this endpoint.
"""

import asyncio
import time
from typing import Any, Literal

import httpx
from fastapi import APIRouter, Depends, HTTPException
from pydantic import BaseModel

from app.auth import current_user
from app.config import settings
from app.models import User

router = APIRouter(prefix="/crm", tags=["crm"])

# Cloudflare in front of some workspaces rejects requests without a browser agent (error 1010).
USER_AGENT = (
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 "
    "(KHTML, like Gecko) Chrome/140.0.0.0 Safari/537.36"
)
# The CRM sits behind Cloudflare, which occasionally takes tens of seconds on a cold connection.
TIMEOUT = httpx.Timeout(45.0, connect=15.0)
# One client for the process, so connections (and their TLS handshakes) are reused. The idle limit
# outlives httpx's 5 s default, so the reads a call start makes a few seconds apart share one.
_client = httpx.AsyncClient(
    timeout=TIMEOUT,
    limits=httpx.Limits(max_keepalive_connections=8, keepalive_expiry=300),
)

# What Sidekick reads and writes: people, companies, the opportunity a call lands on, and the
# notes it attaches. Anything else is refused.
ALLOWED = {
    "personsCustoms",
    "companyCustoms",
    "opportunityCustoms",
    "notes",
    "noteTargets",
}


class CrmRequest(BaseModel):
    method: Literal["GET", "POST", "PATCH"]
    """A REST path under /rest, e.g. `opportunityCustoms` or `opportunityCustoms/<id>`."""
    path: str
    params: dict[str, str] = {}
    body: dict[str, Any] | None = None


def _collection(path: str) -> str:
    return path.strip("/").split("/")[0].split("?")[0]


@router.get("/status")
def crm_status(_: User = Depends(current_user)) -> dict:
    """Whether this workspace has a CRM configured, for the app's Send to CRM panel."""
    configured = bool(settings().twenty_api_key and settings().twenty_base_url)
    return {"configured": configured, "base_url": settings().twenty_base_url}


class CrmPersonEntry(BaseModel):
    """One person as the start-call search shows them. Deals are read after a pick."""

    id: str
    name: str
    email: str
    job_title: str
    phone: str
    company_id: str | None
    company_name: str


# The CRM takes about a second to answer any search, which is too slow to type against. The
# whole people list is small (hundreds), so it is kept here and the app searches it locally.
PEOPLE_FRESH_FOR = 5 * 60
PAGE_SIZE = "200"
_people: list[CrmPersonEntry] | None = None
_people_at = 0.0
_people_lock = asyncio.Lock()


def _crm() -> tuple[str, str]:
    key, base = settings().twenty_api_key, settings().twenty_base_url.rstrip("/")
    if not key or not base:
        raise HTTPException(status_code=503, detail="No CRM is configured for this workspace")
    return key, base


async def _all(collection: str) -> list[dict[str, Any]]:
    """Every record of a collection, a page at a time."""
    key, base = _crm()
    records: list[dict[str, Any]] = []
    params = {"limit": PAGE_SIZE}
    while True:
        response = await _client.get(
            f"{base}/rest/{collection}",
            params=params,
            headers={"Authorization": f"Bearer {key}", "User-Agent": USER_AGENT},
        )
        response.raise_for_status()
        body = response.json()
        records.extend(body.get("data", {}).get(collection, []))
        page = body.get("pageInfo") or {}
        if not page.get("hasNextPage") or not page.get("endCursor"):
            return records
        params = {"limit": PAGE_SIZE, "starting_after": page["endCursor"]}


def _text(record: dict[str, Any], *keys: str) -> str:
    for key in keys:
        value = record.get(key)
        if isinstance(value, str) and value.strip():
            return value.strip()
    return ""


async def _read_people() -> list[CrmPersonEntry]:
    people, companies = await asyncio.gather(_all("personsCustoms"), _all("companyCustoms"))
    company_names = {c["id"]: _text(c, "name") for c in companies if c.get("id")}
    return [
        CrmPersonEntry(
            id=p["id"],
            name=_text(p, "name"),
            email=_text(p, "workEmail", "email"),
            job_title=_text(p, "title"),
            phone=_text(p, "phone"),
            company_id=p.get("companyId") or None,
            # Blank when the company was deleted but the person still points at it.
            company_name=company_names.get(p.get("companyId") or "", ""),
        )
        for p in people
        if p.get("id") and (_text(p, "name") or _text(p, "workEmail", "email"))
    ]


async def _refresh_people() -> None:
    global _people, _people_at
    async with _people_lock:
        if _people is not None and time.monotonic() - _people_at < PEOPLE_FRESH_FOR:
            return  # Someone else refreshed while this waited.
        _people = await _read_people()
        _people_at = time.monotonic()


async def _refresh_quietly() -> None:
    try:
        await _refresh_people()
    except (httpx.HTTPError, HTTPException):
        pass  # The old list keeps serving; the next request tries again.


@router.get("/people")
async def crm_people(_: User = Depends(current_user)) -> list[CrmPersonEntry]:
    """Everyone in the CRM, slimmed down for the app's instant search.

    Answers from memory. A list older than five minutes is still served at once while a fresh
    one is read behind it, so only the very first request after a restart waits for the CRM.
    """
    _crm()
    if _people is None:
        try:
            await _refresh_people()
        except httpx.HTTPError as problem:
            raise HTTPException(
                status_code=504, detail=f"The CRM didn't answer: {problem}"
            ) from problem
    elif time.monotonic() - _people_at >= PEOPLE_FRESH_FOR and not _people_lock.locked():
        task = asyncio.create_task(_refresh_quietly())
        _background.add(task)  # A task nobody holds can be collected mid-run.
        task.add_done_callback(_background.discard)
    return _people or []


_background: set[asyncio.Task[None]] = set()


@router.post("/request")
async def crm_request(payload: CrmRequest, _: User = Depends(current_user)) -> Any:
    """Passes one CRM call through with the workspace's key."""
    key, base = _crm()

    collection = _collection(payload.path)
    if collection not in ALLOWED:
        raise HTTPException(
            status_code=403,
            detail=f"Sidekick doesn't touch '{collection}' records",
        )

    try:
        response = await _client.request(
            payload.method,
            f"{base}/rest/{payload.path.lstrip('/')}",
            params=payload.params or None,
            json=payload.body,
            headers={
                "Authorization": f"Bearer {key}",
                "Content-Type": "application/json",
                "User-Agent": USER_AGENT,
            },
        )
    except httpx.HTTPError as problem:
        raise HTTPException(
            status_code=504, detail=f"The CRM didn't answer: {problem}"
        ) from problem
    if response.status_code >= 300:
        raise HTTPException(status_code=response.status_code, detail=response.text[:400])
    if payload.method != "GET" and collection in {"personsCustoms", "companyCustoms"}:
        # A new or renamed person should show in the next search, not five minutes later.
        global _people_at
        _people_at = 0.0
    return response.json()
