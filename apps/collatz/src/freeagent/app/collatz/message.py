"""The one message type Collatz puts on the wire, plus the pure Collatz step it carries.

The environment hands each agent a :class:`Chain` -- a list of integers, initially a single starting
number -- and the agent returns it extended by exactly one Collatz step. Environment and agent share
this single message type; direction is carried by the NATS subject, not the type (the
ack-then-counter-request shape). Keeping the step function here, beside the message it extends, lets
the unit tests exercise the whole of Collatz's domain logic against constructed :class:`Chain`
objects without a NATS server; see :mod:`freeagent.app.collatz.entity` for the entities that move
these messages over the wire.
"""

from __future__ import annotations

from freeagent.sdk.message import Message


class Chain(Message):
    """A Collatz chain in progress: the sequence of numbers produced so far.

    Starts as a single starting number and grows by one :func:`collatz_step` each time an agent
    extends it. The chain is *complete* once it reaches 1 (see :func:`is_complete`), the fixed point
    of the Collatz map -- the environment watches for that to know when an agent is finished.

    :ivar numbers: The chain so far, oldest first; never empty in normal use. Its last element is
        the current value the next step extends.
    """

    numbers: list[int]


def collatz_step(n: int) -> int:
    """Apply one step of the Collatz map to ``n``.

    The Collatz map sends an even number to half itself and an odd number to ``3n + 1``. Iterating
    it from any positive integer is conjectured to always reach 1; Collatz exists here to rehearse
    the platform's messaging shape on logic simple enough to verify by hand, not to test that
    conjecture.

    :param n: The number to step; must be a positive integer.
    :return: ``n // 2`` if ``n`` is even, else ``3 * n + 1``.
    :raises ValueError: If ``n`` is not a positive integer.
    """
    if n < 1:
        raise ValueError(f"Collatz is defined on positive integers; got {n}")
    return n // 2 if n % 2 == 0 else 3 * n + 1


def extend(chain: Chain) -> Chain:
    """Return a new :class:`Chain` with one more :func:`collatz_step` appended.

    Pure: the input chain is left untouched and a fresh :class:`Chain` is returned, so the same
    incoming message can be safely inspected after the extension is computed.

    :param chain: The chain to extend; must be non-empty.
    :return: A new chain whose numbers are ``chain.numbers`` plus the next Collatz value.
    :raises ValueError: If ``chain`` is empty (there is no current value to step from).
    """
    if not chain.numbers:
        raise ValueError("Cannot extend an empty chain: there is no current value to step from")
    return Chain(numbers=[*chain.numbers, collatz_step(chain.numbers[-1])])


def is_complete(chain: Chain) -> bool:
    """Report whether a chain has reached the Collatz fixed point and is therefore finished.

    A chain is complete once its current (last) value is 1. An empty chain is never complete: it
    carries no value to have reached the fixed point.

    :param chain: The chain to judge.
    :return: ``True`` if the chain's last value is 1, else ``False``.
    """
    return bool(chain.numbers) and chain.numbers[-1] == 1
