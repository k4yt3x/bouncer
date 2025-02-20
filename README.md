# Bouncer

A bot that verifies and approves Telegram join requests by generating and validating topic-based questions using LLMs.

## Features

- **Automated Verification**: Generates topic-based questions and validates user responses before approving join requests.
- **Scalable Handling**: Uses Telegram Bot API 5.4’s `approveChatJoinRequest`, preventing overload during high join request volumes.
- **Proactive Messaging**: Sends messages to users upon request submission, a feature not available with standard bot interactions.
- **Multi-Backend Support**: Compatible with OpenAI, Ollama, and Gemini APIs.
- **Flexible Approval**: Can operate in automated mode or collect responses for manual admin approval.
