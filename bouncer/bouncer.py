#!/usr/bin/env python3
# -*- coding: utf-8 -*-

import asyncio
import contextlib
from dataclasses import dataclass
from datetime import datetime, timezone

from loguru import logger
from telegram import Bot, Update
from telegram.ext import (
    ApplicationBuilder,
    CallbackContext,
    ChatJoinRequestHandler,
    MessageHandler,
    filters,
)

from .database_manager import DatabaseManager
from .generative_ai import GenerativeAI


@dataclass
class BotMessages:
    internal_error: str = (
        "An internal error occurred. Please notify the admin or try again later."
    )
    join_requested: str = (
        "Hi {}! You have requested to join {}.\nBefore I can approve your request, "
        "please answer this question:\n\n{}\n\nReply with the correct answer. "
        "You have {} seconds."
    )
    correct_answer: str = "✅ Correct! You have been approved to join the group."
    wrong_answer: str = (
        "❌ Wrong answer! Your request has been declined. "
        "Please try again in {} seconds."
    )
    timed_out: str = (
        "⏰ Your challenge attempt has timed out. Please try again in {} seconds."
    )
    retry_timer: str = (
        "Please wait for {} seconds before trying to join the group again."
    )
    no_challenge: str = "I don't have any active challenges for you."


@dataclass
class PromptTemplates:
    generate_challenge: str = "Generate a challenge for the topic: {}."
    verify_answer: str = (
        'Given this challenge: "{}". Is this a valid challenge? '
        'Reply either "true" or "false".'
    )


