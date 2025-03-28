# Bouncer

Bouncer is an LLM-powered Telegram bot designed to verify and approve group chat join requests by generating and validating topic-based questions.

<p align="center">
   <img src="https://github.com/user-attachments/assets/4b7ef412-8a70-4db2-ab94-ddb7bc8c4af7"/>
</p>

## Methodology

This bot uses Telegram Bot API 5.4's [approveChatJoinRequest](https://core.telegram.org/bots/api#approvechatjoinrequest) and [declineChatJoinRequest](https://core.telegram.org/bots/api#declinechatjoinrequest) methods to approve or reject join requests. When a user requests to join the group, the bot proactively sends them a topic-based question. If they answer correctly, their request is approved.

In the traditional approach, users are first allowed to join the group, and then the bot restricts their permissions so they cannot send messages. To gain full access, they must manually message the bot and answer a CAPTCHA. Bouncer's approach has the following advantages:

- **Immune to flooding attacks**: The bot does not need to restrict the user's permissions, so it is not vulnerable to flooding attacks, where the attacker controls tens or hundreds of accounts to join the group, overwhelming the bot and lets some of them pass.
- **Ease of use**: The approval process is simplified by the use of Telegram's native join request system. The bot does not need to hack the permissions system to restrict the user's permissions, and the user does not need to manually message the bot to answer a CAPTCHA.

## Deployment

You will first need to make a copy of the `configs/bouncer.example.yaml` file and rename it to `bouncer.yaml`. You can then configure the bot by editing the `bouncer.yaml` file. You will need to, at a minimum, set the following configuration options:

- `telegram_bot_token`: The Telegram bot token.
- `generative_ai_backend`: The backend to use for the generative AI model. You can choose between `openai`, `ollama`, and `gemini`.
- The API key for OpenAI or Ollama, depending on the backend you choose.

### Option 1: With Docker

```bash
docker run -d --name bouncer \
  -v $PWD/bouncer.yaml:/data/bouncer.yaml \
  -v $PWD/bouncer.db:/data/bouncer.db \
  ghcr.io/k4yt3x/bouncer:0.1.0
```

### Option 2: Without Docker

1. Clone the repository:
   ```bash
   git clone https://github.com/k4yt3x/bouncer.git
   cd bouncer
   ```
2. Create a virtual environment and activate it:
   ```bash
   python -m venv venv
   source venv/bin/activate
   ```
3. Install the required dependencies:
   ```bash
   pip install -U pdm
   pdm install
   ```
4. Run the bot:
   ```bash
   python -m bouncer
   ```

## Configuration

1. Add the bot to your Telegram group.
2. Add the group ID to the `allowed_groups` table in the database.
3. Configure the topic of this group by sending `/settopic <topic>` to the bot in the group chat. Alternatively, you can also add the group's topic to the `group_topics` table in the database.

## License

Bouncer is licensed under [GNU AGPL version 3](https://www.gnu.org/licenses/agpl-3.0.txt).

![AGPLv3](https://www.gnu.org/graphics/agplv3-155x51.png)
