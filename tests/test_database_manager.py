#!/usr/bin/env python3
# -*- coding: utf-8 -*-

import sqlite3
from datetime import datetime, timezone

import pytest
import sqlalchemy
from faker import Faker

from bouncer.database_manager import DatabaseManager


@pytest.fixture
def db_manager():
    """
    Pytest fixture that provides a fresh, in-memory DatabaseManager instance.
    """
    # Arrange
    manager = DatabaseManager(db_path=":memory:")
    yield manager
    # No teardown needed, the in-memory DB is dropped automatically


@pytest.fixture
def faker_instance():
    """
    Pytest fixture that returns a Faker instance for generating test data.
    """
    return Faker()


def test_is_group_allowed(db_manager, faker_instance):
    # Arrange
    chat_id = faker_instance.random_int(min=1, max=9999999)
    chat_title = faker_instance.company()

    # Act
    # Initially, the group is not in allowed_groups
    result_before = db_manager.is_group_allowed(chat_id)

    # Insert the group manually
    with db_manager.session() as session:
        session.execute(
            sqlalchemy.text(
                """
                INSERT INTO allowed_groups (chat_id, chat_title)
                VALUES (:chat_id, :chat_title)
                """
            ),
            {"chat_id": chat_id, "chat_title": chat_title},
        )
        session.commit()

    # Check again
    result_after = db_manager.is_group_allowed(chat_id)

    # Assert
    assert result_before is False
    assert result_after is True


def test_store_verification_attempt(db_manager, faker_instance):
    # Arrange
    timestamp = int(faker_instance.date_time_this_year().timestamp())
    chat_id = faker_instance.random_int(min=1, max=9999999)
    chat_title = faker_instance.company()
    user_id = faker_instance.random_int(min=1, max=9999999)
    full_name = faker_instance.name()
    challenge = faker_instance.sentence()
    response = faker_instance.word()
    verdict = faker_instance.random_element(elements=["accepted", "declined"])
    reason = faker_instance.sentence()

    # Act
    db_manager.store_verification_attempt(
        timestamp=timestamp,
        chat_id=chat_id,
        chat_title=chat_title,
        user_id=user_id,
        full_name=full_name,
        challenge=challenge,
        response=response,
        verdict=verdict,
        reason=reason,
    )

    # Assert
    with db_manager.session() as session:
        row = session.execute(
            sqlalchemy.text(
                """
                SELECT timestamp, chat_id, chat_title, user_id, 
                       full_name, challenge, answer, verdict, reason
                FROM verification_history
                WHERE user_id = :user_id
                """
            ),
            {"user_id": user_id},
        ).fetchone()

    assert row is not None, "Expected a row for the inserted verification attempt."
    assert row[0] == timestamp
    assert row[1] == chat_id
    assert row[2] == chat_title
    assert row[3] == user_id
    assert row[4] == full_name
    assert row[5] == challenge
    assert row[6] == response
    assert row[7] == verdict
    assert row[8] == reason


def test_get_group_topic_when_none(db_manager, faker_instance):
    # Arrange
    chat_id = faker_instance.random_int(min=1, max=9999999)

    # Act
    topic = db_manager.get_group_topic(chat_id)

    # Assert
    assert topic is None, "Expected None if no topic is set for the group."


def test_set_and_get_group_topic(db_manager, faker_instance):
    # Arrange
    chat_id = faker_instance.random_int(min=1, max=9999999)
    topic_text = faker_instance.sentence()

    # Act
    db_manager.set_group_topic(chat_id, topic_text)
    retrieved_topic = db_manager.get_group_topic(chat_id)

    # Assert
    assert retrieved_topic == topic_text


def test_is_user_pending(db_manager, faker_instance):
    # Arrange
    user_id = faker_instance.random_int(min=1, max=9999999)

    # Act
    # Initially, the user should not be pending
    result_before = db_manager.is_user_pending(user_id)

    # Insert user
    timestamp = int(faker_instance.date_time_this_year().timestamp())
    chat_id = faker_instance.random_int(min=1, max=9999999)
    challenge = faker_instance.sentence()

    db_manager.store_pending_user(timestamp, chat_id, user_id, challenge)
    result_after = db_manager.is_user_pending(user_id)

    # Assert
    assert result_before is False
    assert result_after is True


def test_store_pending_user_and_get(db_manager, faker_instance):
    # Arrange
    timestamp = int(faker_instance.date_time_this_year().timestamp())
    chat_id = faker_instance.random_int(min=1, max=9999999)
    user_id = faker_instance.random_int(min=1, max=9999999)
    challenge = faker_instance.sentence()

    # Act
    db_manager.store_pending_user(timestamp, chat_id, user_id, challenge)
    pending_data = db_manager.get_pending_user(user_id)

    # Assert
    assert pending_data is not None, "Expected pending user info to be returned."
    stored_dt, stored_chat_id, stored_user_id, stored_challenge = pending_data
    assert stored_chat_id == chat_id
    assert stored_user_id == user_id
    assert stored_challenge == challenge

    # The timestamp should match (converted to a UTC datetime object)
    expected_dt = datetime.fromtimestamp(timestamp).replace(tzinfo=timezone.utc)
    assert stored_dt == expected_dt


def test_remove_pending_user(db_manager, faker_instance):
    # Arrange
    timestamp = int(faker_instance.date_time_this_year().timestamp())
    chat_id = faker_instance.random_int(min=1, max=9999999)
    user_id = faker_instance.random_int(min=1, max=9999999)
    challenge = faker_instance.sentence()

    db_manager.store_pending_user(timestamp, chat_id, user_id, challenge)

    # Act
    db_manager.remove_pending_user(chat_id, user_id)

    # Assert
    assert not db_manager.is_user_pending(user_id), "User should no longer be pending."


def test_store_join_attempt_and_get_last(db_manager, faker_instance):
    # Arrange
    user_id = faker_instance.random_int(min=1, max=9999999)
    t1 = int(faker_instance.date_time_this_year().timestamp())
    t2 = int(faker_instance.date_time_this_year().timestamp())

    db_manager.store_join_attempt(t1, user_id, faker_instance.name())
    db_manager.store_join_attempt(t2, user_id, faker_instance.name())

    # Act
    last_attempt = db_manager.get_last_join_attempt(user_id)

    # Assert
    assert last_attempt is not None, "Expected a last join attempt timestamp."
    # We expect last_attempt to be the one associated with t2 if t2 > t1
    # However, since these are random, let's just ensure last_attempt is the max
    # of t1 and t2.
    assert last_attempt.timestamp() == max(t1, t2)