class Bouncer:
    def __init__(
        self,
        telegram_token: str,
        generative_ai: GenerativeAI,
        bot_messages: BotMessages,
        prompt_templates: PromptTemplates,
        answer_timeout: int = 120,
        retry_timeout: int = 600,
    ) -> None:
        self.application = ApplicationBuilder().token(telegram_token).build()
        self.bot_messages = bot_messages
        self.prompt_templates = prompt_templates
        self.answer_timeout = answer_timeout
        self.retry_timeout = retry_timeout
        self.generative_ai = generative_ai
        self.database = DatabaseManager()
        self.application.add_error_handler(self._error_handler)
        self.application.add_handler(ChatJoinRequestHandler(self._send_challenge))
        self.application.add_handler(
            MessageHandler(
                filters.TEXT & ~filters.COMMAND & filters.ChatType.PRIVATE,
                self._check_answer,
            )
        )

    def run(self) -> int:
        logger.info("Starting Bouncer polling loop")
        self.application.run_polling()
        return 1

    async def _error_handler(self, _: Update, context: CallbackContext):
        logger.error(f"An error occurred: {context.error}", exc_info=True)

    async def _send_challenge(self, update: Update, context: CallbackContext):
        """Sends a challenge question to the user when they request to join."""
        chat_join_request = update.chat_join_request
        if chat_join_request is None:
            return

        user_id = chat_join_request.from_user.id
        chat_id = chat_join_request.chat.id
        group_name = chat_join_request.chat.title
        timestamp = datetime.now(timezone.utc).timestamp()

        full_name = chat_join_request.from_user.first_name
        if chat_join_request.from_user.last_name:
            full_name += " " + chat_join_request.from_user.last_name

        if not self.database.is_group_allowed(chat_id):
            logger.warning(
                f"Ignored join request: Group {group_name} ({chat_id}) is not allowed"
            )
            return

        try:
            logger.info(
                f"Received chat join request from {full_name} ({user_id}) in {chat_id}"
            )

            # Check if user is already in pending list
            if self.database.is_user_pending(user_id):
                logger.warning(
                    f"Ignored join request: User {full_name} "
                    "is already in the pending list"
                )
                return

            # Check if user's last failed attempt was less than 10 minutes ago
            last_join_attempt_time = self.database.get_last_join_attempt(user_id)
            if last_join_attempt_time is not None:
                # Calculate time since last attempt
                seconds_since_last_attempt = (
                    datetime.now(timezone.utc) - last_join_attempt_time
                ).total_seconds()

                # Decline request if last attempt was less than retry timeout
                if seconds_since_last_attempt < self.retry_timeout:
                    logger.warning(
                        f"Declined join request: user {full_name} has a failed attempt "
                        f"less than {self.retry_timeout} seconds ago "
                        f"({seconds_since_last_attempt}s)"
                    )
                    await context.bot.decline_chat_join_request(
                        chat_id=chat_id, user_id=user_id
                    )
                    await context.bot.send_message(
                        chat_id=user_id,
                        text=self.bot_messages.retry_timer.format(
                            self.retry_timeout,
                            int(self.retry_timeout - seconds_since_last_attempt),
                        ),
                    )
                    return

            group_topic = self.database.get_group_topic(chat_id)
            if group_topic is None:
                logger.error(
                    f"Ignored join request: Group {group_name} ({chat_id}) "
                    "has no topic set"
                )
                return

            # Generate a challenge
            logger.info(f"Generating challenge for {full_name} in {chat_id}")
            challenge = await self.generate_challenge(group_topic)

            # Store user in pending list
            self.database.store_pending_user(
                int(timestamp), chat_id, user_id, challenge
            )

            # Send the challenge to the user
            await context.bot.send_message(
                chat_id=user_id,
                text=self.bot_messages.join_requested.format(
                    full_name, group_name, challenge, self.answer_timeout
                ),
            )

            self.database.store_join_attempt(
                int(datetime.now(timezone.utc).timestamp()), user_id
            )

            asyncio.create_task(
                self._timeout_user(
                    chat_id, user_id, full_name, int(timestamp), context.bot
                )
            )
        except Exception as e:  # pylint: disable=broad-except
            logger.exception(e)

            with contextlib.suppress(Exception):
                await context.bot.send_message(
                    chat_id=user_id,
                    text=self.bot_messages.internal_error,
                )

    async def _timeout_user(
        self, chat_id: int, user_id: int, full_name: str, timestamp: int, bot: Bot
    ):
        await asyncio.sleep(self.answer_timeout)
        pending_user = self.database.get_pending_user(user_id)
        if pending_user and int(pending_user[0].timestamp()) == timestamp:
            logger.warning(
                f"User {full_name} ({user_id}) timed out after "
                f"{self.answer_timeout} seconds"
            )
            self.database.remove_pending_user(chat_id, user_id)
            await bot.decline_chat_join_request(chat_id=chat_id, user_id=user_id)
            await bot.send_message(
                chat_id=user_id,
                text=self.bot_messages.timed_out.format(self.retry_timeout),
            )

    async def _check_answer(self, update: Update, context: CallbackContext):
        """Checks if the user's response is correct."""
        message = update.message
        if message is None or message.text is None:
            return

        from_user = message.from_user
        if from_user is None:
            return

        user_id = from_user.id
        full_name = from_user.full_name
        answer = message.text.strip()

        logger.debug(f"User {full_name} ({user_id}) answered: {answer}")

        if self.database.is_user_pending(user_id):
            pending_user = self.database.get_pending_user(user_id)
            if pending_user is None:
                return

            # Unpack pending user data
            timestamp, chat_id, user_id, challenge = pending_user

            # Check if the answer has exceeded the time limit
            if (
                datetime.now(timezone.utc) - timestamp
            ).total_seconds() > self.answer_timeout:
                logger.warning(
                    f"Declined join request: User {full_name} ({user_id}) "
                    "took too long to answer"
                )
                self.database.remove_pending_user(chat_id, user_id)
                await context.bot.decline_chat_join_request(
                    chat_id=chat_id, user_id=user_id
                )
                await message.reply_text(self.bot_messages.no_challenge)
                return

            if await self.verify_answer(challenge, answer):
                await context.bot.approve_chat_join_request(
                    chat_id=chat_id, user_id=user_id
                )
                await message.reply_text(self.bot_messages.correct_answer)
            else:
                await context.bot.decline_chat_join_request(
                    chat_id=chat_id, user_id=user_id
                )
                await message.reply_text(
                    self.bot_messages.wrong_answer.format(self.retry_timeout)
                )

            # Remove user from pending list
            self.database.remove_pending_user(chat_id, user_id)
        else:
            await message.reply_text(self.bot_messages.no_challenge)

    async def generate_challenge(self, topic: str) -> str:
        response = await self.generative_ai.generate(
            self.prompt_templates.generate_challenge.format(topic)
        )
        logger.debug(f"Challenge generation response: {response}")
        return response

    async def verify_answer(self, challenge: str, answer: str) -> bool:
        response = await self.generative_ai.generate(
            self.prompt_templates.verify_answer.format(challenge, answer)
        )
        logger.debug(f"Challenge verification response: {response}")
        if response == "verification_passed":
            return True
        return False
