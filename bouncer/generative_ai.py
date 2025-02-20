#!/usr/bin/env python3
# -*- coding: utf-8 -*-
from abc import ABC, abstractmethod


class GenerationError(Exception):
    def __init__(self, message: str):
        self.message = message


class GenerativeAI(ABC):
    @abstractmethod
    async def generate(self, prompt: str) -> str:
        pass
