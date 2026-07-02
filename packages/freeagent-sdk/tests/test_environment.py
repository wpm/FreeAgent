from typing import Self

from freeagent.sdk import Agent
from freeagent.sdk.message import Message


class Product(Message):
    x: float
    y: float
    x_times_y: float | None = None

    def __call__(self) -> Self:
        return self.model_copy(update={"x_times_y": self.x * self.y})

    def __str__(self):
        if self.x_times_y is not None:
            s = f" = {self.x_times_y}"
        else:
            s = ""
        return f"{self.x} · {self.y}{s}"


class Multiplier(Agent):
    async def process_message(self, message: Product) -> None:
        message()
        return await super().process_message(message)
