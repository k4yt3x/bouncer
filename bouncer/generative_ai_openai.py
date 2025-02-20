#!/usr/bin/env python3
# -*- coding: utf-8 -*-

from openai import AsyncOpenAI

from .generative_ai import GenerationError, GenerativeAI


class OpenAIGenerativeAI(GenerativeAI):
    def __init__(self, api_key: str, model: str, options: dict):
        self.api_key = api_key
        self.model = model
        self.options = options
        self.client = AsyncOpenAI(api_key=api_key)

    async def generate(self, prompt: str) -> str:
        response = await self.client.chat.completions.create(
            model=self.model,
            messages=[{"role": "user", "content": prompt}],
            **self.options,
        )

        if (
            response.choices is None
            or len(response.choices) == 0
            or response.choices[0].message.content is None
        ):
            raise GenerationError("Gemini returned empty text")

        return response.choices[0].message.content
