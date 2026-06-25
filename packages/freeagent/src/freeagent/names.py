"""Auto-generated friendly names for episodes.

An episode is a named object (ADR-0003). Its friendly name has three sources,
in increasing specificity: an **auto-generated fallback** (here), a **user-set**
name (rename), and an **app-contributed** name (Twenty Questions titles a
finished game by its secret). This module supplies the fallback: a short,
memorable ``adjective-animal`` label derived deterministically from the episode
id, so the same episode always gets the same readable name without any state.
"""

from __future__ import annotations

import hashlib

#: Curated adjectives and animals, kept short and unambiguous. Deterministic
#: selection by episode-id hash gives a stable, memorable name per episode.
_ADJECTIVES: tuple[str, ...] = (
    "amber",
    "brave",
    "calm",
    "clever",
    "cosmic",
    "crimson",
    "dapper",
    "eager",
    "fuzzy",
    "gentle",
    "golden",
    "happy",
    "jolly",
    "keen",
    "lucky",
    "mellow",
    "nimble",
    "noble",
    "plucky",
    "quiet",
    "rapid",
    "scarlet",
    "silver",
    "spry",
    "sunny",
    "swift",
    "teal",
    "vivid",
    "witty",
    "zesty",
)
_ANIMALS: tuple[str, ...] = (
    "otter",
    "falcon",
    "lynx",
    "heron",
    "panda",
    "gecko",
    "marten",
    "raven",
    "badger",
    "ibex",
    "tapir",
    "narwhal",
    "quokka",
    "puffin",
    "civet",
    "okapi",
    "dingo",
    "fennec",
    "jackal",
    "kestrel",
    "lemur",
    "magpie",
    "newt",
    "osprey",
    "pika",
    "sable",
    "tern",
    "vole",
    "wombat",
    "yak",
)


def fallback_episode_name(episode_id: str) -> str:
    """A short, memorable ``adjective-animal`` name derived from *episode_id*.

    Deterministic: the same id always yields the same name, so a fallback name
    is stable across restarts and needs nothing stored. Used when neither a user
    nor the application has given the episode a more specific name.
    """
    digest = hashlib.sha256(episode_id.encode("utf-8")).digest()
    adjective = _ADJECTIVES[digest[0] % len(_ADJECTIVES)]
    animal = _ANIMALS[digest[1] % len(_ANIMALS)]
    return f"{adjective}-{animal}"
