# ai_assistant

Lightweight, privacy-first assistant runtime for Raspberry Pi Zero 2 WH class hardware.

`ai_assistant/` is the new implementation target for the local assistant. It stays isolated from `rust_api/`, which remains the earlier prototype.

## Design

- Rust binary with minimal dependencies
- Offline-first runtime after deployment
- SQLite state via `sqlite3`
- Markdown transcripts, summaries, profiles, and notes
- Deterministic harness-style prompt assembly
- Simple RAG over `.md`, `.txt`, and `.json`
- Memory TTL cleanup for compact long-term state
- Direct local `llama-cli` inference by default
- Telegram DM onboarding and long-poll runtime built into the CLI
- Scheduled jobs and a terminal-first operator workflow
- File-backed skills that can be installed, selected by trigger, and run through allowlisted tools

## Command Surface

- `assistant onboard telegram`
- `assistant telegram status`
- `assistant telegram pending`
- `assistant telegram approve <code>`
- `assistant telegram deny <code>`
- `assistant doctor`
- `assistant chat`
- `assistant search`
- `assistant ingest`
- `assistant task add`
- `assistant tool list`
- `assistant tool add <command>`
- `assistant tool remove <command>`
- `assistant skill create`
- `assistant skill install`
- `assistant skill list`
- `assistant skill run`
- `assistant memory search`
- `assistant summarize`
- `assistant schedule add`
- `assistant rag index`
- `assistant jobs run --once`
- `assistant serve`

## Key Components

- `src/adapters/llama_cpp.rs`: local llama.cpp HTTP adapter with retry and timeout handling
- `src/adapters/telegram.rs`: Telegram Bot API long polling and DM parsing
- `src/adapters/storage.rs`: SQLite adapter and schema initialization
- `src/core/harness.rs`: identity/system/user/context/memory/tool/task/safety prompt layers
- `src/core/memory.rs`: long-term + short-term memory, summaries, and compaction
- `src/core/tasks.rs`: lightweight task engine
- `src/core/scheduler.rs`: recurring jobs and service loop
- `src/core/service.rs`: shared chat execution for CLI and Telegram
- `src/core/skills.rs`: local skill registry, trigger matching, and deterministic skill execution
- `src/core/telegram.rs`: pairing, allowlist, and Telegram runtime state
- `src/core/rag.rs`: low-cost document indexing and retrieval
- `src/core/tools.rs`: allowlist-based local tool execution

## Layout

```text
ai_assistant/
├── benchmarks/
├── config/
├── data/
│   ├── conversations/
│   ├── memory/
│   ├── notes/
│   ├── profiles/
│   ├── skills/
│   ├── summaries/
│   └── tasks/
├── deploy/
├── scripts/
├── src/
│   ├── adapters/
│   └── core/
├── tests/
├── install.sh
└── setup.md
```

## Quick Start

```bash
cd ai_assistant
./install.sh
./target/release/ai_assistant
./target/release/ai_assistant doctor
./target/release/ai_assistant serve
```

## First Run

Run `./target/release/ai_assistant` with no arguments on a fresh workspace. If onboarding is incomplete, the CLI launches the Telegram wizard automatically.

The wizard will:

- validate the BotFather token with `getMe`
- write `config/telegram.json`
- write or normalize `config/llm.json` for direct local `llama-cli`
- long-poll Telegram until the owner's first DM arrives
- create and approve the first pairing
- send a live test reply
- offer the systemd install commands

## Skills and Tools

Tools are bare executable names stored in `config/tools.json`. Add them with:

```bash
assistant tool add printf
assistant tool list
```

Skills are markdown files stored in `data/skills/`. They declare triggers, useful tools, instructions, and optional deterministic steps:

```bash
assistant skill create "Runtime Note" \
  --description "Capture runtime notes" \
  --triggers "note,runtime" \
  --tools "printf" \
  --instructions "Append the requested task to a markdown note." \
  --step "append_markdown: data/notes/runtime.md | {{task}}" \
  --step "command: printf ok"

assistant skill run auto "write a runtime note"
```

Chat prompt assembly automatically includes skills whose name or triggers match the user message, so the local model can choose from the currently installed capabilities.
