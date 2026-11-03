"""How each rep's calls are going, for admins.

The conversation measures follow what call-intelligence tools report (talk ratio, longest
monologue, questions asked, back-and-forth), computed from the transcripts the app sends. The
coaching measures are Sidekick's own: was the prospect heard, did hints show, how fast.

Transcript lines carry no times, so "time" measures use words: a monologue is the most words the
rep said in a row, and talk ratio is the rep's share of all words.
"""

import math
from collections import defaultdict
from datetime import timedelta
from typing import Any
from uuid import UUID

from fastapi import APIRouter, Depends, Query
from sqlmodel import Session, select

from app.db import session
from app.models import Call, User, now
from app.routers.users import admin

router = APIRouter(prefix="/insights", tags=["insights"])

# Shorter than this says nothing about a rep: a misclick. The same cut-off as Team calls, so a
# call flagged there is counted here.
MIN_MINUTES = 0.3


def conversation(transcript: list[dict[str, Any]]) -> dict[str, float] | None:
    """Talk ratio, longest monologue, questions and switches for one call, or None without one."""
    lines = [
        (line.get("channel"), str(line.get("text") or "").strip())
        for line in transcript or []
        if line.get("channel") in ("me", "them")
    ]
    lines = [(channel, text) for channel, text in lines if text]
    if not lines:
        return None
    words = {"me": 0, "them": 0}
    longest = run = 0
    switches = 0
    questions = 0
    previous = None
    for channel, text in lines:
        count = len(text.split())
        words[channel] += count
        if channel == "me":
            run = run + count if previous == "me" else count
            longest = max(longest, run)
            questions += text.count("?")
        if previous is not None and channel != previous:
            switches += 1
        previous = channel
    total = words["me"] + words["them"]
    return {
        "talk_ratio": words["me"] / total if total else 0.0,
        "longest_monologue_words": float(longest),
        "questions": float(questions),
        "switches": float(switches),
    }


def _number(value: Any) -> float | None:
    # bool is an int in Python; a true/false is not a measurement.
    if isinstance(value, bool) or not isinstance(value, int | float):
        return None
    return float(value)


def _first_number(*values: Any) -> float | None:
    """The first value that is a number, 0 included."""
    return next((n for n in map(_number, values) if n is not None), None)


def p90(values: list[float]) -> float | None:
    """The value 90% of the others are at or under (nearest rank)."""
    if not values:
        return None
    ordered = sorted(values)
    return ordered[max(0, math.ceil(0.9 * len(ordered)) - 1)]


def hint_latency(calls: list[Call]) -> tuple[float | None, str]:
    """The p90 from speech to hint across every hint shown on these calls, from their logs.

    Calls sent before logs were kept only have their own p90; for those the slowest call's p90
    stands in, which can only overstate the true figure, never hide a slow call."""
    from_logs = [
        speech_to_hint
        for call in calls
        for event in call.events or []
        if event.get("kind") == "llm_call" and event.get("shown")
        for speech_to_hint in [_number((event.get("latency_ms") or {}).get("speech_to_hint"))]
        if speech_to_hint is not None
    ]
    if from_logs:
        return p90(from_logs), "hints"
    per_call = [
        value
        for call in calls
        for value in [
            _first_number(
                (call.metrics or {}).get("latency_ms", {}).get("speech_to_screen_p90"),
                (call.metrics or {}).get("latency_ms", {}).get("speech_to_hint_p90"),
            )
        ]
        if value is not None
    ]
    return (max(per_call), "slowest call") if per_call else (None, "none")


def rep_stats(calls: list[Call], emails: dict[UUID, str]) -> list[dict[str, Any]]:
    """One row per rep, busiest first. Live calls only, long enough to judge."""
    by_rep: dict[UUID, list[Call]] = defaultdict(list)
    for call in calls:
        if call.mode == "live" and call.duration_minutes >= MIN_MINUTES:
            by_rep[call.user_id].append(call)

    rows = []
    for user_id, rep_calls in by_rep.items():
        metrics = [call.metrics or {} for call in rep_calls]
        heard = [m for m in metrics if (_number((m.get("stt_minutes") or {}).get("them")) or 0) > 0]
        hints = [_number(m.get("hints_shown")) or 0 for m in metrics]
        latency, latency_source = hint_latency(rep_calls)
        talk = [c for c in (conversation(call.transcript or []) for call in rep_calls) if c]
        sentiments: dict[str, int] = defaultdict(int)
        for call in rep_calls:
            sentiment = (call.summary or {}).get("sentiment")
            if isinstance(sentiment, str):
                sentiments[sentiment.lower()] += 1
        minutes = sum(call.duration_minutes for call in rep_calls)
        cost = sum(call.cost_usd or 0 for call in rep_calls)

        def average(key: str, items: list[dict[str, float]] = talk) -> float | None:
            return sum(item[key] for item in items) / len(items) if items else None

        rows.append(
            {
                "email": emails.get(user_id, ""),
                "calls": len(rep_calls),
                "minutes": round(minutes, 1),
                "avg_minutes": round(minutes / len(rep_calls), 1),
                "last_call_at": max(call.created_at for call in rep_calls),
                "prospect_heard_rate": len(heard) / len(rep_calls),
                "hints_per_call": sum(hints) / len(rep_calls),
                "calls_with_hints_rate": sum(1 for h in hints if h > 0) / len(rep_calls),
                "hint_p90_ms": latency,
                "hint_p90_source": latency_source,
                "calls_with_transcript": len(talk),
                "talk_ratio": average("talk_ratio"),
                "longest_monologue_words": average("longest_monologue_words"),
                "questions_per_call": average("questions"),
                "switches_per_call": average("switches"),
                "sentiment": dict(sentiments),
                "cost_usd": round(cost, 2),
                "cost_per_hour_usd": round(cost / (minutes / 60), 2) if minutes else None,
            }
        )
    return sorted(rows, key=lambda row: row["calls"], reverse=True)


@router.get("/reps")
def reps(
    days: int = Query(default=30, ge=1, le=365),
    user: User = Depends(admin),
    db: Session = Depends(session),
) -> dict[str, Any]:
    """Each rep's numbers over the last `days` days."""
    since = now() - timedelta(days=days)
    calls = list(
        db.exec(
            select(Call).where(Call.workspace_id == user.workspace_id, Call.created_at >= since)
        ).all()
    )
    members = db.exec(select(User).where(User.workspace_id == user.workspace_id)).all()
    return {"days": days, "reps": rep_stats(calls, {m.id: m.email for m in members})}
