use std::sync::Arc;

use teloxide::Bot;
use teloxide::dispatching::{Dispatcher, UpdateFilterExt};
use teloxide::dptree;
use teloxide::payloads::AnswerCallbackQuerySetters;
use teloxide::prelude::Requester;
use teloxide::types::{CallbackQuery, ChatJoinRequest, ChatKind, Message, Update};
use tracing::{error, warn};

use crate::verification::{Engine, NOOP_CALLBACK, START_CALLBACK};

pub async fn run(bot: Bot, engine: Arc<Engine>) {
    let handler = dptree::entry()
        .branch(Update::filter_chat_join_request().endpoint(on_chat_join_request))
        .branch(Update::filter_callback_query().endpoint(on_callback_query))
        .branch(Update::filter_message().endpoint(on_message));

    Dispatcher::builder(bot, handler)
        .dependencies(dptree::deps![engine])
        .enable_ctrlc_handler()
        .build()
        .dispatch()
        .await;
}

async fn on_chat_join_request(
    engine: Arc<Engine>,
    request: ChatJoinRequest,
) -> Result<(), anyhow::Error> {
    let chat_id = request.chat.id.0;
    let chat_title = request.chat.title().map(str::to_owned);
    let user = request.from.clone();
    let dm_chat_id = request.user_chat_id.0;
    if let Err(e) = engine
        .on_join_request(chat_id, chat_title, user, dm_chat_id)
        .await
    {
        error!(chat_id, error = %e, "on_join_request failed");
    }
    Ok(())
}

async fn on_callback_query(
    bot: Bot,
    engine: Arc<Engine>,
    query: CallbackQuery,
) -> Result<(), anyhow::Error> {
    let data = query.data.as_deref();
    if matches!(data, Some(d) if d == NOOP_CALLBACK) {
        // The "Generating..." placeholder button — silently ack so the
        // client clears its spinner. No further work to do.
        if let Err(e) = bot.answer_callback_query(query.id.clone()).await {
            warn!(error = %e, "answer_callback_query failed");
        }
        return Ok(());
    }
    let is_start = matches!(data, Some(d) if d == START_CALLBACK);
    if !is_start {
        if let Err(e) = bot.answer_callback_query(query.id.clone()).await {
            warn!(error = %e, "answer_callback_query failed");
        }
        return Ok(());
    }

    let Some(message) = query.regular_message() else {
        if let Err(e) = bot.answer_callback_query(query.id.clone()).await {
            warn!(error = %e, "answer_callback_query failed");
        }
        return Ok(());
    };
    let user_id = query.from.id.0 as i64;
    let dm_chat_id = message.chat.id.0;
    let welcome_msg_id = message.id.0;

    // Look up the pending row up front so we can resolve the group's locale
    // for the toast text and answer the callback in the right language.
    let pending = engine
        .storage()
        .find_awaiting_button_by_dm(dm_chat_id)
        .await;
    let Ok(Some(row)) = pending else {
        if let Err(e) = bot.answer_callback_query(query.id.clone()).await {
            warn!(error = %e, "answer_callback_query failed");
        }
        return Ok(());
    };
    if row.user_id != user_id {
        if let Err(e) = bot.answer_callback_query(query.id.clone()).await {
            warn!(error = %e, "answer_callback_query failed");
        }
        return Ok(());
    }

    let locale = engine.locale_for_chat(row.chat_id);
    if let Err(e) = bot
        .answer_callback_query(query.id.clone())
        .text(locale.generating_question.clone())
        .await
    {
        warn!(error = %e, "answer_callback_query failed");
    }

    if let Err(e) = engine
        .on_button_press(row.chat_id, user_id, welcome_msg_id)
        .await
    {
        error!(chat_id = row.chat_id, user_id, error = %e, "on_button_press failed");
    }
    Ok(())
}

async fn on_message(engine: Arc<Engine>, msg: Message) -> Result<(), anyhow::Error> {
    // Only handle messages in private chats with the bot.
    if !matches!(msg.chat.kind, ChatKind::Private(_)) {
        return Ok(());
    }
    let Some(from) = &msg.from else {
        return Ok(());
    };
    let Some(text) = msg.text() else {
        return Ok(());
    };
    let user_id = from.id.0 as i64;
    let dm_chat_id = msg.chat.id.0;
    let message_id = msg.id.0;
    if let Err(e) = engine
        .on_user_answer(user_id, dm_chat_id, message_id, text)
        .await
    {
        error!(user_id, error = %e, "on_user_answer failed");
    }
    Ok(())
}
