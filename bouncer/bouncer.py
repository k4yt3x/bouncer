#!/usr/bin/env python3
# -*- coding: utf-8 -*-

import asyncio
import contextlib
import uuid
from dataclasses import dataclass
from datetime import datetime, timezone

import telegram
from loguru import logger
from telegram import Bot, ChatMember, Update
from telegram.ext import (
    ApplicationBuilder,
    CallbackContext,
    ChatJoinRequestHandler,
    MessageHandler,
    filters,
)
from telegram.ext._handlers.commandhandler import CommandHandler

from .database_manager import DatabaseManager
from .generative_ai import GenerativeAI


@dataclass
class BotMessages:
    internal_error: str
    join_requested: str
    correct_answer: str
    wrong_answer: str
    timed_out: str
    retry_timer: str
    ongoing_challenge: str
    no_challenge: str


@dataclass
class PromptTemplates:
    generate_challenge: str
    verify_answer: str


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
        self.application.add_handler(CommandHandler("settopic", self.set_topic))
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

    async def _error_handler(self, _: object, context: CallbackContext):
        logger.exception(context.error)

    async def _safe_send_message(self, bot: telegram.Bot, *args, **kwargs):
        try:
            return await bot.send_message(*args, **kwargs)
        except telegram.error.Forbidden:
            logger.warning(f"Bot is blocked by the user {kwargs.get('chat_id')}")
        except Exception as e:  # pylint: disable=broad-except
            logger.error(f"Failed to send message: {e}")

    async def set_topic(self, update: Update, context: CallbackContext):
        """Sets the topic of the group."""
        message = update.message
        if message is None or message.text is None:
            return

        chat_id = message.chat_id
        chat_title = message.chat.title
        from_user = message.from_user

        if from_user is None:
            return

        if not self.database.is_group_allowed(chat_id):
            logger.warning(
                f"Ignored topic setting: Group {chat_title} ({chat_id}) is not allowed"
            )
            return

        member = await context.bot.get_chat_member(chat_id, from_user.id)
        if not member.status in [ChatMember.ADMINISTRATOR, ChatMember.OWNER]:
            logger.warning(
                f"Ignored topic setting: User {from_user.full_name} ({from_user.id}) is not an admin"
            )
            return

        try:
            topic = message.text.split(" ", 1)[1].strip()
        except IndexError:
            await message.reply_text("Usage: /settopic <topic>")
            return

        self.database.set_group_topic(chat_id, topic)
        logger.info(f"Set topic for group {chat_title} ({chat_id}): {topic}")
        await message.reply_text(f"Set group topic to: {topic}")

    async def _send_challenge(self, update: Update, context: CallbackContext):
        """Sends a challenge question to the user when they request to join."""
        chat_join_request = update.chat_join_request
        if chat_join_request is None:
            return

        user_id = chat_join_request.from_user.id
        chat_id = chat_join_request.chat.id
        chat_title = chat_join_request.chat.title
        timestamp = datetime.now(timezone.utc).timestamp()

        full_name = chat_join_request.from_user.first_name
        if chat_join_request.from_user.last_name:
            full_name += " " + chat_join_request.from_user.last_name

        if not self.database.is_group_allowed(chat_id):
            logger.warning(
                f"Ignored join request: Group {chat_title} ({chat_id}) is not allowed"
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
                await self._safe_send_message(
                    context.bot,
                    chat_id=user_id,
                    text=self.bot_messages.ongoing_challenge,
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
                    await self._safe_send_message(
                        context.bot,
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
                    f"Ignored join request: Group {chat_title} ({chat_id}) "
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
            try:
                await self._safe_send_message(
                    context.bot,
                    chat_id=user_id,
                    text=self.bot_messages.join_requested.format(
                        full_name, chat_title, challenge, self.answer_timeout
                    ),
                )
            except telegram.error.Forbidden:
                logger.warning(f"Bot is blocked by the user {user_id}")

            self.database.store_join_attempt(
                int(datetime.now(timezone.utc).timestamp()), user_id, full_name
            )

            asyncio.create_task(
                self._timeout_user(
                    chat_id,
                    str(chat_title),
                    user_id,
                    full_name,
                    int(timestamp),
                    context.bot,
                )
            )

        except Exception as e:  # pylint: disable=broad-except
            logger.exception(e)

            with contextlib.suppress(Exception):
                await self._safe_send_message(
                    context.bot,
                    chat_id=user_id,
                    text=self.bot_messages.internal_error,
                )

    async def _timeout_user(
        self,
        chat_id: int,
        chat_title: str,
        user_id: int,
        full_name: str,
        timestamp: int,
        bot: Bot,
    ):
        await asyncio.sleep(self.answer_timeout)
        pending_user = self.database.get_pending_user(user_id)
        if pending_user is not None:
            ts, chat_id, user_id, challenge = pending_user
            if int(ts.timestamp()) == timestamp:
                logger.warning(
                    f"User {full_name} ({user_id}) timed out after "
                    f"{self.answer_timeout} seconds"
                )
                self.database.remove_pending_user(chat_id, user_id)
                self.database.store_verification_attempt(
                    timestamp,
                    chat_id,
                    chat_title,
                    user_id,
                    full_name,
                    challenge,
                    "",
                    "declined",
                    "Timed out",
                )
                await bot.decline_chat_join_request(chat_id=chat_id, user_id=user_id)
                await self._safe_send_message(
                    bot,
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
        chat_title = message.chat.title
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

            # Verify the answer through the LLM
            passed, reason = await self.verify_answer(challenge, answer)

            if passed is True:
                verdict = "accepted"
                await context.bot.approve_chat_join_request(
                    chat_id=chat_id, user_id=user_id
                )
                await message.reply_text(self.bot_messages.correct_answer)
            else:
                verdict = "declined"
                await context.bot.decline_chat_join_request(
                    chat_id=chat_id, user_id=user_id
                )
                await message.reply_text(
                    self.bot_messages.wrong_answer.format(self.retry_timeout)
                )

            # Store verification attempt
            self.database.store_verification_attempt(
                int(timestamp.timestamp()),
                chat_id,
                str(chat_title),
                user_id,
                full_name,
                challenge,
                answer,
                verdict,
                reason,
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

    async def verify_answer(self, challenge: str, answer: str) -> tuple[bool, str]:
        verification_token = str(uuid.uuid4())
        response = await self.generative_ai.generate(
            self.prompt_templates.verify_answer.format(
                challenge, answer, verification_token
            )
        )
        logger.debug(f"Challenge verification response: {response}")
        if response == verification_token:
            return True, ""
        return False, response
