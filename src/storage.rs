use std::path::Path;
use std::sync::{Arc, Mutex};

use rusqlite::{Connection, OptionalExtension, params};
use rusqlite_migration::{M, Migrations};

use crate::error::{Error, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    AwaitingButton,
    /// Transient stage during the question-generation LLM call. No deadline
    /// applies — the answer-side timeout only re-arms once the question is
    /// delivered.
    GeneratingQuestion,
    AwaitingAnswer,
    /// Transient stage during the answer-verification LLM call. No deadline
    /// applies — once the user has submitted, they've met the deadline; the
    /// LLM gets as long as the per-request timeout to decide.
    Verifying,
}

impl Stage {
    pub fn as_str(self) -> &'static str {
        match self {
            Stage::AwaitingButton => "awaiting_button",
            Stage::GeneratingQuestion => "generating_question",
            Stage::AwaitingAnswer => "awaiting_answer",
            Stage::Verifying => "verifying",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "awaiting_button" => Some(Stage::AwaitingButton),
            "generating_question" => Some(Stage::GeneratingQuestion),
            "awaiting_answer" => Some(Stage::AwaitingAnswer),
            "verifying" => Some(Stage::Verifying),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Approved,
    RejectedWrong,
    /// Join request was made but the user never pressed the start button —
    /// commonly seen with spam bots that can't interact with inline keyboards.
    RejectedNoButton,
    /// User pressed the start button and got the question, but never sent
    /// an answer in time.
    RejectedNoAnswer,
    RejectedLlmError,
    RejectedCooldown,
}

impl Outcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Outcome::Approved => "approved",
            Outcome::RejectedWrong => "rejected_wrong",
            Outcome::RejectedNoButton => "rejected_no_button",
            Outcome::RejectedNoAnswer => "rejected_no_answer",
            Outcome::RejectedLlmError => "rejected_llm_error",
            Outcome::RejectedCooldown => "rejected_cooldown",
        }
    }

    /// Whether this outcome should put the user on a retry cool-down. We
    /// only penalize rejections that are the user's fault (wrong answer or
    /// missed deadline). LLM/transport failures are bot-side problems, and
    /// `RejectedCooldown` is itself a no-op event recorded for audit while
    /// the cool-down is already active.
    pub fn imposes_cooldown(self) -> bool {
        matches!(
            self,
            Outcome::RejectedWrong | Outcome::RejectedNoButton | Outcome::RejectedNoAnswer
        )
    }
}

#[derive(Debug, Clone)]
pub struct PendingRow {
    pub chat_id: i64,
    pub user_id: i64,
    pub dm_chat_id: i64,
    pub stage: Stage,
    pub deadline: i64,
    pub question: Option<String>,
    pub question_msg_id: Option<i64>,
    pub started_at: i64,
    /// Human-readable name for the user (first + last, or `@username`, or
    /// `user <id>`). Captured at join-request time and persisted so later
    /// log lines (button press, answer, timeout, recovery) can identify the
    /// user without re-querying Telegram.
    pub display_name: Option<String>,
    /// Telegram `@username` if the user has one set.
    pub username: Option<String>,
}

