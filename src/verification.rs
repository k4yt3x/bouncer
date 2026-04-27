use std::collections::HashMap;
use std::sync::Arc;

use teloxide::Bot;
use teloxide::payloads::{
    EditMessageReplyMarkupSetters, SendMessageSetters, SetMessageReactionSetters,
};
use teloxide::prelude::Requester;
use teloxide::types::{
    ChatAction, ChatId, InlineKeyboardButton, InlineKeyboardMarkup, MessageId, ReactionType,
    Recipient, User, UserId,
};
use tokio::sync::Mutex;
use tracing::{debug, error, info, warn};

use crate::config::Config;
use crate::error::Result;
use crate::i18n::{Locale, LocaleRegistry, render};
use crate::llm::LlmClient;
use crate::storage::{AuditRecord, Outcome, PendingRow, Stage, Storage};

pub const START_CALLBACK: &str = "bouncer:start";
/// Callback data for the "Generating..." button shown while the LLM is busy.
/// Pressing it does nothing — the handler simply acks the callback.
pub const NOOP_CALLBACK: &str = "bouncer:noop";

type TimeoutMap = HashMap<(i64, i64), tokio::task::JoinHandle<()>>;

#[derive(Clone)]
pub struct Engine {
    storage: Storage,
    llm: Arc<LlmClient>,
    bot: Bot,
    config: Arc<Config>,
    locales: Arc<LocaleRegistry>,
    timeouts: Arc<Mutex<TimeoutMap>>,
}

