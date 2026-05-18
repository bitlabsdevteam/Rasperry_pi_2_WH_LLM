# Setup

## Requirements

- Rust toolchain
- `sqlite3`
- `curl`
- a local `llama-cli` binary
- a local GGUF model file

## Local bootstrap

```bash
cd ai_assistant
./scripts/release.sh
./target/release/ai_assistant
```

On a fresh workspace, `assistant` launches the Telegram onboarding wizard instead of a static help screen. The explicit entrypoint is:

```bash
./target/release/ai_assistant onboard telegram
```

That flow validates the bot token, writes `config/telegram.json`, resolves local `llama-cli` defaults, waits for the first owner DM, approves the pairing, and sends a live Telegram reply.

## Expected runtime layout

- `data/assistant.db`: SQLite state
- `config/telegram.json`: Telegram bot and pairing configuration
- `data/conversations/`: markdown transcripts
- `data/summaries/`: compaction outputs
- `data/profiles/assistant.md`: persistent persona
- `data/notes/`: user-authored markdown notes for ingestion

## Example usage

```bash
./target/release/ai_assistant doctor
./target/release/ai_assistant telegram status
./target/release/ai_assistant task add "Review overnight logs"
./target/release/ai_assistant rag index data/notes
./target/release/ai_assistant schedule add compact 15 summarize
./target/release/ai_assistant schedule add cleanup 60 "cleanup memories"
./target/release/ai_assistant serve --once
./target/release/ai_assistant chat --message "Summarize pending tasks"
```

## Raspberry Pi validation

Run `./scripts/pi_validate.sh` on the Pi to collect:

- CPU utilization samples
- resident memory samples
- thermal data
- reboot persistence checks

## Raspberry Pi notes

- Install `sqlite3` on the Pi before running the assistant tests or scheduler-backed commands.
- The default inference path is direct local `llama-cli`; the HTTP endpoint fields remain optional advanced configuration.
- Telegram v1 is private-DM-only and uses long polling instead of webhooks.
