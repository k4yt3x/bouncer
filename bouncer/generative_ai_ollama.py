#!/usr/bin/env python3
# -*- coding: utf-8 -*-

from ollama import AsyncClient

from .generative_ai import GenerationError, GenerativeAI


class OllamaGenerativeAI(GenerativeAI):
    def __init__(self, model: str, options: dict):
        self.model = model
        self.options = options

    async def generate(self, prompt: str) -> str:
        response = await AsyncClient().chat(
            model=self.model,
            options=self.options,
            messages=[
                {
                    "role": "user",
                    "content": prompt,
                }
            ],
        )

        message = response.get("message")
        if message is None:
            raise GenerationError("Ollama returned empty message")

        content = message.get("content")
        if content is None:
            raise GenerationError("Ollama returned empty content")

        return content.strip()
