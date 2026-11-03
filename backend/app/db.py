"""Database engine and session.

Tables are created on startup while the schema is young. `create_all` never changes a table that
already exists, so a column added later is added by `add_new_columns` until Alembic migrations
arrive.
"""

from collections.abc import Generator

from sqlalchemy import inspect, text
from sqlmodel import Session, SQLModel, create_engine

from app.config import settings

engine = create_engine(settings().database_url, pool_pre_ping=True)

# (table, column, SQL type, indexed) added after the table first shipped. Each is nullable, so
# existing rows stay valid. `indexed` must match the model, so upgraded and fresh databases agree.
NEW_COLUMNS = [
    ("calls", "crm_person_id", "VARCHAR", True),
    ("calls", "crm_company_id", "VARCHAR", True),
    ("calls", "events", "JSONB", False),
    ("calls", "transcript", "JSONB", False),
]


def create_tables() -> None:
    SQLModel.metadata.create_all(engine)
    add_new_columns()


def add_new_columns() -> None:
    existing = inspect(engine)
    # Two processes starting at once (the API and a seed run) can both find the column missing;
    # Postgres can make the second ALTER a no-op. SQLite (tests) has no IF NOT EXISTS here.
    if_missing = "IF NOT EXISTS " if engine.dialect.name == "postgresql" else ""
    for table, column, sql_type, indexed in NEW_COLUMNS:
        if column in {c["name"] for c in existing.get_columns(table)}:
            continue
        with engine.begin() as connection:
            connection.execute(
                text(f"ALTER TABLE {table} ADD COLUMN {if_missing}{column} {sql_type}")
            )
            if indexed:
                connection.execute(
                    text(f"CREATE INDEX IF NOT EXISTS ix_{table}_{column} ON {table} ({column})")
                )


def session() -> Generator[Session, None, None]:
    with Session(engine) as db:
        yield db
