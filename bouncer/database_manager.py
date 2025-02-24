#!/usr/bin/env python3
# -*- coding: utf-8 -*-

from datetime import datetime, timezone

from sqlalchemy import (
    Column,
    Integer,
    String,
    create_engine,
)
from sqlalchemy.orm import declarative_base, sessionmaker

Base = declarative_base()


class VerificationHistory(Base):
    __tablename__ = "verification_history"

    id = Column(Integer, primary_key=True, autoincrement=True)
    timestamp = Column(Integer)
    chat_id = Column(Integer)
    chat_title = Column(String)
    user_id = Column(Integer)
    full_name = Column(String)
    challenge = Column(String)
    answer = Column(String)
    verdict = Column(String)
    reason = Column(String)


class AllowedGroups(Base):
    __tablename__ = "allowed_groups"

    id = Column(Integer, primary_key=True, autoincrement=True)
    chat_id = Column(Integer, unique=True)
    chat_title = Column(String)


class GroupTopics(Base):
    __tablename__ = "group_topics"

    id = Column(Integer, primary_key=True, autoincrement=True)
    chat_id = Column(Integer, unique=True)
    topic = Column(String)


class PendingUsers(Base):
    __tablename__ = "pending_users"

    id = Column(Integer, primary_key=True, autoincrement=True)
    timestamp = Column(Integer)
    chat_id = Column(Integer)
    user_id = Column(Integer)
    challenge = Column(String)


class JoinAttempts(Base):
    __tablename__ = "join_attempts"

    id = Column(Integer, primary_key=True, autoincrement=True)
    timestamp = Column(Integer)
    user_id = Column(Integer)
    full_name = Column(String)


class DatabaseManager:
    def __init__(self, db_path: str = "bouncer.db") -> None:
        """Initialize the DB connection and create all tables."""
        self.db_path = db_path
        self.engine = create_engine(f"sqlite:///{self.db_path}", echo=False)
        Base.metadata.create_all(self.engine)
        self.session = sessionmaker(bind=self.engine, autocommit=False, autoflush=False)

    def is_group_allowed(self, chat_id: int) -> bool:
        """Check if a given chat_id exists in AllowedGroups."""
        with self.session() as session:
            group = session.query(AllowedGroups).filter_by(chat_id=chat_id).first()
            return group is not None

    def store_verification_attempt(
        self,
        timestamp: int,
        chat_id: int,
        chat_title: str,
        user_id: int,
        full_name: str,
        challenge: str,
        response: str,
        verdict: str,
        reason: str,
    ) -> None:
        """Add a new entry into VerificationHistory."""
        with self.session() as session:
            attempt = VerificationHistory(
                timestamp=timestamp,
                chat_id=chat_id,
                chat_title=chat_title,
                user_id=user_id,
                full_name=full_name,
                challenge=challenge,
                answer=response,
                verdict=verdict,
                reason=reason,
            )
            session.add(attempt)
            session.commit()

    def get_group_topic(self, chat_id: int) -> str | None:
        """Return the topic for a given group, if any."""
        with self.session() as session:
            group_topic = session.query(GroupTopics).filter_by(chat_id=chat_id).first()
            return group_topic.topic if group_topic else None

    def set_group_topic(self, chat_id: int, topic: str) -> None:
        """Create or replace a topic for a given group."""
        with self.session() as session:
            # Try to find existing record
            group_topic = session.query(GroupTopics).filter_by(chat_id=chat_id).first()
            if not group_topic:
                group_topic = GroupTopics(chat_id=chat_id)
                session.add(group_topic)
            group_topic.topic = topic
            session.commit()

    def is_user_pending(self, user_id: int) -> bool:
        """Check if a given user_id is present in PendingUsers."""
        with self.session() as session:
            pending_user = (
                session.query(PendingUsers).filter_by(user_id=user_id).first()
            )
            return pending_user is not None

    def store_pending_user(
        self, timestamp: int, chat_id: int, user_id: int, challenge: str
    ) -> None:
        """Store a new pending user record."""
        with self.session() as session:
            pending = PendingUsers(
                timestamp=timestamp,
                chat_id=chat_id,
                user_id=user_id,
                challenge=challenge,
            )
            session.add(pending)
            session.commit()

    def get_pending_user(self, user_id: int) -> tuple[datetime, int, int, str] | None:
        """
        Return the pending user record as (timestamp, chat_id, user_id, challenge).
        If no record, return None.
        """
        with self.session() as session:
            pending_user = (
                session.query(PendingUsers).filter_by(user_id=user_id).first()
            )

        if not pending_user:
            return None

        # Convert stored UNIX timestamp to datetime with UTC
        timestamp = datetime.fromtimestamp(pending_user.timestamp, tz=timezone.utc)
        return (
            timestamp,
            pending_user.chat_id,
            pending_user.user_id,
            pending_user.challenge,
        )

    def remove_pending_user(self, chat_id: int, user_id: int) -> None:
        """Remove a user from the pending list by chat_id and user_id."""
        with self.session() as session:
            pending_user = (
                session.query(PendingUsers)
                .filter_by(chat_id=chat_id, user_id=user_id)
                .first()
            )
            if pending_user:
                session.delete(pending_user)
                session.commit()

    def store_join_attempt(self, timestamp: int, user_id: int, full_name: str) -> None:
        """Store a join attempt in the JoinAttempts table."""
        with self.session() as session:
            join_attempt = JoinAttempts(
                timestamp=timestamp, user_id=user_id, full_name=full_name
            )
            session.add(join_attempt)
            session.commit()

    def get_last_join_attempt(self, user_id: int) -> datetime | None:
        """
        Return the most recent join attempt for the given user_id as a UTC datetime,
        or None if no attempts exist.
        """
        with self.session() as session:
            # Order by timestamp desc and grab first match
            attempt = (
                session.query(JoinAttempts)
                .filter_by(user_id=user_id)
                .order_by(JoinAttempts.timestamp.desc())
                .first()
            )

        if attempt is not None:
            return datetime.fromtimestamp(attempt.timestamp, tz=timezone.utc)
        return None
