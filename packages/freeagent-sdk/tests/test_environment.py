from typing import cast

from fixtures import Product
from freeagent.sdk import Agent
from freeagent.sdk.message import Message


class Multiplier(Agent):
    async def process_message(self, message: Message) -> Product:
        return cast(Product, message)()
