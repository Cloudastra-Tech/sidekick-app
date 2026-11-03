"""Files and typed notes about the client companies a team meets.

Everyone in the workspace can add, change and remove them, so whoever takes the next call with a
client has the same background. The coach gets them as text at the start of each call with that
client (`/clients/context`); a client never sees another's, and a workspace never sees another's.
"""

from uuid import UUID

from fastapi import APIRouter, Depends, File, Form, HTTPException, Response, UploadFile
from fastapi.concurrency import run_in_threadpool
from sqlmodel import Session, select

from app.auth import current_user
from app.db import session
from app.extract import UnreadableFile, clean, extract_text, word_count
from app.models import ClientDocument, User, now
from app.schemas import (
    ClientContextOut,
    ClientDocumentOut,
    ClientDocumentsOut,
    ClientNoteIn,
    ClientNoteUpdate,
)

router = APIRouter(prefix="/clients", tags=["clients"])

# All of a client's files and notes together. They go into every hint request's cached prefix, so
# this bounds both the cost (~$0.15 per call-hour at the limit) and how much slower hints get.
WORD_LIMIT = 18_000
MAX_UPLOAD_BYTES = 10 * 1024 * 1024
MAX_KEY_CHARS = 200


def normalize_key(raw: str) -> str:
    """`crm:<id>` as given; `name:<company>` lower case with single spaces, so typing varies."""
    key = raw.strip()[:MAX_KEY_CHARS]
    if key.startswith("name:"):
        key = "name:" + " ".join(key[5:].lower().split())
    if key in {"", "crm:", "name:"}:
        raise HTTPException(status_code=422, detail="Pick or type the client's company first")
    return key


def _documents(workspace_id: UUID, key: str, db: Session) -> list[ClientDocument]:
    return list(
        db.exec(
            select(ClientDocument)
            .where(ClientDocument.workspace_id == workspace_id, ClientDocument.client_key == key)
            .order_by(ClientDocument.created_at.desc())
        ).all()
    )


def _owned(document_id: UUID, user: User, db: Session) -> ClientDocument:
    document = db.get(ClientDocument, document_id)
    if document is None or document.workspace_id != user.workspace_id:
        raise HTTPException(status_code=404, detail="No such file or note in this workspace")
    return document


def _check_room(used: int, adding: int) -> None:
    if used + adding > WORD_LIMIT:
        raise HTTPException(
            status_code=413,
            detail=(
                f"This client would have {used + adding:,} words; the limit is "
                f"{WORD_LIMIT:,}. Remove a file or add a shorter one."
            ),
        )


def _out(document: ClientDocument, emails: dict[UUID, str]) -> ClientDocumentOut:
    return ClientDocumentOut(
        id=str(document.id),
        kind=document.kind,
        filename=document.filename,
        client_name=document.client_name,
        words=document.words,
        text=document.text if document.kind == "note" else "",
        created_by=emails.get(document.created_by, ""),
        created_at=document.created_at,
        updated_at=document.updated_at,
    )


def _emails(workspace_id: UUID, db: Session) -> dict[UUID, str]:
    users = db.exec(select(User).where(User.workspace_id == workspace_id)).all()
    return {u.id: u.email for u in users}


def _save(document: ClientDocument, user: User, db: Session) -> ClientDocumentOut:
    db.add(document)
    db.commit()
    db.refresh(document)
    return _out(document, {user.id: user.email} | _emails(user.workspace_id, db))


@router.get("/documents", response_model=ClientDocumentsOut)
def list_documents(
    client_key: str,
    user: User = Depends(current_user),
    db: Session = Depends(session),
) -> ClientDocumentsOut:
    documents = _documents(user.workspace_id, normalize_key(client_key), db)
    emails = _emails(user.workspace_id, db)
    return ClientDocumentsOut(
        documents=[_out(d, emails) for d in documents],
        words=sum(d.words for d in documents),
        word_limit=WORD_LIMIT,
    )


@router.post("/documents", response_model=ClientDocumentOut)
async def upload_document(
    client_key: str = Form(...),
    client_name: str = Form(""),
    file: UploadFile = File(...),
    user: User = Depends(current_user),
    db: Session = Depends(session),
) -> ClientDocumentOut:
    key = normalize_key(client_key)
    data = await file.read(MAX_UPLOAD_BYTES + 1)
    if len(data) > MAX_UPLOAD_BYTES:
        raise HTTPException(status_code=413, detail="Files can be up to 10 MB")
    filename = (file.filename or "file").strip()[:200]
    try:
        # A long PDF takes a moment; other requests shouldn't wait for it.
        text = await run_in_threadpool(extract_text, filename, data)
    except UnreadableFile as error:
        raise HTTPException(status_code=422, detail=str(error)) from error
    if not text:
        raise HTTPException(
            status_code=422,
            detail=f"{filename} has no text to read (a scanned PDF is only pictures)",
        )
    words = word_count(text)
    _check_room(sum(d.words for d in _documents(user.workspace_id, key, db)), words)
    return _save(
        ClientDocument(
            workspace_id=user.workspace_id,
            client_key=key,
            client_name=client_name.strip()[:200],
            kind="file",
            filename=filename,
            text=text,
            words=words,
            created_by=user.id,
        ),
        user,
        db,
    )


@router.post("/notes", response_model=ClientDocumentOut)
def add_note(
    payload: ClientNoteIn,
    user: User = Depends(current_user),
    db: Session = Depends(session),
) -> ClientDocumentOut:
    key = normalize_key(payload.client_key)
    text = clean(payload.text)
    if not text:
        raise HTTPException(status_code=422, detail="The note is empty")
    words = word_count(text)
    _check_room(sum(d.words for d in _documents(user.workspace_id, key, db)), words)
    return _save(
        ClientDocument(
            workspace_id=user.workspace_id,
            client_key=key,
            client_name=payload.client_name.strip()[:200],
            kind="note",
            text=text,
            words=words,
            created_by=user.id,
        ),
        user,
        db,
    )


@router.put("/notes/{document_id}", response_model=ClientDocumentOut)
def change_note(
    document_id: UUID,
    payload: ClientNoteUpdate,
    user: User = Depends(current_user),
    db: Session = Depends(session),
) -> ClientDocumentOut:
    note = _owned(document_id, user, db)
    if note.kind != "note":
        raise HTTPException(status_code=422, detail="Only typed notes can be edited")
    text = clean(payload.text)
    if not text:
        raise HTTPException(status_code=422, detail="The note is empty")
    words = word_count(text)
    others = [d for d in _documents(user.workspace_id, note.client_key, db) if d.id != note.id]
    _check_room(sum(d.words for d in others), words)
    note.text, note.words, note.updated_at = text, words, now()
    return _save(note, user, db)


@router.delete("/documents/{document_id}", status_code=204)
def remove_document(
    document_id: UUID,
    user: User = Depends(current_user),
    db: Session = Depends(session),
) -> Response:
    db.delete(_owned(document_id, user, db))
    db.commit()
    return Response(status_code=204)


@router.get("/context", response_model=ClientContextOut)
def call_context(
    client_key: str,
    user: User = Depends(current_user),
    db: Session = Depends(session),
) -> ClientContextOut:
    """The client's files and notes as one text for the coach, newest first."""
    documents = _documents(user.workspace_id, normalize_key(client_key), db)
    blocks = []
    for d in documents:
        title = f"File: {d.filename}" if d.kind == "file" else "Note"
        blocks.append(f"### {title} (added {d.created_at:%Y-%m-%d})\n\n{d.text}")
    return ClientContextOut(text="\n\n".join(blocks), words=sum(d.words for d in documents))