#[derive(Debug, Clone)]
pub struct AuditRecord {
    pub chat_id: i64,
    pub user_id: i64,
    pub username: Option<String>,
    pub display_name: Option<String>,
    pub started_at: i64,
    pub completed_at: i64,
    pub question: Option<String>,
    pub answer: Option<String>,
    pub outcome: Outcome,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct GroupStats {
    pub attempts: u64,
    pub approved: u64,
    pub rejected_wrong: u64,
    pub rejected_no_button: u64,
    pub rejected_no_answer: u64,
    pub rejected_llm_error: u64,
    pub rejected_cooldown: u64,
    pub unique_users: u64,
}

impl GroupStats {
    pub fn rejected(&self) -> u64 {
        self.rejected_wrong
            + self.rejected_no_button
            + self.rejected_no_answer
            + self.rejected_llm_error
            + self.rejected_cooldown
    }
}

#[derive(Clone)]
pub struct Storage {
    conn: Arc<Mutex<Connection>>,
}

impl Storage {
    pub fn open(path: &Path) -> Result<Self> {
        let mut conn = Connection::open(path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        migrations().to_latest(&mut conn)?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    pub async fn run<F, T>(&self, f: F) -> Result<T>
    where
        F: FnOnce(&mut Connection) -> Result<T> + Send + 'static,
        T: Send + 'static,
    {
        let conn = self.conn.clone();
        tokio::task::spawn_blocking(move || {
            let mut guard = conn.lock().expect("storage mutex poisoned");
            f(&mut guard)
        })
        .await?
    }

    pub async fn active_cooldown(
        &self,
        chat_id: i64,
        user_id: i64,
        now: i64,
    ) -> Result<Option<i64>> {
        self.run(move |c| {
            let expires: Option<i64> = c
                .query_row(
                    "SELECT expires_at FROM cooldowns WHERE chat_id = ?1 AND user_id = ?2",
                    params![chat_id, user_id],
                    |row| row.get(0),
                )
                .optional()?;
            Ok(expires.filter(|exp| *exp > now))
        })
        .await
    }

    pub async fn upsert_pending(&self, row: PendingRow) -> Result<()> {
        self.run(move |c| {
            c.execute(
                "INSERT INTO pending_verifications (\
                    chat_id, user_id, dm_chat_id, stage, deadline, question, question_msg_id, \
                    started_at, display_name, username\
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)\
                 ON CONFLICT(chat_id, user_id) DO UPDATE SET \
                    dm_chat_id = excluded.dm_chat_id, \
                    stage = excluded.stage, \
                    deadline = excluded.deadline, \
                    question = excluded.question, \
                    question_msg_id = excluded.question_msg_id, \
                    started_at = excluded.started_at, \
                    display_name = excluded.display_name, \
                    username = excluded.username",
                params![
                    row.chat_id,
                    row.user_id,
                    row.dm_chat_id,
                    row.stage.as_str(),
                    row.deadline,
                    row.question,
                    row.question_msg_id,
                    row.started_at,
                    row.display_name,
                    row.username,
                ],
            )?;
            Ok(())
        })
        .await
    }

    /// Atomically transition `awaiting_button` → `generating_question`.
    /// Sets `deadline = 0` so any in-flight button-press timeout task that
    /// already passed its sleep checkpoint will fail its `deadline == captured`
    /// check and bail. Returns true if exactly one row was claimed.
    pub async fn try_begin_generating(&self, chat_id: i64, user_id: i64) -> Result<bool> {
        self.run(move |c| {
            let n = c.execute(
                "UPDATE pending_verifications SET stage = 'generating_question', deadline = 0 \
                 WHERE chat_id = ?1 AND user_id = ?2 AND stage = 'awaiting_button'",
                params![chat_id, user_id],
            )?;
            Ok(n == 1)
        })
        .await
    }

    /// Atomically transition `awaiting_answer` → `verifying`. Same race-
    /// safety guarantees as `try_begin_generating`.
    pub async fn try_begin_verifying(&self, chat_id: i64, user_id: i64) -> Result<bool> {
        self.run(move |c| {
            let n = c.execute(
                "UPDATE pending_verifications SET stage = 'verifying', deadline = 0 \
                 WHERE chat_id = ?1 AND user_id = ?2 AND stage = 'awaiting_answer'",
                params![chat_id, user_id],
            )?;
            Ok(n == 1)
        })
        .await
    }

    pub async fn advance_to_answer(
        &self,
        chat_id: i64,
        user_id: i64,
        question: String,
        question_msg_id: Option<i64>,
        deadline: i64,
    ) -> Result<()> {
        self.run(move |c| {
            let affected = c.execute(
                "UPDATE pending_verifications SET \
                    stage = 'awaiting_answer', \
                    question = ?3, \
                    question_msg_id = ?4, \
                    deadline = ?5 \
                 WHERE chat_id = ?1 AND user_id = ?2",
                params![chat_id, user_id, question, question_msg_id, deadline],
            )?;
            if affected == 0 {
                return Err(Error::ConfigInvalid(format!(
                    "no pending verification for chat {chat_id} user {user_id}"
                )));
            }
            Ok(())
        })
        .await
    }

    pub async fn get_pending(&self, chat_id: i64, user_id: i64) -> Result<Option<PendingRow>> {
        self.run(move |c| {
            let row = c
                .query_row(
                    "SELECT chat_id, user_id, dm_chat_id, stage, deadline, question, question_msg_id, \
                            started_at, display_name, username \
                     FROM pending_verifications WHERE chat_id = ?1 AND user_id = ?2",
                    params![chat_id, user_id],
                    map_pending_row,
                )
                .optional()?;
            Ok(row)
        })
        .await
    }

    /// Look up the single `AwaitingAnswer` row (if any) for a user's DM chat.
    /// Users DM the bot directly, so there's no `chat_id` on the incoming
    /// message — we route by `dm_chat_id` (== user's private chat id).
    pub async fn find_awaiting_answer_by_dm(&self, dm_chat_id: i64) -> Result<Option<PendingRow>> {
        self.find_pending_by_dm(dm_chat_id, Stage::AwaitingAnswer)
            .await
    }

    /// Look up the most recent `AwaitingButton` row for a user's DM chat.
    pub async fn find_awaiting_button_by_dm(&self, dm_chat_id: i64) -> Result<Option<PendingRow>> {
        self.find_pending_by_dm(dm_chat_id, Stage::AwaitingButton)
            .await
    }

    async fn find_pending_by_dm(
        &self,
        dm_chat_id: i64,
        stage: Stage,
    ) -> Result<Option<PendingRow>> {
        let stage_str = stage.as_str();
        self.run(move |c| {
            let row = c
                .query_row(
                    "SELECT chat_id, user_id, dm_chat_id, stage, deadline, question, question_msg_id, \
                            started_at, display_name, username \
                     FROM pending_verifications \
                     WHERE dm_chat_id = ?1 AND stage = ?2 \
                     ORDER BY started_at DESC LIMIT 1",
                    params![dm_chat_id, stage_str],
                    map_pending_row,
                )
                .optional()?;
            Ok(row)
        })
        .await
    }

    /// Return up to `limit` of the most recently seen questions for a chat,
    /// drawn from both the audit log and any in-flight pending row. Used to
    /// show the LLM a "do not repeat" list when generating the next question.
    pub async fn recent_questions(&self, chat_id: i64, limit: u32) -> Result<Vec<String>> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        self.run(move |c| {
            let mut stmt = c.prepare(
                "SELECT question FROM (\
                    SELECT question, completed_at AS ts FROM verifications \
                        WHERE chat_id = ?1 AND question IS NOT NULL \
                    UNION ALL \
                    SELECT question, started_at AS ts FROM pending_verifications \
                        WHERE chat_id = ?1 AND question IS NOT NULL\
                 ) ORDER BY ts DESC LIMIT ?2",
            )?;
            let rows: Vec<String> = stmt
                .query_map(params![chat_id, limit], |row| row.get::<_, String>(0))?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            Ok(rows)
        })
        .await
    }

    pub async fn list_pending(&self) -> Result<Vec<PendingRow>> {
        self.run(|c| {
            let mut stmt = c.prepare(
                "SELECT chat_id, user_id, dm_chat_id, stage, deadline, question, question_msg_id, \
                        started_at, display_name, username \
                 FROM pending_verifications",
            )?;
            let rows = stmt
                .query_map([], map_pending_row)?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            Ok(rows)
        })
        .await
    }

    /// Finalize a verification atomically: remove the pending row, append an
    /// audit record, and (on rejection) upsert the cool-down entry.
    pub async fn finalize(
        &self,
        record: AuditRecord,
        cooldown_expires_at: Option<i64>,
    ) -> Result<()> {
        self.run(move |c| {
            let tx = c.transaction()?;
            tx.execute(
                "DELETE FROM pending_verifications WHERE chat_id = ?1 AND user_id = ?2",
                params![record.chat_id, record.user_id],
            )?;
            tx.execute(
                "INSERT INTO verifications (\
                    chat_id, user_id, username, display_name, started_at, completed_at, \
                    question, answer, outcome, reason\
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![
                    record.chat_id,
                    record.user_id,
                    record.username,
                    record.display_name,
                    record.started_at,
                    record.completed_at,
                    record.question,
                    record.answer,
                    record.outcome.as_str(),
                    record.reason,
                ],
            )?;
            if let Some(expires) = cooldown_expires_at {
                tx.execute(
                    "INSERT INTO cooldowns (chat_id, user_id, expires_at) VALUES (?1, ?2, ?3)\
                     ON CONFLICT(chat_id, user_id) DO UPDATE SET expires_at = excluded.expires_at",
                    params![record.chat_id, record.user_id, expires],
                )?;
            }
            tx.commit()?;
            Ok(())
        })
        .await
    }

