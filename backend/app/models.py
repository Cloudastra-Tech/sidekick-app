"""Database tables.

One workspace per company, users inside it, and everything else hanging off the workspace. Call
summaries and metrics are stored as JSON documents because their shape is the model's output, not
something the backend reasons about; the columns the team filters on are their own fields.

Transcripts are kept so admins can review calls (decided 2026-10-27; the consent text tells the
prospect the transcript is saved for the team). The legal review of consent and data handling is
still open (roadmap Phase 4) and must happen before any external pilot.
"""

from datetime import UTC, datetime
from typing import Any
from uuid import UUID, uuid4

from sqlalchemy import Column, UniqueConstraint
from sqlalchemy.dialects.postgresql import JSONB
from sqlmodel import JSON, Field, SQLModel

# Postgres gets JSONB; SQLite (tests) gets plain JSON.
JSON_COLUMN = JSON().with_variant(JSONB(), "postgresql")


def now() -> datetime:
    return datetime.now(UTC)


class Workspace(SQLModel, table=True):
    __tablename__ = "workspaces"

    id: UUID = Field(default_factory=uuid4, primary_key=True)
    name: str
    created_at: datetime = Field(default_factory=now)


class User(SQLModel, table=True):
    __tablename__ = "users"

    id: UUID = Field(default_factory=uuid4, primary_key=True)
    workspace_id: UUID = Field(foreign_key="workspaces.id", index=True)
    email: str = Field(unique=True, index=True)
    name: str = ""
    password_hash: str
    is_admin: bool = False
    # A member who has left: kept so their calls keep an owner, but can no longer sign in.
    is_active: bool = True
    # Spend allowed per user per month, in USD; null means no limit of their own.
    monthly_spend_limit_usd: float | None = None
    created_at: datetime = Field(default_factory=now)


class WorkspaceSettings(SQLModel, table=True):
    """What the desktop app pulls after logging in, instead of reading local files."""

    __tablename__ = "workspace_settings"

    workspace_id: UUID = Field(foreign_key="workspaces.id", primary_key=True)
    language: str = "en"
    max_call_minutes: float = 180
    auto_stop_silence_minutes: float = 15
    max_call_cost_usd: float = 5
    call_detection: bool = True
    deepgram_mip_opt_out: bool = True
    updated_at: datetime = Field(default_factory=now)


class Playbook(SQLModel, table=True):
    """Kept as versions, so a bad edit during a call day can be rolled back."""

    __tablename__ = "playbooks"
    __table_args__ = (UniqueConstraint("workspace_id", "version"),)

    id: UUID = Field(default_factory=uuid4, primary_key=True)
    workspace_id: UUID = Field(foreign_key="workspaces.id", index=True)
    version: int
    text: str
    edited_by: UUID | None = Field(default=None, foreign_key="users.id")
    created_at: datetime = Field(default_factory=now)


class Call(SQLModel, table=True):
    """One coached call. `call_id` is the desktop app's own id, so re-sending updates this row."""

    __tablename__ = "calls"
    __table_args__ = (UniqueConstraint("workspace_id", "call_id"),)

    id: UUID = Field(default_factory=uuid4, primary_key=True)
    workspace_id: UUID = Field(foreign_key="workspaces.id", index=True)
    user_id: UUID = Field(foreign_key="users.id", index=True)
    call_id: str = Field(index=True)
    prospect: str = ""
    company: str = ""
    # The CRM person picked before the call, so their next call can find this one.
    crm_person_id: str | None = Field(default=None, index=True)
    # The CRM company, so a call with a colleague of theirs finds this one too.
    crm_company_id: str | None = Field(default=None, index=True)
    mode: str = "live"
    started_at: datetime | None = None
    ended_at: datetime | None = None
    duration_minutes: float = 0
    cost_usd: float | None = None
    crm_record_url: str | None = None
    summary: dict[str, Any] | None = Field(default=None, sa_column=Column(JSON_COLUMN))
    metrics: dict[str, Any] | None = Field(default=None, sa_column=Column(JSON_COLUMN))
    # The call's whole metrics log from the laptop, one entry per line: every Claude request with
    # its trigger, timings and tokens, every hint painted. What a "no hints" report is checked
    # against. Never the transcript.
    events: list[dict[str, Any]] | None = Field(default=None, sa_column=Column(JSON_COLUMN))
    # What was said, `{"channel": "me" | "them", "text": ...}` per line. Read only by admins and
    # the rep who made the call.
    transcript: list[dict[str, Any]] | None = Field(default=None, sa_column=Column(JSON_COLUMN))
    created_at: datetime = Field(default_factory=now)
    updated_at: datetime = Field(default_factory=now)


class ConsentEvent(SQLModel, table=True):
    """The record that matters in a dispute, kept off the laptop that could be wiped."""

    __tablename__ = "consent_events"

    id: UUID = Field(default_factory=uuid4, primary_key=True)
    workspace_id: UUID = Field(foreign_key="workspaces.id", index=True)
    call_id: str = Field(index=True)
    event: str
    acknowledged_by: str = ""
    audio_source: str = ""
    at: datetime = Field(default_factory=now)
    created_at: datetime = Field(default_factory=now)


class ClientDocument(SQLModel, table=True):
    """A file or typed note about a client company, used on calls with anyone there.

    `client_key` is `crm:<company id>` when the company came from the CRM, else `name:<company
    name, lower case>`. Only the extracted text is kept, not the file.
    """

    __tablename__ = "client_documents"

    id: UUID = Field(default_factory=uuid4, primary_key=True)
    workspace_id: UUID = Field(foreign_key="workspaces.id", index=True)
    client_key: str = Field(index=True)
    client_name: str = ""
    kind: str = "file"  # "file" or "note"
    filename: str = ""
    text: str
    words: int = 0
    created_by: UUID = Field(foreign_key="users.id")
    created_at: datetime = Field(default_factory=now)
    updated_at: datetime = Field(default_factory=now)
