#!/usr/bin/env python3
# -*- coding: utf-8 -*-

import sqlite3
from datetime import datetime, timezone


class DatabaseManager:
    def __init__(self, db_path: str = "bouncer.db") -> None:
        self.db_path = db_path
        self._create_tables()

    def _create_tables(self) -> None:
        with sqlite3.connect(self.db_path) as conn:
            cursor = conn.cursor()

            # Conversation history
            cursor.execute(
                """
                CREATE TABLE IF NOT EXISTS verification_history (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    timestamp INTEGER,
                    chat_id INTEGER,
                    chat_name TEXT,
                    user_id INTEGER,
                    full_name TEXT,
                    challenge TEXT,
                    answer TEXT,
                    result TEXT
                )
                """
            )

            # Groups allowed to use the bot
            cursor.execute(
                """
                CREATE TABLE IF NOT EXISTS allowed_groups (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    chat_id INTEGER UNIQUE,
                    chat_name TEXT
                )
                """
            )

            # Group-specific instructions
            cursor.execute(
                """
                CREATE TABLE IF NOT EXISTS group_topics (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    chat_id INTEGER UNIQUE,
                    topic TEXT
                )
                """
            )

            cursor.execute(
                """
                CREATE TABLE IF NOT EXISTS pending_users (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    timestamp INTEGER,
                    chat_id INTEGER,
                    user_id INTEGER,
                    challenge TEXT
                )
                """
            )

            cursor.execute(
                """
                CREATE TABLE IF NOT EXISTS join_attempts (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    timestamp INTEGER,
                    user_id INTEGER
                )
                """
            )

    def is_group_allowed(self, chat_id: int) -> bool:
        with sqlite3.connect(self.db_path) as conn:
            cursor = conn.cursor()
            cursor.execute(
                "SELECT chat_id FROM allowed_groups WHERE chat_id = ?",
                (chat_id,),
            )
            row = cursor.fetchone()
        return row is not None

    def store_verification_attempt(
        self,
        timestamp: int,
        chat_id: int,
        chat_name: str,
        user_id: int,
        full_name: str,
        challenge: str,
        response: str,
        verdict: str,
    ) -> None:
        with sqlite3.connect(self.db_path) as conn:
            cursor = conn.cursor()
            cursor.execute(
                """
                INSERT INTO conversation_history
                (timestamp, chat_id, chat_name, user_id,
                full_name, challenge, answer, result)
                VALUES (?, ?, ?, ?, ?, ?, ?, ?)
                """,
                (
                    timestamp,
                    chat_id,
                    chat_name,
                    user_id,
                    full_name,
                    challenge,
                    response,
                    verdict,
                ),
            )

    def get_group_topic(self, chat_id: int) -> str | None:
        with sqlite3.connect(self.db_path) as conn:
            cursor = conn.cursor()
            cursor.execute(
                "SELECT topic FROM group_topics WHERE chat_id = ?",
                (chat_id,),
            )
            row = cursor.fetchone()
        return row[0] if row else None

    def set_group_topic(self, chat_id: int, topic: str) -> None:
        with sqlite3.connect(self.db_path) as conn:
            cursor = conn.cursor()
            cursor.execute(
                """
                INSERT OR REPLACE INTO group_topics (chat_id, topic)
                VALUES (?, ?)
                """,
                (chat_id, topic),
            )

    def is_user_pending(self, user_id: int) -> bool:
        with sqlite3.connect(self.db_path) as conn:
            cursor = conn.cursor()
            cursor.execute(
                "SELECT user_id FROM pending_users WHERE user_id = ?",
                (user_id,),
            )
            row = cursor.fetchone()
        return row is not None

    def store_pending_user(
        self, timestamp: int, chat_id: int, user_id: int, challenge: str
    ) -> None:
        with sqlite3.connect(self.db_path) as conn:
            cursor = conn.cursor()
            cursor.execute(
                """
                INSERT INTO pending_users
                (timestamp, chat_id, user_id, challenge)
                VALUES (?, ?, ?, ?)
                """,
                (timestamp, chat_id, user_id, challenge),
            )

    def get_pending_user(self, user_id: int) -> tuple[datetime, int, int, str] | None:
        with sqlite3.connect(self.db_path) as conn:
            cursor = conn.cursor()
            cursor.execute(
                """
                SELECT timestamp, chat_id, user_id, challenge
                FROM pending_users WHERE user_id = ?
                """,
                (user_id,),
            )
            row = cursor.fetchone()

        if row is None:
            return None

        return (
            datetime.fromtimestamp(row[0]).replace(tzinfo=timezone.utc),
            row[1],
            row[2],
            row[3],
        )

    def remove_pending_user(self, chat_id: int, user_id: int) -> None:
        with sqlite3.connect(self.db_path) as conn:
            cursor = conn.cursor()
            cursor.execute(
                "DELETE FROM pending_users WHERE chat_id = ? AND user_id = ?",
                (chat_id, user_id),
            )

    def store_join_attempt(self, timestamp: int, user_id: int) -> None:
        with sqlite3.connect(self.db_path) as conn:
            cursor = conn.cursor()
            cursor.execute(
                """
                INSERT INTO join_attempts (timestamp, user_id)
                VALUES (?, ?)
                """,
                (timestamp, user_id),
            )

    def get_last_join_attempt(self, user_id: int) -> datetime | None:
        with sqlite3.connect(self.db_path) as conn:
            cursor = conn.cursor()
            cursor.execute(
                """
                SELECT timestamp FROM join_attempts WHERE user_id = ?
                ORDER BY timestamp DESC LIMIT 1
                """,
                (user_id,),
            )
            row = cursor.fetchone()

        return (
            datetime.fromtimestamp(row[0]).replace(tzinfo=timezone.utc) if row else None
        )