    /// Stats grouped by chat_id. Returns `(chat_id, stats)` pairs.
    pub async fn stats_by_group(&self, since: Option<i64>) -> Result<Vec<(i64, GroupStats)>> {
        self.run(move |c| {
            let mut stmt = c.prepare(
                "SELECT chat_id, outcome, COUNT(*) FROM verifications \
                 WHERE (?1 IS NULL OR completed_at >= ?1) \
                 GROUP BY chat_id, outcome",
            )?;
            let mut result: std::collections::BTreeMap<i64, GroupStats> =
                std::collections::BTreeMap::new();
            let rows = stmt.query_map(params![since], |row| {
                let chat_id: i64 = row.get(0)?;
                let outcome: String = row.get(1)?;
                let count: i64 = row.get(2)?;
                Ok((chat_id, outcome, count))
            })?;
            for row in rows {
                let (chat_id, outcome, count) = row?;
                let stats = result.entry(chat_id).or_default();
                apply_outcome_count(stats, &outcome, count as u64);
            }

            let mut unique_stmt = c.prepare(
                "SELECT chat_id, COUNT(DISTINCT user_id) FROM verifications \
                 WHERE (?1 IS NULL OR completed_at >= ?1) GROUP BY chat_id",
            )?;
            let unique_rows = unique_stmt.query_map(params![since], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?))
            })?;
            for row in unique_rows {
                let (chat_id, unique) = row?;
                result.entry(chat_id).or_default().unique_users = unique as u64;
            }