impl Engine {
    pub fn new(
        storage: Storage,
        llm: Arc<LlmClient>,
        bot: Bot,
        config: Arc<Config>,
        locales: Arc<LocaleRegistry>,
    ) -> Self {
        Self {
            storage,
            llm,
            bot,
            config,
            locales,
            timeouts: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Pick up any `pending_verifications` left behind by a previous run.
    /// Expired ones are finalized as timeouts; unexpired ones get their
    /// timeout tasks re-armed. Rows stuck in a transient LLM-call stage
    /// (`generating_question` / `verifying`) mean the previous process died
    /// mid-LLM-call — they are declined as `RejectedLlmError` (no cool-down,
    /// since this was the bot's fault).
    pub async fn recover(self: &Arc<Self>) -> Result<()> {
        let rows = self.storage.list_pending().await?;
        info!(count = rows.len(), "recovering pending verifications");
        let now = unix_now();
        for row in rows {
            match row.stage {
                Stage::GeneratingQuestion | Stage::Verifying => {
                    warn!(
                        chat_id = row.chat_id,
                        user_id = row.user_id,
                        display_name = row.display_name.as_deref().unwrap_or(""),
                        username = row.username.as_deref().unwrap_or(""),
                        stage = ?row.stage,
                        "crashed mid-LLM-call; declining as LLM error"
                    );
                    self.clone().fire_llm_error(row).await;
                }
                Stage::AwaitingButton | Stage::AwaitingAnswer => {
                    if row.deadline <= now {
                        self.clone().fire_timeout(row).await;
                    } else {
                        self.clone()
                            .arm_timeout(row.chat_id, row.user_id, row.deadline)
                            .await;
                    }
                }
            }
        }
        Ok(())
    }

    /// Entry point called from the Telegram handler when a join request arrives.
    pub async fn on_join_request(
        self: &Arc<Self>,
        chat_id: i64,
        chat_title: Option<String>,
        user: User,
        dm_chat_id: i64,
    ) -> Result<()> {
        let user_id = user.id.0 as i64;
        let display_name_str = display_name(&user);
        let username_str = user.username.clone().unwrap_or_default();
        debug!(
            chat_id,
            chat_title = chat_title.as_deref().unwrap_or(""),
            user_id,
            display_name = display_name_str.as_str(),
            username = username_str.as_str(),
            "received join request"
        );

        let Some(group) = self.config.group(chat_id) else {
            warn!(
                chat_id,
                user_id,
                display_name = display_name_str.as_str(),
                username = username_str.as_str(),
                "join request for unenrolled group — ignoring"
            );
            return Ok(());
        };
        if !group.enabled {
            warn!(
                chat_id,
                user_id,
                display_name = display_name_str.as_str(),
                username = username_str.as_str(),
                "join request for disabled group — ignoring"
            );
            return Ok(());
        }

        let now = unix_now();

        if let Some(expires) = self.storage.active_cooldown(chat_id, user_id, now).await? {
            self.handle_cooldown_hit(chat_id, &user, dm_chat_id, expires, now)
                .await;
            return Ok(());
        }

        let locale = self.locale_for_group(group.locale.as_deref());
        let deadline = now + self.config.timeouts.button_press_secs as i64;
        let row = PendingRow {
            chat_id,
            user_id,
            dm_chat_id,
            stage: Stage::AwaitingButton,
            deadline,
            question: None,
            question_msg_id: None,
            started_at: now,
            display_name: Some(display_name_str.clone()),
            username: user.username.clone(),
        };
        self.storage.upsert_pending(row.clone()).await?;

        let welcome = render_welcome(
            locale,
            chat_title.as_deref(),
            &display_name(&user),
            self.config.timeouts.button_press_secs,
        );
        let keyboard = InlineKeyboardMarkup::new(vec![vec![InlineKeyboardButton::callback(
            locale.start_button.clone(),
            START_CALLBACK,
        )]]);
        if let Err(e) = self
            .bot
            .send_message(ChatId(dm_chat_id), welcome)
            .reply_markup(keyboard)
            .await
        {
            warn!(
                chat_id,
                user_id,
                display_name = display_name_str.as_str(),
                username = username_str.as_str(),
                error = %e,
                "failed to DM welcome"
            );
        }

        debug!(
            chat_id,
            user_id,
            display_name = display_name_str.as_str(),
            username = username_str.as_str(),
            deadline,
            "verification started; awaiting button press"
        );
        self.clone().arm_timeout(chat_id, user_id, deadline).await;
        Ok(())
    }

    /// Entry point called when the user presses the "Start Verification" button.
    pub async fn on_button_press(
        self: &Arc<Self>,
        chat_id: i64,
        user_id: i64,
        welcome_msg_id: i32,
    ) -> Result<()> {
        let Some(row) = self.storage.get_pending(chat_id, user_id).await? else {
            warn!(
                chat_id,
                user_id, "button press with no pending verification — ignoring"
            );
            return Ok(());
        };
        let dn = row.display_name.as_deref().unwrap_or("").to_string();
        let un = row.username.as_deref().unwrap_or("").to_string();
        if row.stage != Stage::AwaitingButton {
            warn!(
                chat_id,
                user_id,
                display_name = dn.as_str(),
                username = un.as_str(),
                stage = ?row.stage,
                "button press in wrong stage — ignoring"
            );
            return Ok(());
        }

        let Some(group) = self.config.group(chat_id) else {
            warn!(chat_id, "button press for unenrolled group — ignoring");
            return Ok(());
        };
        if !group.enabled {
            warn!(chat_id, "button press for disabled group — ignoring");
            return Ok(());
        }
        let locale = self.locale_for_group(group.locale.as_deref()).clone();
        let prompt = group.question_prompt.clone();

        // Atomically claim the row before doing anything user-visible. If the
        // button-press timeout already fired (or another concurrent press
        // claimed first), bail — the previous handler already finalized.
        let claimed = self.storage.try_begin_generating(chat_id, user_id).await?;
        if !claimed {
            warn!(
                chat_id,
                user_id,
                display_name = dn.as_str(),
                username = un.as_str(),
                "button press lost the race against the timeout — ignoring"
            );
            return Ok(());
        }
        info!(
            chat_id,
            user_id,
            display_name = dn.as_str(),
            username = un.as_str(),
            "button pressed; generating question"
        );
        self.clear_timeout(chat_id, user_id).await;

        // Swap the inline keyboard's button text to a "Generating..." label
        // (with a no-op callback) so the user sees progress on the original
        // welcome message. The message text itself is left untouched.
        let generating_keyboard =
            InlineKeyboardMarkup::new(vec![vec![InlineKeyboardButton::callback(
                locale.generating_button.clone(),
                NOOP_CALLBACK,
            )]]);
        if let Err(e) = self
            .bot
            .edit_message_reply_markup(ChatId(row.dm_chat_id), MessageId(welcome_msg_id))
            .reply_markup(generating_keyboard)
            .await
        {
            warn!(
                chat_id,
                user_id,
                display_name = dn.as_str(),
                username = un.as_str(),
                error = %e,
                "failed to swap keyboard to generating"
            );
        }

        // Feed the most recent questions for this group back to the LLM so
        // it doesn't keep regenerating slight variations of the operator's
        // examples.
        let recent = self
            .storage
            .recent_questions(chat_id, self.config.llm.recent_question_window)
            .await
            .unwrap_or_else(|e| {
                warn!(
                    chat_id,
                    user_id,
                    display_name = dn.as_str(),
                    username = un.as_str(),
                    error = %e,
                    "failed to fetch recent questions; proceeding with empty list"
                );
                Vec::new()
            });
        let typing = spawn_typing_indicator(self.bot.clone(), row.dm_chat_id);
        let llm_result = self.llm.generate_question(&prompt, &recent).await;
        typing.abort();

        let question = match llm_result {
            Ok(q) => q,
            Err(e) => {
                error!(
                    chat_id,
                    user_id,
                    display_name = dn.as_str(),
                    username = un.as_str(),
                    error = %e,
                    "question generation failed"
                );
                // Strip the keyboard so the welcome no longer offers a button,
                // then DM the user-facing error. Empty markup clears the
                // existing keyboard.
                if let Err(edit_err) = self
                    .bot
                    .edit_message_reply_markup(ChatId(row.dm_chat_id), MessageId(welcome_msg_id))
                    .reply_markup(InlineKeyboardMarkup::new(
                        Vec::<Vec<InlineKeyboardButton>>::new(),
                    ))
                    .await
                {
                    warn!(
                        chat_id,
                        user_id,
                        display_name = dn.as_str(),
                        username = un.as_str(),
                        error = %edit_err,
                        "failed to clear keyboard on error"
                    );
                }
                self.clone()
                    .finalize_rejection(
                        row,
                        Outcome::RejectedLlmError,
                        None,
                        Some(format!("question generation failed: {e}")),
                        Some(locale.rejected_llm_error.clone()),
                    )
                    .await;
                return Ok(());
            }
        };

        // Question is ready — strip the keyboard from the welcome so it no
        // longer shows the "Generating..." label after the question DM lands.
        if let Err(e) = self
            .bot
            .edit_message_reply_markup(ChatId(row.dm_chat_id), MessageId(welcome_msg_id))
            .reply_markup(InlineKeyboardMarkup::new(
                Vec::<Vec<InlineKeyboardButton>>::new(),
            ))
            .await
        {
            warn!(
                chat_id,
                user_id,
                display_name = dn.as_str(),
                username = un.as_str(),
                error = %e,
                "failed to clear keyboard after question ready"
            );
        }

        let now = unix_now();
        let deadline = now + self.config.timeouts.answer_submission_secs as i64;
        let intro = render(
            &locale.question_intro,
            &[(
                "minutes",
                &format_minutes(self.config.timeouts.answer_submission_secs),
            )],
        )
        .into_owned();
        let text = format!("{intro}\n\n{question}");
        let sent = match self.bot.send_message(ChatId(row.dm_chat_id), text).await {
            Ok(m) => m,
            Err(e) => {
                error!(
                    chat_id,
                    user_id,
                    display_name = dn.as_str(),
                    username = un.as_str(),
                    error = %e,
                    "failed to send question DM"
                );
                self.clone()
                    .finalize_rejection(
                        row,
                        Outcome::RejectedLlmError,
                        None,
                        Some(format!("failed to send question: {e}")),
                        None,
                    )
                    .await;
                return Ok(());
            }
        };

        info!(
            chat_id,
            user_id,
            display_name = dn.as_str(),
            username = un.as_str(),
            deadline,
            question = question.as_str(),
            "question delivered; awaiting answer"
        );
        self.storage
            .advance_to_answer(chat_id, user_id, question, Some(sent.id.0 as i64), deadline)
            .await?;
        self.clone().arm_timeout(chat_id, user_id, deadline).await;
        Ok(())
    }

    /// Entry point called when the user sends a DM that may be an answer.
    pub async fn on_user_answer(
        self: &Arc<Self>,
        _user_id: i64,
        dm_chat_id: i64,
        message_id: i32,
        answer: &str,
    ) -> Result<()> {
        let Some(row) = self.storage.find_awaiting_answer_by_dm(dm_chat_id).await? else {
            return Ok(());
        };
        let chat_id = row.chat_id;
        let user_id = row.user_id;
        let dn = row.display_name.as_deref().unwrap_or("").to_string();
        let un = row.username.as_deref().unwrap_or("").to_string();
        let Some(question) = row.question.clone() else {
            warn!(
                chat_id,
                user_id,
                display_name = dn.as_str(),
                username = un.as_str(),
                "awaiting_answer row missing question"
            );
            return Ok(());
        };
        let Some(group) = self.config.group(chat_id) else {
            warn!(chat_id, "answer for unenrolled group — ignoring");
            return Ok(());
        };
        if !group.enabled {
            warn!(chat_id, "answer for disabled group — ignoring");
            return Ok(());
        }
        let locale = self.locale_for_group(group.locale.as_deref()).clone();
        let prompt = group.question_prompt.clone();

        // Atomically claim the row before any user-visible work. If the
        // answer-deadline timeout fired between the user's send and our
        // handler running, the row is gone (or no longer awaiting_answer)
        // and we just silently drop the late answer — the timeout flow
        // already declined and notified the user.
        let claimed = self.storage.try_begin_verifying(chat_id, user_id).await?;
        if !claimed {
            warn!(
                chat_id,
                user_id,
                display_name = dn.as_str(),
                username = un.as_str(),
                "answer arrived after the timeout fired — ignoring"
            );
            return Ok(());
        }
        info!(
            chat_id,
            user_id,
            display_name = dn.as_str(),
            username = un.as_str(),
            answer_len = answer.chars().count(),
            answer = truncate_for_log(answer, 500).as_str(),
            "answer received; verifying"
        );
        self.clear_timeout(chat_id, user_id).await;

        // Mark the user's message with a 👀 reaction so they get an instant
        // acknowledgment that the bot received it and is working on the
        // verification. Failures here are non-fatal.
        if let Err(e) = self
            .bot
            .set_message_reaction(ChatId(dm_chat_id), MessageId(message_id))
            .reaction(vec![ReactionType::Emoji {
                emoji: "👀".to_string(),
            }])
            .await
        {
            warn!(
                chat_id,
                user_id,
                display_name = dn.as_str(),
                username = un.as_str(),
                error = %e,
                "set_message_reaction failed"
            );
        }
        let typing = spawn_typing_indicator(self.bot.clone(), row.dm_chat_id);
        let verdict_result = self.llm.verify_answer(&prompt, &question, answer).await;
        typing.abort();

        let verdict = match verdict_result {
            Ok(v) => v,
            Err(e) => {
                error!(
                    chat_id,
                    user_id,
                    display_name = dn.as_str(),
                    username = un.as_str(),
                    error = %e,
                    "answer verification failed"
                );
                self.clone()
                    .finalize_rejection(
                        row,
                        Outcome::RejectedLlmError,
                        Some(answer.to_string()),
                        Some(format!("verification failed: {e}")),
                        Some(locale.rejected_llm_error.clone()),
                    )
                    .await;
                return Ok(());
            }
        };

        info!(
            chat_id,
            user_id,
            display_name = dn.as_str(),
            username = un.as_str(),
            verdict = if verdict.accept { "accept" } else { "reject" },
            reason = verdict.reason.as_str(),
            "verification verdict received"
        );
        if verdict.accept {
            self.clone()
                .finalize_acceptance(row, answer.to_string(), verdict.reason, locale)
                .await;
        } else {
            self.clone()
                .finalize_rejection(
                    row,
                    Outcome::RejectedWrong,
                    Some(answer.to_string()),
                    Some(verdict.reason),
                    Some(render_cooldown_template(
                        &locale.rejected_wrong,
                        self.config.cooldown.retry_after_secs,
                    )),
                )
                .await;
        }
        Ok(())
    }

    async fn arm_timeout(self: Arc<Self>, chat_id: i64, user_id: i64, deadline: i64) {
        let mut map = self.timeouts.lock().await;
        if let Some(prev) = map.remove(&(chat_id, user_id)) {
            prev.abort();
        }
        let engine = self.clone();
        let handle = tokio::spawn(async move {
            let now = unix_now();
            let sleep_for = (deadline - now).max(0) as u64;
            tokio::time::sleep(std::time::Duration::from_secs(sleep_for)).await;
            // Re-check the current row; someone may have advanced it.
            let pending = match engine.storage.get_pending(chat_id, user_id).await {
                Ok(Some(row)) if row.deadline == deadline => row,
                Ok(_) => return,
                Err(e) => {
                    error!(chat_id, user_id, error = %e, "timeout check: db error");
                    return;
                }
            };
            engine.clone().fire_timeout(pending).await;
        });
        map.insert((chat_id, user_id), handle);
    }

    async fn fire_timeout(self: Arc<Self>, row: PendingRow) {
        let chat_id = row.chat_id;
        let user_id = row.user_id;
        let locale = self
            .config
            .group(chat_id)
            .and_then(|g| g.locale.as_deref())
            .map(|k| self.locales.resolve(Some(k)))
            .unwrap_or_else(|| self.locales.resolve(None))
            .clone();
        let message = render_cooldown_template(
            &locale.rejected_timeout,
            self.config.cooldown.retry_after_secs,
        );
        // Distinguish bots that never pressed the button (commonly seen with
        // simple spam scripts that can't drive an inline keyboard) from real
        // users who started the verification but stalled on the answer.
        // Transient stages can't legitimately reach fire_timeout — recover()
        // routes them through fire_llm_error — but if they somehow do, treat
        // as no_answer defensively.
        let outcome = match row.stage {
            Stage::AwaitingButton => Outcome::RejectedNoButton,
            Stage::AwaitingAnswer | Stage::GeneratingQuestion | Stage::Verifying => {
                Outcome::RejectedNoAnswer
            }
        };
        // Treat the no-button-press timeout as background spam-bot noise so
        // it doesn't drown out logs for users who actually engage.
        if matches!(outcome, Outcome::RejectedNoButton) {
            debug!(
                chat_id,
                user_id,
                display_name = row.display_name.as_deref().unwrap_or(""),
                username = row.username.as_deref().unwrap_or(""),
                stage = ?row.stage,
                ?outcome,
                "verification timed out"
            );
        } else {
            info!(
                chat_id,
                user_id,
                display_name = row.display_name.as_deref().unwrap_or(""),
                username = row.username.as_deref().unwrap_or(""),
                stage = ?row.stage,
                ?outcome,
                "verification timed out"
            );
        }
        self.finalize_rejection(row, outcome, None, Some("timeout".into()), Some(message))
            .await;
    }

    /// Finalize an interrupted (transient-stage) row that we recovered from
    /// disk. Used at startup when the previous process died mid-LLM-call.
    async fn fire_llm_error(self: Arc<Self>, row: PendingRow) {
        let chat_id = row.chat_id;
        let user_id = row.user_id;
        let locale = self
            .config
            .group(chat_id)
            .and_then(|g| g.locale.as_deref())
            .map(|k| self.locales.resolve(Some(k)))
            .unwrap_or_else(|| self.locales.resolve(None))
            .clone();
        info!(
            chat_id,
            user_id,
            display_name = row.display_name.as_deref().unwrap_or(""),
            username = row.username.as_deref().unwrap_or(""),
            stage = ?row.stage,
            "recovering interrupted LLM call as rejection"
        );
        self.finalize_rejection(
            row,
            Outcome::RejectedLlmError,
            None,
            Some("verification interrupted by restart".into()),
            Some(locale.rejected_llm_error.clone()),
        )
        .await;
    }

    async fn finalize_acceptance(
        self: Arc<Self>,
        row: PendingRow,
        answer: String,
        reason: String,
        locale: Locale,
    ) {
        let chat_id = row.chat_id;
        let user_id = row.user_id;
        let dn = row.display_name.as_deref().unwrap_or("").to_string();
        let un = row.username.as_deref().unwrap_or("").to_string();
        let record = AuditRecord {
            chat_id,
            user_id,
            username: row.username.clone(),
            display_name: row.display_name.clone(),
            started_at: row.started_at,
            completed_at: unix_now(),
            question: row.question.clone(),
            answer: Some(answer),
            outcome: Outcome::Approved,
            reason: Some(reason),
        };
        if let Err(e) = self.storage.finalize(record, None).await {
            error!(
                chat_id,
                user_id,
                display_name = dn.as_str(),
                username = un.as_str(),
                error = %e,
                "failed to persist approved record"
            );
        }
        if let Err(e) = self
            .bot
            .send_message(ChatId(row.dm_chat_id), locale.approved.clone())
            .await
        {
            warn!(
                chat_id,
                user_id,
                display_name = dn.as_str(),
                username = un.as_str(),
                error = %e,
                "failed to send approved DM"
            );
        }
        if let Err(e) = self
            .bot
            .approve_chat_join_request(Recipient::Id(ChatId(chat_id)), UserId(user_id as u64))
            .await
        {
            error!(
                chat_id,
                user_id,
                display_name = dn.as_str(),
                username = un.as_str(),
                error = %e,
                "approve_chat_join_request failed"
            );
        }
        info!(
            chat_id,
            user_id,
            display_name = dn.as_str(),
            username = un.as_str(),
            "join request approved"
        );
        self.clear_timeout(chat_id, user_id).await;
    }

    async fn finalize_rejection(
        self: Arc<Self>,
        row: PendingRow,
        outcome: Outcome,
        answer: Option<String>,
        reason: Option<String>,
        user_message: Option<String>,
    ) {
        let chat_id = row.chat_id;
        let user_id = row.user_id;
        let dn = row.display_name.as_deref().unwrap_or("").to_string();
        let un = row.username.as_deref().unwrap_or("").to_string();
        let now = unix_now();
        let cooldown = if outcome.imposes_cooldown() {
            Some(now + self.config.cooldown.retry_after_secs as i64)
        } else {
            None
        };
        let record = AuditRecord {
            chat_id,
            user_id,
            username: row.username.clone(),
            display_name: row.display_name.clone(),
            started_at: row.started_at,
            completed_at: now,
            question: row.question.clone(),
            answer,
            outcome,
            reason,
        };
        if let Err(e) = self.storage.finalize(record, cooldown).await {
            error!(
                chat_id,
                user_id,
                display_name = dn.as_str(),
                username = un.as_str(),
                error = %e,
                "failed to persist rejection record"
            );
        }
        if let Some(text) = user_message
            && let Err(e) = self.bot.send_message(ChatId(row.dm_chat_id), text).await
        {
            warn!(
                chat_id,
                user_id,
                display_name = dn.as_str(),
                username = un.as_str(),
                error = %e,
                "failed to DM rejection"
            );
        }
        if let Err(e) = self
            .bot
            .decline_chat_join_request(Recipient::Id(ChatId(chat_id)), UserId(user_id as u64))
            .await
        {
            error!(
                chat_id,
                user_id,
                display_name = dn.as_str(),
                username = un.as_str(),
                error = %e,
                "decline_chat_join_request failed"
            );
        }
        // Same shaping as fire_timeout: silence the spam-bot path so the log
        // surfaces only declines that involve actual user engagement.
        if matches!(outcome, Outcome::RejectedNoButton) {
            debug!(
                chat_id,
                user_id,
                display_name = dn.as_str(),
                username = un.as_str(),
                outcome = outcome.as_str(),
                cooldown_until = cooldown,
                "join request declined"
            );
        } else {
            info!(
                chat_id,
                user_id,
                display_name = dn.as_str(),
                username = un.as_str(),
                outcome = outcome.as_str(),
                cooldown_until = cooldown,
                "join request declined"
            );
        }
        self.clear_timeout(chat_id, user_id).await;
    }

    /// Cool-down hit path: no pending row exists to finalize, so we only
    /// record a standalone audit entry, DM the user, and decline.
    async fn handle_cooldown_hit(
        self: &Arc<Self>,
        chat_id: i64,
        user: &User,
        dm_chat_id: i64,
        expires: i64,
        now: i64,
    ) {
        let dn = display_name(user);
        let un = user.username.clone().unwrap_or_default();
        debug!(
            chat_id,
            user_id = user.id.0,
            display_name = dn.as_str(),
            username = un.as_str(),
            cooldown_until = expires,
            remaining_secs = expires - now,
            "join request hit active cool-down; declining"
        );
        let group = self.config.group(chat_id);
        let locale = self
            .locale_for_group(group.and_then(|g| g.locale.as_deref()))
            .clone();
        let remaining_secs = (expires - now).max(60) as u64;
        let text = render(
            &locale.cooldown_notice,
            &[("minutes", &format_minutes(remaining_secs))],
        )
        .into_owned();
        if let Err(e) = self.bot.send_message(ChatId(dm_chat_id), text).await {
            warn!(
                chat_id,
                user_id = user.id.0,
                display_name = dn.as_str(),
                username = un.as_str(),
                error = %e,
                "failed to DM cooldown notice"
            );
        }
        let record = AuditRecord {
            chat_id,
            user_id: user.id.0 as i64,
            username: user.username.clone(),
            display_name: Some(dn.clone()),
            started_at: now,
            completed_at: now,
            question: None,
            answer: None,
            outcome: Outcome::RejectedCooldown,
            reason: Some("cooldown active".into()),
        };
        if let Err(e) = self.storage.finalize(record, None).await {
            error!(
                chat_id,
                user_id = user.id.0,
                display_name = dn.as_str(),
                username = un.as_str(),
                error = %e,
                "failed to persist cooldown audit"
            );
        }
        if let Err(e) = self
            .bot
            .decline_chat_join_request(Recipient::Id(ChatId(chat_id)), user.id)
            .await
        {
            error!(
                chat_id,
                user_id = user.id.0,
                display_name = dn.as_str(),
                username = un.as_str(),
                error = %e,
                "decline_chat_join_request failed (cooldown)"
            );
        }
    }

    async fn clear_timeout(self: &Arc<Self>, chat_id: i64, user_id: i64) {
        let mut map = self.timeouts.lock().await;
        if let Some(handle) = map.remove(&(chat_id, user_id)) {
            handle.abort();
        }
    }

    fn locale_for_group(&self, preferred: Option<&str>) -> &Locale {
        self.locales.resolve(preferred)
    }

    /// Resolve the locale that applies to a configured group, used by
    /// callback-query handlers that have only the chat id at hand.
    pub fn locale_for_chat(&self, chat_id: i64) -> Locale {
        let preferred = self.config.group(chat_id).and_then(|g| g.locale.as_deref());
        self.locales.resolve(preferred).clone()
    }

    pub fn storage(&self) -> &Storage {
        &self.storage
    }
}

/// Spawn a task that re-emits a `typing` chat action every 4 seconds (the
/// indicator clears after ~5s server-side, so the cadence keeps it
/// continuously visible). The caller aborts the returned handle once the
/// long-running operation completes.
fn spawn_typing_indicator(bot: Bot, dm_chat_id: i64) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            if let Err(e) = bot
                .send_chat_action(ChatId(dm_chat_id), ChatAction::Typing)
                .await
            {
                warn!(dm_chat_id, error = %e, "send_chat_action(typing) failed");
            }
            tokio::time::sleep(std::time::Duration::from_secs(4)).await;
        }
    })
}

