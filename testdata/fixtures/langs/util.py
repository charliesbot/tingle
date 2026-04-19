import os
from typing import List

from .models import User


class Service:
    def __init__(self, user: User):
        self.user = user

    def greet(self) -> str:
        return f"hello {self.user.name}"


def read_lines(path: str) -> List[str]:
    with open(path) as f:
        return f.readlines()


async def fetch_data(url: str) -> dict:
    return {"url": url}
