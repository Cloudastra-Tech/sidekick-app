"""Calls the desktop app has finished.

A call arrives under the app's own id, so a retry after a dropped connection updates the same row
instead of adding a second one, with its summary, numbers, logs, consent events and transcript.
An admin reads every call in the workspace; a rep only their own. Earlier calls with a prospect
(`/previous`) are the exception: the coach of whoever meets them next needs those summaries.
"""

from uuid import UUID

from fastapi import APIRouter, Depends, HTTPException, Query
from sqlalchemy import and_, false, func, or_
from sqlmodel import Session, select

from app.auth import current_user
from app.db import session
from app.models import Call, ConsentEvent, User, now
from app.schemas import CallIn, CallOut

router = APIRouter(prefix="/calls", tags=["calls"])


def _emails(workspace_id: UUID, db: Session) -> dict[UUID, str]:
    """Who made each call, by user id."""
    users = db.exec(select(User).where(User.workspace_id == workspace_id)).all()
    return {u.id: u.email for u in users}


def _out(call: Call, email: str) -> CallOut:
    return CallOut(
        id=str(call.id),
        call_id=call.call_id,
        prospect=call.prospect,
        company=call.company,
        crm_person_id=call.crm_person_id,
        crm_company_id=call.crm_company_id,
        mode=call.mode,
        duration_minutes=call.duration_minutes,
        cost_usd=call.cost_usd,
        crm_record_url=call.crm_record_url,
        summary=call.summary,
        metrics=call.metrics,
        ended_at=call.ended_at,
        created_at=call.created_at,
        user_email=email,
    )


@router.post("", response_model=CallOut)
def push_call(
    payload: CallIn,
    user: User = Depends(current_user),
    db: Session = Depends(session),
) -> CallOut:
    call = db.exec(
        select(Call).where(
            Call.workspace_id == user.workspace_id,
            Call.call_id == payload.call_id,
        )
    ).first()
    if call is None:
        call = Call(workspace_id=user.workspace_id, user_id=user.id, call_id=payload.call_id)
    elif call.user_id != user.id:
        # Only the rep who made a call sends it again; anyone else would overwrite it and read it
        # back. Same answer as for a call they may not read.
        raise HTTPException(status_code=404, detail="No such call in this workspace")

    fields = payload.model_dump(exclude={"call_id", "consent_events"}, exclude_none=True)
    for field, value in fields.items():
        setattr(call, field, value)
    call.updated_at = now()
    db.add(call)

    # Consent events are append-only; the app may send the same ones again after a retry.
    seen = {
        (event.event, event.at)
        for event in db.exec(
            select(ConsentEvent).where(
                ConsentEvent.workspace_id == user.workspace_id,
                ConsentEvent.call_id == payload.call_id,
            )
        ).all()
    }
    for event in payload.consent_events:
        if (event.event, event.at) in seen:
            continue
        db.add(
            ConsentEvent(
                workspace_id=user.workspace_id,
                call_id=payload.call_id,
                event=event.event,
                acknowledged_by=event.acknowledged_by,
                audio_source=event.audio_source,
                at=event.at or now(),
            )
        )

    db.commit()
    db.refresh(call)
    return _out(call, user.email)


def _readable(call_id: str, user: User, db: Session) -> Call:
    """The call, if this user may read it: an admin reads everyone's, a rep their own. Anyone
    else gets the same answer as for a call that doesn't exist, so ids can't be probed."""
    call = db.exec(
        select(Call).where(Call.workspace_id == user.workspace_id, Call.call_id == call_id)
    ).first()
    if call is None or not (user.is_admin or call.user_id == user.id):
        raise HTTPException(status_code=404, detail="No such call in this workspace")
    return call


@router.get("", response_model=list[CallOut])
def list_calls(
    limit: int = Query(default=50, le=200),
    user: User = Depends(current_user),
    db: Session = Depends(session),
) -> list[CallOut]:
    """Newest first: the whole team's for an admin, a rep's own otherwise."""
    query = select(Call).where(Call.workspace_id == user.workspace_id)
    if not user.is_admin:
        query = query.where(Call.user_id == user.id)
    calls = db.exec(query.order_by(Call.created_at.desc()).limit(limit)).all()
    emails = _emails(user.workspace_id, db)
    return [_out(call, emails.get(call.user_id, "")) for call in calls]


@router.get("/previous", response_model=list[CallOut])
def previous_calls(
    crm_person_id: str | None = None,
    crm_company_id: str | None = None,
    prospect: str = "",
    company: str = "",
    limit: int = Query(default=3, ge=1, le=10),
    user: User = Depends(current_user),
    db: Session = Depends(session),
) -> list[CallOut]:
    """Summarized live calls with this prospect, newest first, from anyone in the workspace.

    Found by CRM person when the rep picked them from the CRM. Calls sent before the app sent
    that id, or with a prospect typed by hand, are found by prospect name and company instead.
    With the CRM company too, calls with anyone else at that company are included, so meeting a
    colleague of theirs starts from what the team already learned.
    A name match never reaches a call tied to a CRM person (that one is found by its id), and a
    name typed without a company only matches calls that had no company either, so "Rahul"
    doesn't pull in every Rahul the team has spoken to.
    """
    prospect, company = prospect.strip().lower(), company.strip().lower()
    # Names were stored as typed, stray spaces included.
    by_name = and_(
        (func.lower(func.trim(Call.prospect)) == prospect) if prospect else false(),
        func.lower(func.trim(Call.company)) == company,
        Call.crm_person_id.is_(None),
    )
    matches = [by_name]
    if crm_person_id:
        matches.append(Call.crm_person_id == crm_person_id)
    if crm_company_id:
        matches.append(Call.crm_company_id == crm_company_id)
    match = or_(*matches)
    calls = db.exec(
        select(Call)
        .where(Call.workspace_id == user.workspace_id, Call.mode == "live", match)
        .order_by(Call.created_at.desc())
        # A call whose summary failed is skipped below (a JSON null isn't an SQL NULL, so SQL can't
        # tell them apart portably); this many rows leaves room for a run of failed ones.
        .limit(50)
    ).all()
    calls = [call for call in calls if call.summary][:limit]
    emails = _emails(user.workspace_id, db)
    return [_out(call, emails.get(call.user_id, "")) for call in calls]


@router.get("/{call_id}", response_model=CallOut)
def get_call(
    call_id: str,
    user: User = Depends(current_user),
    db: Session = Depends(session),
) -> CallOut:
    call = _readable(call_id, user, db)
    owner = db.exec(select(User).where(User.id == call.user_id)).first()
    return _out(call, owner.email if owner else "")


@router.get("/{call_id}/transcript")
def get_call_transcript(
    call_id: str,
    user: User = Depends(current_user),
    db: Session = Depends(session),
) -> dict:
    """What was said, line by line, for an admin or the rep who made the call."""
    return {"transcript": _readable(call_id, user, db).transcript or []}


@router.get("/{call_id}/logs")
def get_call_logs(
    call_id: str,
    user: User = Depends(current_user),
    db: Session = Depends(session),
) -> dict:
    """What the laptop logged for this call: its metrics log and its consent events."""
    call = _readable(call_id, user, db)
    consent = db.exec(
        select(ConsentEvent)
        .where(ConsentEvent.workspace_id == user.workspace_id, ConsentEvent.call_id == call_id)
        .order_by(ConsentEvent.at)
    ).all()
    return {
        "events": call.events or [],
        "consent_events": [
            {
                "event": e.event,
                "acknowledged_by": e.acknowledged_by,
                "audio_source": e.audio_source,
                "at": e.at,
            }
            for e in consent
        ],
    }
