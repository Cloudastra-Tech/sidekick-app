"""Sidekick backend.

The desktop app logs in here, pulls its workspace settings and playbook, and sends every finished
call: the summary, the numbers and the consent events. Transcripts stay on the laptop.
"""

from contextlib import asynccontextmanager

from fastapi import FastAPI

from app.db import create_tables
from app.routers import auth, calls, clients, crm, insights, settings, updates, users, vendors


@asynccontextmanager
async def lifespan(_: FastAPI):
    create_tables()
    yield
    # The shared outbound clients close their pooled connections cleanly on shutdown.
    await vendors._anthropic.aclose()
    await crm._client.aclose()


app = FastAPI(title="Sidekick backend", version="0.1.0", lifespan=lifespan)
app.include_router(auth.router)
app.include_router(settings.router)
app.include_router(calls.router)
app.include_router(users.router)
app.include_router(vendors.router)
app.include_router(updates.router)
app.include_router(crm.router)
app.include_router(clients.router)
app.include_router(insights.router)


@app.get("/health", tags=["health"])
def health() -> dict[str, str]:
    return {"status": "ok"}
