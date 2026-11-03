"""Server settings.

Secrets live in the environment, never in the database: the vendor keys, the database URL and the
token signing secret. Everything a workspace can change (playbook, limits, language) lives in the
database instead, so a fresh install of the desktop app is configured by logging in.
"""

from functools import lru_cache

from pydantic_settings import BaseSettings, SettingsConfigDict


class Settings(BaseSettings):
    model_config = SettingsConfigDict(env_file=".env", extra="ignore")

    database_url: str = "postgresql+psycopg://sidekick:sidekick@localhost:55432/sidekick"
    # Signs the tokens the desktop app holds. Change it and every app has to log in again.
    jwt_secret: str = "change-me-in-production"
    jwt_hours: int = 24 * 30

    # Vendor keys. The desktop app never sees these: it asks for a short-lived Deepgram key per
    # call, and its Claude requests come through /llm/messages.
    anthropic_api_key: str = ""
    deepgram_api_key: str = ""

    # The workspace's CRM (Twenty). The desktop app never holds these: it asks this server.
    twenty_base_url: str = ""
    twenty_api_key: str = ""

    # Published app builds: the folder they sit in, and the address the app reaches this server
    # on (the update file has to hand out a downloadable URL).
    updates_dir: str = "data/updates"
    public_url: str = "http://localhost:8000"


@lru_cache
def settings() -> Settings:
    return Settings()
