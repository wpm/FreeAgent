from fixtures import Product
from freeagent.sdk import Agent
from freeagent.sdk.message import Message


class Multiplier(Agent):
    async def process_message(self, message: Message) -> Product:
        assert isinstance(message, Product)
        return message()
