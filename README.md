# Bouncer

Bouncer is an LLM-powered Telegram bot that gates entry to private groups by challenging prospective members with topic-specific questions.

<p align="center">
   <img src="https://github.com/user-attachments/assets/4b7ef412-8a70-4db2-ab94-ddb7bc8c4af7"/>
</p>

## Methodology

Bouncer uses Telegram's native [`approveChatJoinRequest`](https://core.telegram.org/bots/api#approvechatjoinrequest) / [`declineChatJoinRequest`](https://core.telegram.org/bots/api#declinechatjoinrequest) so users never enter the group until they pass verification. The flow is:

1. A user requests to join an enrolled group.
2. Bouncer DMs them with a "Start Verification" button. (No LLM call yet — this filters out unsophisticated bots that can't interact with inline keyboards.)
3. After the button is pressed, Bouncer asks the configured LLM to generate a topic question and sends it.
4. The user replies; Bouncer asks the LLM to judge the answer.
5. On accept, Bouncer approves the join request. On reject (wrong answer or timeout), it declines and starts a cool-down before another attempt is allowed.

Compared to the more common "let everyone in, then mute them until they pass a CAPTCHA" approach this:

- **Resists flooding:** the bot doesn't manage in-group permissions, so flooding the group with fake accounts can't bypass it.
- **Stays inside Telegram's UX:** there's no need to DM the bot first or wait for a mute to lift.

## Configuration

All configuration lives in a single YAML file. The database (`bouncer.db`) is reserved for ephemeral state — pending verifications, cool-downs, and an audit log — and never holds settings.

1. Copy the example config and edit it:

   ```bash
   cp configs/bouncer.example.yaml configs/bouncer.yaml
   ```

2. At minimum, set:

   - `telegram.bot_token` — bot token from [@BotFather](https://t.me/BotFather).
   - `llm.api_key` — API key for OpenAI or any OpenAI-compatible endpoint (OpenRouter, a self-hosted llama.cpp / vLLM, etc.).
   - `llm.base_url` and `llm.model` — leave defaults for hosted OpenAI, or change for a compatible provider.
   - `groups[].id` and `groups[].question_prompt` — the groups Bouncer should gate, and the prompt that drives question generation for each. Group ids for supergroups are negative integers (e.g. `-1001234567890`).

Each group may also set:

- `enabled: false` — temporarily silence Bouncer for that group without removing it.
- `locale: zh-CN` — override the global UI language for that group. Built-in locales are `en` and `zh-CN`.

Groups not listed in the config are ignored — their join requests are left for human admins to handle.

## Deployment

### Option 1: Docker

Mount a host directory for the YAML config + SQLite database (the DB uses WAL, which writes `-shm` / `-wal` sidecar files, so mount the directory rather than individual files):

```bash
mkdir -p data && cp configs/bouncer.example.yaml data/bouncer.yaml
# edit data/bouncer.yaml ...

docker run -d --name bouncer \
  -v "$PWD/data":/data \
  ghcr.io/k4yt3x/bouncer:$TAG
```

The image's entrypoint already passes `-c /data/bouncer.yaml -d /data/bouncer.db`.

### Option 2: Build from source

```bash
git clone https://github.com/k4yt3x/bouncer.git
cd bouncer
cargo build --release
./target/release/bouncer -c configs/bouncer.yaml -d bouncer.db
```

## CLI

```text
bouncer [-c CONFIG] [-d DATABASE] [COMMAND]

Commands:
  run             Run the bot (default if omitted).
  stats           Print verification stats globally and per group.
                  Flags: -g/--group <id>, -s/--since <unix-seconds>
```

Logging is controlled by `RUST_LOG` (e.g. `RUST_LOG=bouncer=debug,teloxide=info`).

## License

Bouncer is licensed under [GNU AGPL version 3](https://www.gnu.org/licenses/agpl-3.0.txt).

![AGPLv3](https://www.gnu.org/graphics/agplv3-155x51.png)
