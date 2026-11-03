"""Signing in."""

from fastapi import APIRouter, Depends, HTTPException, status
from sqlmodel import Session, select

from app.auth import current_user, issue_token, password_matches
from app.db import session
from app.models import User, Workspace
from app.schemas import LoginRequest, LoginResponse, UserOut

router = APIRouter(tags=["auth"])


def _user_out(user: User, db: Session) -> UserOut:
    workspace = db.exec(select(Workspace).where(Workspace.id == user.workspace_id)).first()
    return UserOut(
        id=str(user.id),
        email=user.email,
        name=user.name,
        workspace_id=str(user.workspace_id),
        workspace_name=workspace.name if workspace else "",
        is_admin=user.is_admin,
    )


@router.post("/auth/login", response_model=LoginResponse)
def login(payload: LoginRequest, db: Session = Depends(session)) -> LoginResponse:
    user = db.exec(select(User).where(User.email == payload.email.lower())).first()
    # The same message either way, so this can't be used to find out who has an account.
    if (
        user is None
        or not user.is_active
        or not password_matches(payload.password, user.password_hash)
    ):
        raise HTTPException(
            status_code=status.HTTP_401_UNAUTHORIZED,
            detail="That email and password don't match",
        )
    return LoginResponse(access_token=issue_token(user), user=_user_out(user, db))


@router.get("/me", response_model=UserOut)
def me(user: User = Depends(current_user), db: Session = Depends(session)) -> UserOut:
    return _user_out(user, db)
