"""Creates the first workspace and admin, so a fresh database can be logged into.

    uv run python -m app.seed --workspace Acme --email you@example.com --password ...

Running it again with the same email resets that user's password instead of failing, which is the
behaviour you want at 9pm before a demo.
"""

import argparse

from sqlmodel import Session, select

from app import db as database
from app.auth import hash_password
from app.db import create_tables
from app.models import User, Workspace, WorkspaceSettings


def seed(workspace_name: str, email: str, password: str, name: str) -> str:
    create_tables()
    email = email.lower()
    # Looked up at call time, not import time, so tests can point it at their own database.
    with Session(database.engine) as db:
        workspace = db.exec(select(Workspace).where(Workspace.name == workspace_name)).first()
        if workspace is None:
            workspace = Workspace(name=workspace_name)
            db.add(workspace)
            db.commit()
            db.refresh(workspace)
            db.add(WorkspaceSettings(workspace_id=workspace.id))
            db.commit()

        user = db.exec(select(User).where(User.email == email)).first()
        if user is None:
            user = User(
                workspace_id=workspace.id,
                email=email,
                name=name,
                password_hash=hash_password(password),
                is_admin=True,
            )
            action = "created"
        else:
            user.password_hash = hash_password(password)
            action = "password reset for"
        db.add(user)
        db.commit()
        return f"{action} {email} in workspace {workspace_name}"


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--workspace", required=True)
    parser.add_argument("--email", required=True)
    parser.add_argument("--password", required=True)
    parser.add_argument("--name", default="")
    args = parser.parse_args()
    print(seed(args.workspace, args.email, args.password, args.name))


if __name__ == "__main__":
    main()
