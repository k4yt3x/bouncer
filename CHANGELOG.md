# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Per-group `enabled` flag (default true) silences a group without removing it from the config.
- OpenAI-compatible LLM client with configurable base URL, model, and temperature.
- Prompt-injection-resistant answer verification (tag-wrapped user input, tolerant JSON parsing).
- SQLite-backed persistence of cool-downs, audit log, and in-flight verifications.
- Restart recovery of pending verifications: timers re-armed, expired ones finalized as timeouts.
- Built-in English and Simplified Chinese UI strings with per-group overrides.
- `stats` subcommand reports approvals/rejections globally and per group.
- `stats` separates `no_button` (no start press) and `no_answer` (no reply) to spot spam bots.
- Generating-question UX: callback toast, button-label swap, and recurring typing chat action.
- Verifying-answer UX: 👀 reaction on the answer message plus recurring typing chat action.
- Question generation feeds the last N per-group questions to the LLM as a no-repeat list.
- INFO logs cover the full flow: join, button press, question, answer, verdict, decision, timeout.
- Logs and audit rows now include the user's display name and `@username` alongside `user_id`.

### Changed

- Rewrote Bouncer from Python to Rust; old Python implementation removed, not backwards-compatible.
- LLM/transport errors no longer impose a cool-down; only wrong-answer and timeout rejections do.

### Fixed

- Atomically claim the pending row before LLM calls so timeouts can't decline mid-verification.
