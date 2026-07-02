from fixtures import Product
from freeagent.sdk import Agent


class Multiplier(Agent):
    async def process_message(self, message: Product) -> Product:
        return message()