            Ok(result.into_iter().collect())
        })
        .await
    }

    pub async fn stats_global(&self, since: Option<i64>) -> Result<GroupStats> {
        self.run(move |c| {
            let mut stats = GroupStats::default();
            let mut stmt = c.prepare(
                "SELECT outcome, COUNT(*) FROM verifications \
                 WHERE (?1 IS NULL OR completed_at >= ?1) GROUP BY outcome",
            )?;
            let rows = stmt.query_map(params![since], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
            })?;
            for row in rows {
                let (outcome, count) = row?;
                apply_outcome_count(&mut stats, &outcome, count as u64);
            }

            let unique: i64 = c.query_row(
                "SELECT COUNT(DISTINCT user_id || ':' || chat_id) FROM verifications \
                 WHERE (?1 IS NULL OR completed_at >= ?1)",
                params![since],
                |row| row.get(0),
            )?;
            stats.unique_users = unique as u64;
            Ok(stats)
        })
        .await
    }
}

fn map_pending_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<PendingRow> {
    let stage_raw: String = row.get(3)?;
    let stage = Stage::parse(&stage_raw).ok_or_else(|| {
        rusqlite::Error::FromSqlConversionFailure(
            3,
            rusqlite::types::Type::Text,
            format!("unknown stage `{stage_raw}`").into(),
        )
    })?;
    Ok(PendingRow {
        chat_id: row.get(0)?,
        user_id: row.get(1)?,
        dm_chat_id: row.get(2)?,
        stage,
        deadline: row.get(4)?,
        question: row.get(5)?,
        question_msg_id: row.get(6)?,
        started_at: row.get(7)?,
        display_name: row.get(8)?,
        username: row.get(9)?,
    })
}

fn apply_outcome_count(stats: &mut GroupStats, outcome: &str, count: u64) {
    stats.attempts += count;
    match outcome {
        "approved" => stats.approved += count,
        "rejected_wrong" => stats.rejected_wrong += count,
        "rejected_no_button" => stats.rejected_no_button += count,
        "rejected_no_answer" => stats.rejected_no_answer += count,
        // Pre-split DB rows recorded only `rejected_timeout`. Bucket them
        // into `no_answer` since that's the more common timeout cause and
        // we can no longer recover the original stage.
        "rejected_timeout" => stats.rejected_no_answer += count,
        "rejected_llm_error" => stats.rejected_llm_error += count,
        "rejected_cooldown" => stats.rejected_cooldown += count,
        _ => {}
    }
}

