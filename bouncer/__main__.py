#!/usr/bin/env python3
# -*- coding: utf-8 -*-

import argparse
import sys
from pathlib import Path

import yaml
from loguru import logger

from . import __version__
from .bouncer import BotMessages, Bouncer, PromptTemplates
from .generative_ai import GenerativeAI
from .generative_ai_gemini import GeminiGenerativeAI
from .generative_ai_ollama import OllamaGenerativeAI
from .generative_ai_openai import OpenAIGenerativeAI

LOGURU_FORMAT = (
    "<green>{time:HH:mm:ss.SSSSSS!UTC}</green> | "
    "<level>{level: <8}</level> | "
    "<level>{message}</level>"
)


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(prog="bouncer", description="Run Bouncer bot")

    parser.add_argument(
        "-c",
        "--config",
        default="bouncer.yaml",
        help="Path to the Bouncer configuration file",
    )

    return parser.parse_args()


def load_config(config_path: Path) -> dict | None:
    if config_path.is_file():
        logger.debug(f"Loading configuration from {config_path}")
        with config_path.open("r") as bouncer_config_file:
            return yaml.safe_load(bouncer_config_file)
    else:
        logger.warning(f"Configuration file not found at {config_path}")
        return None


def main() -> int:
    # Set up logging
    logger.remove()
    logger.add(sys.stderr, colorize=True, format=LOGURU_FORMAT)

    logger.info(f"Starting Bouncer version {__version__}")

    # Parse command-line arguments
    args = parse_args()

    # Read configuration file
    bouncer_config = load_config(Path(args.config))
    if bouncer_config is None:
        logger.error("Failed to load configuration file")
        return 1

    telegram_bot_token = bouncer_config.get("telegram_bot_token")
    if telegram_bot_token is None:
        logger.error("Telegram bot token not found in the configuration file")
        return 1

    generative_ai_backend = bouncer_config.get("generative_ai_backend")

    generative_ai: GenerativeAI | None = None
    if generative_ai_backend is None:
        logger.error("Generative AI backend not specified in the configuration file")
        return 1

    if generative_ai_backend == "openai":
        logger.debug("Using OpenAI as the generative AI backend")
        openai_config = bouncer_config.get("openai")
        if openai_config is None:
            logger.error("OpenAI configuration not found in the configuration file")
            return 1

        openai_api_key = openai_config.get("api_key")
        if openai_api_key is None:
            logger.error("OpenAI API key not found in the configuration file")
            return 1

        openai_model = openai_config.get("model")
        if openai_model is None:
            logger.error("OpenAI model not found in the configuration file")
            return 1

        openai_options = openai_config.get("options")

        generative_ai = OpenAIGenerativeAI(openai_api_key, openai_model, openai_options)

    if generative_ai_backend == "ollama":
        logger.debug("Using Ollama as the generative AI backend")
        ollama_config = bouncer_config.get("ollama")
        if ollama_config is None:
            logger.error("Ollama configuration not found in the configuration file")
            return 1

        ollama_model = ollama_config.get("model")
        if ollama_model is None:
            logger.error("Ollama model not found in the configuration file")
            return 1

        ollama_options = ollama_config.get("options")

        generative_ai = OllamaGenerativeAI(ollama_model, ollama_options)

    if generative_ai_backend == "gemini":
        logger.debug("Using Gemini as the generative AI backend")
        gemini_config = bouncer_config.get("gemini")
        if gemini_config is None:
            logger.error("Gemini configuration not found in the configuration file")
            return 1

        gemini_api_key = gemini_config.get("api_key")
        if gemini_api_key is None:
            logger.error("Gemini API key not found in the configuration file")
            return 1

        gemini_model = gemini_config.get("model")
        if gemini_model is None:
            logger.error("Gemini model not found in the configuration file")
            return 1

        gemini_options = gemini_config.get("options")

        generative_ai = GeminiGenerativeAI(gemini_api_key, gemini_model, gemini_options)

    if generative_ai is None:
        logger.error(f"Unsupported generative AI backend: {generative_ai_backend}")
        return 1

    messages_config = bouncer_config.get("messages")
    if messages_config is None or not isinstance(messages_config, dict):
        logger.error(
            "Messages configuration not found or invalid in the configuration file"
        )
        return 1

    bot_messages = BotMessages(
        internal_error=messages_config["internal_error"],
        join_requested=messages_config["join_requested"],
        correct_answer=messages_config["correct_answer"],
        wrong_answer=messages_config["wrong_answer"],
        timed_out=messages_config["timed_out"],
        ongoing_challenge=messages_config["ongoing_challenge"],
        no_challenge=messages_config["no_challenge"],
        retry_timer=messages_config["retry_timer"],
    )

    # Verify all message fields are populated
    for field_name, field_value in vars(bot_messages).items():
        if field_value is None:
            logger.error(f"Required message '{field_name}' not found in configuration")
            return 1

    prompts_config = bouncer_config.get("prompts")
    if prompts_config is None or not isinstance(prompts_config, dict):
        logger.error(
            "Prompts configuration not found or invalid in the configuration file"
        )
        return 1

    prompt_templates = PromptTemplates(
        generate_challenge=prompts_config["generate_challenge"],
        verify_answer=prompts_config["verify_answer"],
    )

    answer_timeout = bouncer_config.get("answer_timeout", 120)
    retry_timeout = bouncer_config.get("retry_timeout", 600)

    # Initialize Bouncer with dynamic ollama options
    bouncer = Bouncer(
        telegram_token=telegram_bot_token,
        generative_ai=generative_ai,
        bot_messages=bot_messages,
        prompt_templates=prompt_templates,
        answer_timeout=answer_timeout,
        retry_timeout=retry_timeout,
    )

    # Run Bouncer and start monitoring for messages
    return bouncer.run()


if __name__ == "__main__":
    sys.exit(main())
