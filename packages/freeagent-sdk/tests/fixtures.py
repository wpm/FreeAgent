from typing import Self

from freeagent.sdk.message import Message


class Product(Message):
    x: float
    y: float
    x_times_y: float | None = None

    def __call__(self) -> Self:
        return self.model_copy(update={"x_times_y": self.x * self.y})

    def __str__(self) -> str:
        if self.x_times_y is not None:
            s = f" = {self.x_times_y}"
        else:
            s = ""
        return f"{self.x} · {self.y}{s}"