fn migrations() -> Migrations<'static> {
    Migrations::new(vec![
        M::up(
            "CREATE TABLE pending_verifications (\
                chat_id         INTEGER NOT NULL,\
                user_id         INTEGER NOT NULL,\
                dm_chat_id      INTEGER NOT NULL,\
                stage           TEXT    NOT NULL,\
                deadline        INTEGER NOT NULL,\
                question        TEXT,\
                question_msg_id INTEGER,\
                started_at      INTEGER NOT NULL,\
                PRIMARY KEY (chat_id, user_id)\
            );\
            CREATE TABLE cooldowns (\
                chat_id    INTEGER NOT NULL,\
                user_id    INTEGER NOT NULL,\
                expires_at INTEGER NOT NULL,\
                PRIMARY KEY (chat_id, user_id)\
            );\
            CREATE TABLE verifications (\
                id           INTEGER PRIMARY KEY AUTOINCREMENT,\
                chat_id      INTEGER NOT NULL,\
                user_id      INTEGER NOT NULL,\
                username     TEXT,\
                started_at   INTEGER NOT NULL,\
                completed_at INTEGER NOT NULL,\
                question     TEXT,\
                answer       TEXT,\
                outcome      TEXT    NOT NULL,\
                reason       TEXT\
            );\
            CREATE INDEX verifications_chat_idx ON verifications(chat_id);\
            CREATE INDEX verifications_outcome_idx ON verifications(outcome);\
            CREATE INDEX verifications_completed_at_idx ON verifications(completed_at);",
        ),
        M::up(
            "ALTER TABLE pending_verifications ADD COLUMN display_name TEXT;\
             ALTER TABLE pending_verifications ADD COLUMN username     TEXT;\
             ALTER TABLE verifications         ADD COLUMN display_name TEXT;",
        ),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn migrations_apply_to_memory_db() {
        let dir = tempdir();
        let db = dir.join("bouncer.db");
        let storage = Storage::open(&db).expect("open");
        let stats = storage.stats_global(None).await.expect("stats");
        assert_eq!(stats.attempts, 0);
    }

    #[tokio::test]
    async fn pending_upsert_and_recover() {
        let dir = tempdir();
        let db = dir.join("bouncer.db");
        let storage = Storage::open(&db).unwrap();
        let row = PendingRow {
            chat_id: -100,
            user_id: 42,
            dm_chat_id: 42,
            stage: Stage::AwaitingButton,
            deadline: 1_000,
            question: None,
            question_msg_id: None,
            started_at: 900,
            display_name: Some("Alice".into()),
            username: Some("alice".into()),
        };
        storage.upsert_pending(row.clone()).await.unwrap();
        let rows = storage.list_pending().await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].user_id, 42);
        assert_eq!(rows[0].stage, Stage::AwaitingButton);
    }

    #[tokio::test]
    async fn finalize_writes_audit_and_cooldown() {
        let dir = tempdir();
        let db = dir.join("bouncer.db");
        let storage = Storage::open(&db).unwrap();
        storage
            .upsert_pending(PendingRow {
                chat_id: -100,
                user_id: 7,
                dm_chat_id: 7,
                stage: Stage::AwaitingAnswer,
                deadline: 2_000,
                question: Some("q".into()),
                question_msg_id: Some(10),
                started_at: 100,
                display_name: Some("Alice".into()),
                username: Some("alice".into()),
            })
            .await
            .unwrap();
        let audit = AuditRecord {
            chat_id: -100,
            user_id: 7,
            username: Some("alice".into()),
            display_name: Some("Alice".into()),
            started_at: 100,
            completed_at: 200,
            question: Some("q".into()),
            answer: Some("a".into()),
            outcome: Outcome::RejectedWrong,
            reason: Some("bad".into()),
        };
        storage.finalize(audit, Some(9_999)).await.unwrap();
        assert!(storage.get_pending(-100, 7).await.unwrap().is_none());
        assert_eq!(
            storage.active_cooldown(-100, 7, 0).await.unwrap(),
            Some(9_999)
        );
        let stats = storage.stats_global(None).await.unwrap();
        assert_eq!(stats.attempts, 1);
        assert_eq!(stats.rejected_wrong, 1);
    }

    #[tokio::test]
    async fn recent_questions_returns_newest_first_capped_to_limit() {
        let dir = tempdir();
        let db = dir.join("bouncer.db");
        let storage = Storage::open(&db).unwrap();
        // Insert three completed verifications with increasing timestamps.
        for (i, q) in [(100, "Q1"), (200, "Q2"), (300, "Q3")].iter() {
            storage
                .upsert_pending(PendingRow {
                    chat_id: -7,
                    user_id: *i,
                    dm_chat_id: *i,
                    stage: Stage::AwaitingAnswer,
                    deadline: 0,
                    question: Some((*q).into()),
                    question_msg_id: None,
                    started_at: *i,
                    display_name: None,
                    username: None,
                })
                .await
                .unwrap();
            storage
                .finalize(
                    AuditRecord {
                        chat_id: -7,
                        user_id: *i,
                        username: None,
                        display_name: None,
                        started_at: *i,
                        completed_at: *i,
                        question: Some((*q).into()),
                        answer: Some("a".into()),
                        outcome: Outcome::Approved,
                        reason: None,
                    },
                    None,
                )
                .await
                .unwrap();
        }
        let recent = storage.recent_questions(-7, 2).await.unwrap();
        assert_eq!(recent, vec!["Q3".to_string(), "Q2".to_string()]);

        let none_for_other_chat = storage.recent_questions(-99, 5).await.unwrap();
        assert!(none_for_other_chat.is_empty());

        let empty_when_limit_zero = storage.recent_questions(-7, 0).await.unwrap();
        assert!(empty_when_limit_zero.is_empty());
    }

    #[tokio::test]
    async fn try_begin_verifying_claims_only_once() {
        let dir = tempdir();
        let db = dir.join("bouncer.db");
        let storage = Storage::open(&db).unwrap();
        storage
            .upsert_pending(PendingRow {
                chat_id: -1,
                user_id: 5,
                dm_chat_id: 5,
                stage: Stage::AwaitingAnswer,
                deadline: 1_000,
                question: Some("q".into()),
                question_msg_id: Some(11),
                started_at: 100,
                display_name: None,
                username: None,
            })
            .await
            .unwrap();
        assert!(storage.try_begin_verifying(-1, 5).await.unwrap());
        // Second call must fail — stage is now `verifying`, not `awaiting_answer`.
        assert!(!storage.try_begin_verifying(-1, 5).await.unwrap());
        let row = storage.get_pending(-1, 5).await.unwrap().unwrap();
        assert_eq!(row.stage, Stage::Verifying);
        assert_eq!(row.deadline, 0);
    }

    #[tokio::test]
    async fn try_begin_generating_claims_only_once() {
        let dir = tempdir();
        let db = dir.join("bouncer.db");
        let storage = Storage::open(&db).unwrap();
        storage
            .upsert_pending(PendingRow {
                chat_id: -1,
                user_id: 6,
                dm_chat_id: 6,
                stage: Stage::AwaitingButton,
                deadline: 2_000,
                question: None,
                question_msg_id: None,
                started_at: 100,
                display_name: None,
                username: None,
            })
            .await
            .unwrap();
        assert!(storage.try_begin_generating(-1, 6).await.unwrap());
        assert!(!storage.try_begin_generating(-1, 6).await.unwrap());
        let row = storage.get_pending(-1, 6).await.unwrap().unwrap();
        assert_eq!(row.stage, Stage::GeneratingQuestion);
        assert_eq!(row.deadline, 0);
    }

    #[test]
    fn imposes_cooldown_only_for_user_caused_rejections() {
        assert!(Outcome::RejectedWrong.imposes_cooldown());
        assert!(Outcome::RejectedNoButton.imposes_cooldown());
        assert!(Outcome::RejectedNoAnswer.imposes_cooldown());
        assert!(!Outcome::RejectedLlmError.imposes_cooldown());
        assert!(!Outcome::RejectedCooldown.imposes_cooldown());
        assert!(!Outcome::Approved.imposes_cooldown());
    }

    fn tempdir() -> std::path::PathBuf {
        // Atomic counter avoids same-nanosecond collisions when tests run in
        // parallel and end up generating identical timestamps.
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "bouncer-test-{}-{}-{n}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }
}
