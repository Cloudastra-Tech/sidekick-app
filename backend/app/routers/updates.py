"""App updates.

The desktop app asks here whether a newer build exists, and downloads it from here. Both need a
sign-in, so installers never sit on a public URL, and the app checks Tauri's signature before
installing: a file served from this endpoint still can't be installed unless it was signed with
the release key.

An admin publishes a build with `POST /updates` (the installer plus the `.sig` Tauri produced).
Files live next to the database volume, not in the database: they are tens of megabytes.
"""

import json
import re
from datetime import UTC, datetime
from pathlib import Path

from fastapi import APIRouter, Depends, File, Form, HTTPException, Response, UploadFile
from fastapi.responses import FileResponse

from app.auth import current_user
from app.config import settings
from app.models import User
from app.routers.users import admin

router = APIRouter(prefix="/updates", tags=["updates"])

VERSION_PATTERN = re.compile(r"^\d+\.\d+\.\d+$")
MANIFEST = "manifest.json"
# What Tauri calls this platform; the app asks for its own.
DEFAULT_TARGET = "windows-x86_64"


def updates_dir() -> Path:
    path = Path(settings().updates_dir)
    path.mkdir(parents=True, exist_ok=True)
    return path


def read_manifest() -> dict:
    path = updates_dir() / MANIFEST
    if not path.exists():
        return {}
    return json.loads(path.read_text(encoding="utf-8"))


def write_manifest(manifest: dict) -> None:
    (updates_dir() / MANIFEST).write_text(json.dumps(manifest, indent=2), encoding="utf-8")


def _version_key(version: str) -> tuple[int, ...]:
    return tuple(int(part) for part in version.split("."))


@router.post("", status_code=201)
async def publish_update(
    version: str = Form(...),
    notes: str = Form(""),
    signature: str = Form(...),
    target: str = Form(DEFAULT_TARGET),
    installer: UploadFile = File(...),
    _: User = Depends(admin),
) -> dict:
    """Publishes a build. `signature` is the contents of the `.sig` file Tauri wrote beside it."""
    if not VERSION_PATTERN.match(version):
        raise HTTPException(status_code=400, detail="Version must look like 1.2.3")
    if not installer.filename:
        raise HTTPException(status_code=400, detail="The installer needs a file name")

    stored = updates_dir() / f"{version}-{target}-{Path(installer.filename).name}"
    stored.write_bytes(await installer.read())

    manifest = read_manifest()
    manifest.setdefault("releases", {})[f"{version}/{target}"] = {
        "version": version,
        "target": target,
        "notes": notes,
        "signature": signature.strip(),
        "file": stored.name,
        "published_at": datetime.now(UTC).isoformat(),
    }
    write_manifest(manifest)
    return {"version": version, "target": target, "file": stored.name}


# `response_model=None`: this returns either the update description or an empty 204.
@router.get("/latest", response_model=None)
def latest(target: str | None = None, _: User = Depends(current_user)) -> dict | Response:
    """The newest build, in the shape Tauri's updater expects.

    The updater asks with the OS alone (`?target=windows` or `darwin`), then looks itself up by OS
    and architecture (`windows-x86_64`, `darwin-aarch64`). So the answer is the newest version
    that has a build for that OS, with every architecture of it listed. Windows and macOS are
    released separately: a Windows build must not tell Macs there's an update they can't get.

    Nothing published for that OS: 204, which is how Tauri's updater spells "nothing to do".
    """
    releases = list(read_manifest().get("releases", {}).values())
    if target:
        releases = [
            r for r in releases if r["target"] == target or r["target"].startswith(f"{target}-")
        ]
    if not releases:
        return Response(status_code=204)

    version = max(releases, key=lambda r: _version_key(r["version"]))["version"]
    of_version = [r for r in releases if r["version"] == version]
    return {
        "version": version,
        "notes": of_version[0]["notes"],
        "pub_date": of_version[0]["published_at"],
        "platforms": {
            release["target"]: {
                "signature": release["signature"],
                "url": (
                    f"{settings().public_url}/updates/download/{version}?target={release['target']}"
                ),
            }
            for release in of_version
        },
    }


@router.get("/download/{version}")
def download(
    version: str,
    target: str = DEFAULT_TARGET,
    _: User = Depends(current_user),
) -> FileResponse:
    release = read_manifest().get("releases", {}).get(f"{version}/{target}")
    if release is None:
        raise HTTPException(status_code=404, detail="No such build")
    path = updates_dir() / release["file"]
    if not path.exists():
        raise HTTPException(
            status_code=410, detail="That build's installer is gone from the server"
        )
    return FileResponse(path, media_type="application/octet-stream", filename=release["file"])