fn render_welcome(locale: &Locale, group_name: Option<&str>, user: &str, secs: u64) -> String {
    let minutes = format_minutes(secs);
    match group_name {
        Some(name) => render(
            &locale.welcome,
            &[("user", user), ("group", name), ("minutes", &minutes)],
        )
        .into_owned(),
        None => render(
            &locale.welcome_no_group_name,
            &[("user", user), ("minutes", &minutes)],
        )
        .into_owned(),
    }
}

fn render_cooldown_template(template: &str, secs: u64) -> String {
    render(template, &[("minutes", &format_minutes(secs))]).into_owned()
}

fn format_minutes(secs: u64) -> String {
    let m = secs / 60;
    let rem = secs % 60;
    if rem == 0 {
        m.to_string()
    } else {
        format!("{m}.{:02}", (rem * 100) / 60)
    }
}

fn display_name(user: &User) -> String {
    if !user.first_name.is_empty() {
        if let Some(last) = &user.last_name {
            return format!("{} {}", user.first_name, last);
        }
        return user.first_name.clone();
    }
    user.username
        .clone()
        .map(|u| format!("@{u}"))
        .unwrap_or_else(|| format!("user {}", user.id.0))
}

/// Trim a user-supplied string to at most `max_chars` characters for safe
/// inclusion in log fields. Replaces newlines with spaces so the message
/// stays on one line. Appends an ellipsis marker when truncation occurs.
fn truncate_for_log(s: &str, max_chars: usize) -> String {
    let mut out = String::with_capacity(s.len().min(max_chars * 4));
    for (count, c) in s.chars().enumerate() {
        if count >= max_chars {
            out.push('…');
            return out;
        }
        out.push(if c == '\n' || c == '\r' { ' ' } else { c });
    }
    out
}

fn unix_now() -> i64 {
    jiff::Timestamp::now().as_second()
}
