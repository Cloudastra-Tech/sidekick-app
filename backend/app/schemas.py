"""What crosses the wire, in the desktop app's terms.

These stay separate from the tables so the app's payload can keep its own shape (a call, its
summary, its consent events) while the database stores it the way it likes.
"""

from datetime import datetime
from typing import Any

from pydantic import BaseModel, EmailStr, Field


class LoginRequest(BaseModel):
    email: EmailStr
    password: str


class LoginResponse(BaseModel):
    access_token: str
    token_type: str = "bearer"
    user: "UserOut"


class UserOut(BaseModel):
    id: str
    email: str
    name: str
    workspace_id: str
    workspace_name: str
    is_admin: bool


class MemberIn(BaseModel):
    email: EmailStr
    name: str = ""
    is_admin: bool = False
    monthly_spend_limit_usd: float | None = Field(default=None, gt=0)


class MemberUpdate(BaseModel):
    name: str | None = None
    is_admin: bool | None = None
    is_active: bool | None = None
    monthly_spend_limit_usd: float | None = Field(default=None, gt=0)


class MemberOut(BaseModel):
    id: str
    email: str
    name: str
    is_admin: bool
    is_active: bool
    monthly_spend_limit_usd: float | None
    calls: int
    created_at: datetime


class NewMemberOut(BaseModel):
    member: MemberOut
    """Shown once to the admin who created or reset the member; never stored in the clear."""
    temporary_password: str


class SettingsOut(BaseModel):
    """Everything a fresh install needs before its first call."""

    language: str
    max_call_minutes: float
    auto_stop_silence_minutes: float
    max_call_cost_usd: float
    call_detection: bool
    deepgram_mip_opt_out: bool
    playbook: str
    playbook_version: int


class SettingsIn(BaseModel):
    language: str | None = None
    max_call_minutes: float | None = Field(default=None, gt=0, le=1440)
    auto_stop_silence_minutes: float | None = Field(default=None, gt=0, le=1440)
    max_call_cost_usd: float | None = Field(default=None, gt=0, le=1000)
    call_detection: bool | None = None
    deepgram_mip_opt_out: bool | None = None
    playbook: str | None = Field(default=None, min_length=200)


class ConsentEventIn(BaseModel):
    event: str
    acknowledged_by: str = ""
    audio_source: str = ""
    at: datetime | None = None


class CallIn(BaseModel):
    """A finished call, sent by the app. Sending the same `call_id` again updates it."""

    call_id: str
    prospect: str = ""
    company: str = ""
    crm_person_id: str | None = None
    crm_company_id: str | None = None
    mode: str = "live"
    started_at: datetime | None = None
    ended_at: datetime | None = None
    duration_minutes: float = 0
    cost_usd: float | None = None
    crm_record_url: str | None = None
    summary: dict[str, Any] | None = None
    metrics: dict[str, Any] | None = None
    # The laptop's metrics log for the call, one entry per line. Capped so one bad file can't
    # bloat the row: an hour-long call writes a few hundred.
    events: list[dict[str, Any]] | None = Field(default=None, max_length=20_000)
    transcript: list[dict[str, Any]] | None = Field(default=None, max_length=20_000)
    consent_events: list[ConsentEventIn] = []


class CallOut(BaseModel):
    id: str
    call_id: str
    prospect: str
    company: str
    crm_person_id: str | None
    crm_company_id: str | None = None
    mode: str
    duration_minutes: float
    cost_usd: float | None
    crm_record_url: str | None
    summary: dict[str, Any] | None
    metrics: dict[str, Any] | None
    ended_at: datetime | None
    created_at: datetime
    user_email: str


class ClientNoteIn(BaseModel):
    client_key: str
    client_name: str = ""
    text: str


class ClientNoteUpdate(BaseModel):
    text: str


class ClientDocumentOut(BaseModel):
    """A file or note as listed. A note carries its text so it can be edited; a file's text only
    goes out as call context."""

    id: str
    kind: str
    filename: str
    client_name: str
    words: int
    text: str = ""
    created_by: str
    created_at: datetime
    updated_at: datetime


class ClientDocumentsOut(BaseModel):
    documents: list[ClientDocumentOut]
    words: int
    word_limit: int


class ClientContextOut(BaseModel):
    text: str
    words: int


LoginResponse.model_rebuild()
