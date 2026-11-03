"""Workspace settings: what a fresh install of the desktop app pulls after logging in.

The playbook is versioned, so an edit made five minutes before a call can be rolled back.
"""

from fastapi import APIRouter, Depends, HTTPException
from sqlmodel import Session, select

from app.auth import current_user
from app.db import session
from app.models import Playbook, User, WorkspaceSettings, now
from app.schemas import SettingsIn, SettingsOut

router = APIRouter(prefix="/settings", tags=["settings"])

DEFAULT_PLAYBOOK = (
    "This workspace has no playbook yet. Add one in Settings: hints only use facts from it."
)


def _settings_for(workspace_id, db: Session) -> WorkspaceSettings:
    found = db.exec(
        select(WorkspaceSettings).where(WorkspaceSettings.workspace_id == workspace_id)
    ).first()
    if found is None:
        found = WorkspaceSettings(workspace_id=workspace_id)
        db.add(found)
        db.commit()
        db.refresh(found)
    return found


def _latest_playbook(workspace_id, db: Session) -> Playbook | None:
    return db.exec(
        select(Playbook)
        .where(Playbook.workspace_id == workspace_id)
        .order_by(Playbook.version.desc())
    ).first()


def _out(settings: WorkspaceSettings, playbook: Playbook | None) -> SettingsOut:
    return SettingsOut(
        language=settings.language,
        max_call_minutes=settings.max_call_minutes,
        auto_stop_silence_minutes=settings.auto_stop_silence_minutes,
        max_call_cost_usd=settings.max_call_cost_usd,
        call_detection=settings.call_detection,
        deepgram_mip_opt_out=settings.deepgram_mip_opt_out,
        playbook=playbook.text if playbook else DEFAULT_PLAYBOOK,
        playbook_version=playbook.version if playbook else 0,
    )


@router.get("", response_model=SettingsOut)
def read_settings(
    user: User = Depends(current_user), db: Session = Depends(session)
) -> SettingsOut:
    return _out(_settings_for(user.workspace_id, db), _latest_playbook(user.workspace_id, db))


@router.put("", response_model=SettingsOut)
def update_settings(
    payload: SettingsIn,
    user: User = Depends(current_user),
    db: Session = Depends(session),
) -> SettingsOut:
    if not user.is_admin:
        raise HTTPException(status_code=403, detail="Only an admin can change workspace settings")

    settings = _settings_for(user.workspace_id, db)
    for field, value in payload.model_dump(exclude={"playbook"}, exclude_none=True).items():
        setattr(settings, field, value)
    settings.updated_at = now()
    db.add(settings)

    playbook = _latest_playbook(user.workspace_id, db)
    if payload.playbook is not None and payload.playbook != (playbook.text if playbook else None):
        playbook = Playbook(
            workspace_id=user.workspace_id,
            version=(playbook.version + 1) if playbook else 1,
            text=payload.playbook,
            edited_by=user.id,
        )
        db.add(playbook)

    db.commit()
    db.refresh(settings)
    return _out(settings, _latest_playbook(user.workspace_id, db))
