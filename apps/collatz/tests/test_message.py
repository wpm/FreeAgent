"""Unit tests for :mod:`freeagent.app.collatz.message`: the pure Collatz step and chain helpers.

These exercise the whole of Collatz's step logic against constructed :class:`Chain` objects, with no
NATS server -- the acceptance criterion that the Collatz step, completion detection, and chain
extension are correct in isolation.
"""

from __future__ import annotations

import pytest
from freeagent.app.collatz.message import Chain, collatz_step, extend, is_complete


@pytest.mark.parametrize(
    ("n", "expected"),
    [
        (1, 4),  # odd fixed point still steps: 3*1 + 1
        (2, 1),  # even
        (3, 10),  # odd
        (4, 2),  # even
        (6, 3),  # even
        (27, 82),  # odd, the classic long-running start
    ],
)
def test_collatz_step_maps_each_number_to_its_successor(n: int, expected: int) -> None:
    assert collatz_step(n) == expected


def test_collatz_step_halves_even_numbers() -> None:
    assert collatz_step(1024) == 512


def test_collatz_step_triples_and_increments_odd_numbers() -> None:
    assert collatz_step(7) == 22


@pytest.mark.parametrize("n", [0, -1, -27])
def test_collatz_step_rejects_non_positive_numbers(n: int) -> None:
    with pytest.raises(ValueError, match="positive"):
        collatz_step(n)


def test_extend_appends_exactly_one_step() -> None:
    chain = Chain(numbers=[3])

    assert extend(chain).numbers == [3, 10]


def test_extend_leaves_the_input_chain_untouched() -> None:
    chain = Chain(numbers=[6, 3])

    extend(chain)

    assert chain.numbers == [6, 3]


def test_repeated_extension_reproduces_a_known_collatz_sequence() -> None:
    chain = Chain(numbers=[6])
    while not is_complete(chain):
        chain = extend(chain)

    # The canonical Collatz sequence from 6 down to the 4-2-1 fixed cycle.
    assert chain.numbers == [6, 3, 10, 5, 16, 8, 4, 2, 1]


def test_extend_rejects_an_empty_chain() -> None:
    with pytest.raises(ValueError, match="empty"):
        extend(Chain(numbers=[]))


def test_is_complete_is_true_once_the_chain_reaches_one() -> None:
    assert is_complete(Chain(numbers=[4, 2, 1]))


def test_is_complete_is_false_while_the_chain_is_still_running() -> None:
    assert not is_complete(Chain(numbers=[4, 2]))


def test_is_complete_is_false_for_an_empty_chain() -> None:
    assert not is_complete(Chain(numbers=[]))


def test_chain_round_trips_through_json() -> None:
    chain = Chain(numbers=[27, 82, 41])

    decoded = Chain.model_validate_json(chain.to_bytes())

    assert isinstance(decoded, Chain)
    assert decoded.numbers == [27, 82, 41]
