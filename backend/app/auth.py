"""Passwords and tokens.

The desktop app logs in once and keeps a token; every request after that carries it. Tokens are
signed with the server's secret and name the user, so per-user spend limits and per-user call
history work without the app ever holding a vendor key.
"""

from datetime import UTC, datetime, timedelta
from uuid import UUID

import bcrypt
import jwt
from fastapi import Depends, HTTPException, status
from fastapi.security import HTTPAuthorizationCredentials, HTTPBearer
from sqlmodel import Session, select

from app.config import settings
from app.db import session
from app.models import User

ALGORITHM = "HS256"
bearer = HTTPBearer(auto_error=False)


def hash_password(password: str) -> str:
    return bcrypt.hashpw(password.encode(), bcrypt.gensalt()).decode()


def password_matches(password: str, hashed: str) -> bool:
    try:
        return bcrypt.checkpw(password.encode(), hashed.encode())
    except ValueError:
        return False


def issue_token(user: User) -> str:
    expires = datetime.now(UTC) + timedelta(hours=settings().jwt_hours)
    payload = {"sub": str(user.id), "workspace": str(user.workspace_id), "exp": expires}
    return jwt.encode(payload, settings().jwt_secret, algorithm=ALGORITHM)


def current_user(
    credentials: HTTPAuthorizationCredentials | None = Depends(bearer),
    db: Session = Depends(session),
) -> User:
    unauthorized = HTTPException(
        status_code=status.HTTP_401_UNAUTHORIZED,
        detail="Sign in to continue",
        headers={"WWW-Authenticate": "Bearer"},
    )
    if credentials is None:
        raise unauthorized
    try:
        payload = jwt.decode(credentials.credentials, settings().jwt_secret, algorithms=[ALGORITHM])
        user_id = UUID(payload["sub"])
    except (jwt.PyJWTError, KeyError, ValueError) as error:
        raise unauthorized from error
    user = db.exec(select(User).where(User.id == user_id)).first()
    if user is None or not user.is_active:
        raise unauthorized
    return user
