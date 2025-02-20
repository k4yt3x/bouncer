#!/usr/bin/env python3
# -*- coding: utf-8 -*-

from google import genai
from google.genai.types import GenerateContentConfig

from .generative_ai import GenerationError, GenerativeAI


class GeminiGenerativeAI(GenerativeAI):
    def __init__(self, api_key: str, model: str, options: dict):
        self.api_key = api_key
        self.model = model
        self.options = options
        self.client = genai.Client(api_key=self.api_key)

    async def generate(self, prompt: str) -> str:
        response = await self.client.aio.models.generate_content(
            model=self.model,
            contents=prompt,
            config=GenerateContentConfig(**self.options),
        )

        text = response.text
        if text is None:
            raise GenerationError("Gemini returned empty text")

        return text.strip()
