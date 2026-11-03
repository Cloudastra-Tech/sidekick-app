"""Who is in the workspace.

Admins add teammates here instead of running the seed command on the server. A new member gets a
one-time password, shown once to the admin who created them: there is no email sender yet, so it
is passed on by hand. Everything here is admin-only; a rep asking gets a 403.
"""

import secrets
import string

from fastapi import APIRouter, Depends, HTTPException, status
from sqlmodel import Session, select

from app.auth import current_user, hash_password
from app.db import session
from app.models import Call, User
from app.schemas import MemberIn, MemberOut, MemberUpdate, NewMemberOut

router = APIRouter(prefix="/users", tags=["users"])

# Unambiguous characters only: these get read out or retyped from a chat message.
PASSWORD_ALPHABET = "".join(c for c in string.ascii_letters + string.digits if c not in "0OlI1")


def temporary_password() -> str:
    return "-".join("".join(secrets.choice(PASSWORD_ALPHABET) for _ in range(4)) for _ in range(3))


def admin(user: User = Depends(current_user)) -> User:
    if not user.is_admin:
        raise HTTPException(
            status_code=status.HTTP_403_FORBIDDEN,
            detail="Only an admin can manage members",
        )
    return user


def _out(user: User, calls: int) -> MemberOut:
    return MemberOut(
        id=str(user.id),
        email=user.email,
        name=user.name,
        is_admin=user.is_admin,
        is_active=user.is_active,
        monthly_spend_limit_usd=user.monthly_spend_limit_usd,
        calls=calls,
        created_at=user.created_at,
    )


def _call_counts(workspace_id, db: Session) -> dict:
    counts: dict = {}
    for call in db.exec(select(Call).where(Call.workspace_id == workspace_id)).all():
        counts[call.user_id] = counts.get(call.user_id, 0) + 1
    return counts


@router.get("", response_model=list[MemberOut])
def list_members(user: User = Depends(admin), db: Session = Depends(session)) -> list[MemberOut]:
    members = db.exec(
        select(User).where(User.workspace_id == user.workspace_id).order_by(User.created_at)
    ).all()
    counts = _call_counts(user.workspace_id, db)
    return [_out(member, counts.get(member.id, 0)) for member in members]


@router.post("", response_model=NewMemberOut, status_code=status.HTTP_201_CREATED)
def add_member(
    payload: MemberIn,
    user: User = Depends(admin),
    db: Session = Depends(session),
) -> NewMemberOut:
    email = payload.email.lower()
    if db.exec(select(User).where(User.email == email)).first():
        raise HTTPException(status_code=409, detail=f"{email} already has an account")
    password = temporary_password()
    member = User(
        workspace_id=user.workspace_id,
        email=email,
        name=payload.name,
        password_hash=hash_password(password),
        is_admin=payload.is_admin,
        monthly_spend_limit_usd=payload.monthly_spend_limit_usd,
    )
    db.add(member)
    db.commit()
    db.refresh(member)
    # Shown once, to be passed on by hand; it is not stored anywhere in the clear.
    return NewMemberOut(member=_out(member, 0), temporary_password=password)


@router.patch("/{member_id}", response_model=MemberOut)
def update_member(
    member_id: str,
    payload: MemberUpdate,
    user: User = Depends(admin),
    db: Session = Depends(session),
) -> MemberOut:
    member = _member_in_workspace(member_id, user, db)
    changes = payload.model_dump(exclude_none=True)
    if changes.get("is_active") is False and member.id == user.id:
        raise HTTPException(status_code=400, detail="You can't deactivate your own account")
    if changes.get("is_admin") is False and member.id == user.id:
        raise HTTPException(status_code=400, detail="You can't remove your own admin rights")
    for field, value in changes.items():
        setattr(member, field, value)
    db.add(member)
    db.commit()
    db.refresh(member)
    return _out(member, _call_counts(user.workspace_id, db).get(member.id, 0))


@router.post("/{member_id}/password", response_model=NewMemberOut)
def reset_password(
    member_id: str,
    user: User = Depends(admin),
    db: Session = Depends(session),
) -> NewMemberOut:
    member = _member_in_workspace(member_id, user, db)
    password = temporary_password()
    member.password_hash = hash_password(password)
    db.add(member)
    db.commit()
    db.refresh(member)
    return NewMemberOut(
        member=_out(member, _call_counts(user.workspace_id, db).get(member.id, 0)),
        temporary_password=password,
    )


@router.delete("/{member_id}", status_code=status.HTTP_204_NO_CONTENT)
def remove_member(
    member_id: str,
    user: User = Depends(admin),
    db: Session = Depends(session),
) -> None:
    """Removes a member who has no calls. Anyone with calls is deactivated instead, so their
    call history keeps its owner."""
    member = _member_in_workspace(member_id, user, db)
    if member.id == user.id:
        raise HTTPException(status_code=400, detail="You can't remove your own account")
    if _call_counts(user.workspace_id, db).get(member.id, 0):
        raise HTTPException(
            status_code=409,
            detail="This member has calls in the workspace; deactivate them instead",
        )
    db.delete(member)
    db.commit()


def _member_in_workspace(member_id: str, user: User, db: Session) -> User:
    member = next(
        (
            m
            for m in db.exec(select(User).where(User.workspace_id == user.workspace_id)).all()
            if str(m.id) == member_id
        ),
        None,
    )
    if member is None:
        raise HTTPException(status_code=404, detail="No such member in this workspace")
    return member
