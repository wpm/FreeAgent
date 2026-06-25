"""The durable episode record, read from JetStream (ADR-0003).

In the atemporal model the **JetStream streams are the source of truth** for
which episodes exist and what state they are in -- not an in-memory registry.
:class:`EpisodeStore` is the read/write view onto that durable record: it
enumerates episode streams, decodes each one's :class:`~freeagent.EpisodeMetadata`
and sealed state, and supports rename (metadata) and delete (whole stream).

Because the truth lives in JetStream, a service restart loses nothing: a fresh
store over the same NATS sees every episode exactly as before. The store is the
service's single JetStream client for the durable record; the live feed (#42)
reads the same streams.
"""

from __future__ import annotations

from dataclasses import dataclass
from datetime import UTC, datetime
from typing import TYPE_CHECKING

from freeagent.metadata import KEY_NAME, EpisodeMetadata
from freeagent.names import fallback_episode_name
from freeagent.subjects import stream_name

if TYPE_CHECKING:
    from collections.abc import Callable

    from freeagent.transport import Transport


@dataclass(slots=True)
class EpisodeRecord:
    """One episode as it exists in the durable record (its stream)."""

    episode_id: str
    app: str
    application: str
    name: str
    status: str
    sealed: bool
    outcome: str | None
    mode: str
    created_at: datetime | None


def _parse_created_at(value: str | None) -> datetime | None:
    if not value:
        return None
    try:
        return datetime.fromisoformat(value)
    except ValueError:
        return None


class EpisodeStore:
    """Read/write the durable episode record (JetStream streams + metadata).

    *transport* is a connected :class:`~freeagent.transport.Transport` (the
    service's JetStream client). *application_of* maps an undashed subject-prefix
    ``app`` to its dashed REST application name (falling back to the app itself
    for an installed-app-less import).
    """

    def __init__(self, transport: Transport, *, application_of: Callable[[str], str]) -> None:
        self._transport = transport
        self._application_of = application_of

    async def list(self) -> list[EpisodeRecord]:
        """Every episode in the durable record, newest first.

        Streams without FreeAgent episode metadata are not episodes (or predate
        the metadata) and are skipped, so enumerating never trips over a stranger.
        """
        records: list[EpisodeRecord] = []
        for name in await self._transport.list_streams():
            metadata = await self._transport.stream_metadata(name)
            decoded = EpisodeMetadata.from_stream_metadata(metadata)
            if decoded is None:
                continue
            sealed = await self._transport.stream_sealed(name)
            records.append(self._record(name, decoded, sealed))
        epoch = datetime.min.replace(tzinfo=UTC)
        records.sort(key=lambda r: r.created_at or epoch, reverse=True)
        return records

    async def get(self, app: str, episode_id: str) -> EpisodeRecord | None:
        """The durable record for one episode, or ``None`` if no such stream."""
        name = stream_name(app, episode_id)
        try:
            metadata = await self._transport.stream_metadata(name)
        except Exception:
            return None
        decoded = EpisodeMetadata.from_stream_metadata(metadata)
        if decoded is None:
            return None
        sealed = await self._transport.stream_sealed(name)
        return self._record(name, decoded, sealed)

    async def rename(self, app: str, episode_id: str, name: str) -> None:
        """Set the episode's friendly name on its stream metadata."""
        await self._transport.set_stream_metadata(stream_name(app, episode_id), {KEY_NAME: name})

    async def delete(self, app: str, episode_id: str) -> None:
        """Delete the whole episode (its stream and every message)."""
        await self._transport.delete_stream(stream_name(app, episode_id))

    def _record(self, stream: str, metadata: EpisodeMetadata, sealed: bool) -> EpisodeRecord:
        episode_id = stream.removeprefix(f"{metadata.app}_episode_")
        return EpisodeRecord(
            episode_id=episode_id,
            app=metadata.app,
            application=self._application_of(metadata.app),
            name=metadata.name or fallback_episode_name(episode_id),
            status=metadata.status,
            sealed=sealed,
            outcome=metadata.outcome,
            mode=metadata.mode,
            created_at=_parse_created_at(metadata.created_at),
        )
