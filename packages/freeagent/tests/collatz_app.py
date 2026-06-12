"""The Collatz test application: deterministic, no-LLM, exercising the full stack.

Two agents, ``even`` and ``odd``, exchange numbers on the public channel and
apply Collatz steps (n even -> n // 2; n odd -> 3n + 1) until 1 is reached.
Responsibility is a rule, not a turn-scheduler: the agent named ``even``
steps even numbers, the agent named ``odd`` steps odd numbers.

Every step goes through the think queue: ``perceive`` posts the number to the
agent's own event stream, and ``handle_think`` computes the step and ``act``s
the result. When an agent's result is again its own responsibility (e.g. the
tail 16 -> 8 -> 4 -> 2 -> 1, all even) it re-enters its own think queue,
because an agent never perceives its own broadcast. The agent that computes 1
broadcasts it and reports ``{"type": "done", "final": 1}`` to the
environment's inbox; the environment reacts by initiating shutdown. On the
``shutdown`` control message each agent broadcasts a final
``{"type": "bye"}`` inside the stopping grace period.
"""

from __future__ import annotations

from typing import Any

from freeagent import Agent, Envelope, Environment

EVEN = "even"
ODD = "odd"


def collatz_step(n: int) -> int:
    """One Collatz step: n even -> n // 2; n odd -> 3n + 1."""
    return n // 2 if n % 2 == 0 else 3 * n + 1


def responsible_agent(n: int) -> str:
    """The agent that steps *n*: ``even`` for even numbers, ``odd`` for odd."""
    return EVEN if n % 2 == 0 else ODD


def trajectory(seed: int) -> list[int]:
    """The full broadcast number sequence for *seed*, ending at 1."""
    numbers = [seed]
    while numbers[-1] != 1:
        numbers.append(collatz_step(numbers[-1]))
    return numbers


def expected_senders(seed: int, starter: str) -> list[str]:
    """The sender of each broadcast number in :func:`trajectory` order.

    The starter broadcasts the seed; every subsequent number is broadcast by
    the agent responsible for its predecessor.
    """
    return [starter] + [responsible_agent(n) for n in trajectory(seed)[:-1]]


class CollatzAgent(Agent):
    """One Collatz participant. In-world payloads: ``{"type": "number", "value": n}``.

    Config (beyond the framework's ``grace_period``): ``starter`` (bool, this
    agent broadcasts the seed on ``start``) and ``seed`` (int, required when
    ``starter``).
    """

    async def control(self, message: Envelope) -> None:
        await super().control(message)
        mtype = message.payload.get("type") if isinstance(message.payload, dict) else None
        if mtype == "start" and self.config.get("starter", False):
            self.think({"type": "kickoff", "value": int(self.config["seed"])})
        elif mtype == "shutdown":
            # The trailing goodbye: flushed inside the stopping grace period.
            self.act({"type": "bye"})

    async def perceive(self, message: Envelope) -> None:
        payload = message.payload
        if not (isinstance(payload, dict) and payload.get("type") == "number"):
            return
        value = payload.get("value")
        if not isinstance(value, int) or value == 1:
            return  # 1 is terminal; whoever computed it already told the env
        if responsible_agent(value) == self.id:
            self.think({"type": "step", "value": value})

    async def handle_think(self, payload: Any) -> None:
        if not isinstance(payload, dict):
            return
        if payload["type"] == "kickoff":
            value: int = payload["value"]
            self.act({"type": "number", "value": value})
            # An agent never perceives its own broadcast: if the seed is the
            # starter's own responsibility, keep stepping via the think queue.
            if value != 1 and responsible_agent(value) == self.id:
                self.think({"type": "step", "value": value})
        elif payload["type"] == "step":
            result = collatz_step(payload["value"])
            self.act({"type": "number", "value": result})
            if result == 1:
                self.act({"type": "done", "final": 1}, recipients=["env"])
            elif responsible_agent(result) == self.id:
                self.think({"type": "step", "value": result})


class CollatzEnvironment(Environment):
    """Minimal environment: shuts down when an agent reports 1 on the env inbox."""

    async def perceive(self, message: Envelope) -> None:
        payload = message.payload
        if isinstance(payload, dict) and payload.get("type") == "done":
            self.initiate_shutdown("collatz reached 1")
