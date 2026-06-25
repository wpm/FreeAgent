"""The shared manifest work queue (ADR-0005).

ADR-0005 replaces "the runner spawns this episode's children itself" with "the
recruiter *enqueues* the episode's manifests and a pool of workers pulls them
off a shared queue." A single work-queue-retention JetStream stream holds every
episode's manifests, and all workers bind one shared durable pull consumer to
it. A manifest is acknowledged off the queue once a worker takes ownership of
launching it, so each unit of work is delivered to exactly one worker.

This module is the single place the queue's *names* live, so the recruiter
(#53, the producer) and the worker (#52, the consumer) agree on the stream and
subjects without importing each other:

* :data:`WORK_QUEUE_STREAM` -- the one stream name, shared by every app.
* :data:`WORK_QUEUE_SUBJECT_PREFIX` -- the subject token prefix every manifest
  is published under.
* :data:`WORK_QUEUE_SUBJECTS` -- the wildcard the stream captures (and the
  shared consumer binds), covering every app's manifests.
* :data:`WORK_QUEUE_CONSUMER` -- the one shared durable pull consumer name every
  worker binds (never a per-instance consumer).
* :func:`work_subject` -- the per-episode subject one episode's manifests are
  published to, ``freeagent.work.<app>.<episode_id>``. Per-episode tokens keep
  the wire legible and let a debugger watch one episode without filtering, while
  the shared consumer still reads them all through :data:`WORK_QUEUE_SUBJECTS`.

The retention is **work-queue**: a message is removed once acknowledged, so the
queue is a hand-off, not a log. (Episode *content* still lives in each episode's
own limits-retention stream; this queue carries only the launch manifests.)
"""

from __future__ import annotations

from freeagent.subjects import validate_name

#: The one JetStream stream holding every app's manifests. Work-queue
#: retention: an acknowledged manifest is removed, so the stream is a hand-off
#: queue, not a durable log.
WORK_QUEUE_STREAM = "freeagent_work"

#: The subject-token prefix every manifest is published under. A concrete
#: subject is ``freeagent.work.<app>.<episode_id>`` (see :func:`work_subject`).
WORK_QUEUE_SUBJECT_PREFIX = "freeagent.work"

#: The subjects :data:`WORK_QUEUE_STREAM` captures: a single wildcard covering
#: every app's per-episode manifest subjects. The shared pull consumer binds
#: this same wildcard, so one consumer drains every episode's manifests.
WORK_QUEUE_SUBJECTS: tuple[str, ...] = (f"{WORK_QUEUE_SUBJECT_PREFIX}.>",)

#: The ONE shared durable pull consumer every worker binds (ADR-0005). Never a
#: per-instance consumer: every worker binding this same durable name means a
#: manifest is delivered to exactly one of them, so two workers each run a unit
#: of work at most once. A per-instance consumer would instead fan every
#: manifest to every worker and launch each role N times.
WORK_QUEUE_CONSUMER = "freeagent_workers"


def work_subject(app: str, episode_id: str) -> str:
    """The work-queue subject one episode's manifests are published to.

    ``freeagent.work.<app>.<episode_id>``: a per-episode token under
    :data:`WORK_QUEUE_SUBJECT_PREFIX` so an operator can watch one episode's
    enqueue without filtering, while the shared consumer still reads every
    episode through :data:`WORK_QUEUE_SUBJECTS`.
    """
    validate_name(app, kind="application name")
    validate_name(episode_id, kind="episode id")
    return f"{WORK_QUEUE_SUBJECT_PREFIX}.{app}.{episode_id}"
