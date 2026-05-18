# Goal: Build a Lightweight Local AI Assistant for Raspberry Pi 2 WH

## Objective

Design and implement a mini, lightweight, privacy-first AI Assistant optimized for the Raspberry Pi 2 WH with a locally running LLM using llama.cpp.

The assistant must follow principles inspired by:
- OpenClaw architecture patterns
- Harness Engineering methodology
- Modular local-first agent systems
- Minimal RAM and CPU footprint
- Deterministic and stable execution

The assistant must operate fully offline after deployment.

--------------------------------------------------
CORE MISSION
--------------------------------------------------

Create a reliable AI runtime capable of:
- Conversational interaction
- Memory retention
- Lightweight task execution
- Local reasoning
- Prompt orchestration
- Autonomous scheduled operations
- Context compaction
- Persistent identity and personalization
- Simple RAG retrieval
- Local automation

while remaining efficient enough to run on constrained hardware.

--------------------------------------------------
TECHNICAL CONSTRAINTS
--------------------------------------------------

Target Hardware:
- Raspberry Pi 2 WH
- ARM architecture
- Low RAM / constrained CPU
- Passive cooling environment

Local LLM Runtime:
- GGUF quantized model
- Managed through llama.cpp
- CPU-only inference
- Optimized context size

Performance Targets:
- Low memory usage
- Fast startup
- Minimal background processes
- Graceful degradation under load
- Stable long-running execution

--------------------------------------------------
MANDATORY TECH STACK
--------------------------------------------------

Programming Language:
- Rust

Database:
- SQLite

Storage:
- Markdown (.md) files
- JSON configuration files

Runtime Design:
- CLI-first
- Service-oriented modules
- Async Rust where needed
- Minimal dependencies

--------------------------------------------------
REQUIRED SYSTEM COMPONENTS
--------------------------------------------------

1. LLM Runtime Adapter

Responsibilities:
- Communicate with local llama.cpp server
- Handle inference requests
- Manage prompt formatting
- Support streaming responses
- Retry and timeout handling

Features:
- System prompts
- User prompts
- Assistant prompts
- Context window management
- Token budgeting

--------------------------------------------------

2. Harness Engineering Core

Implement strict prompt orchestration patterns.

Required Layers:
- Identity Layer
- System Instruction Layer
- User Intent Layer
- Context Injection Layer
- Memory Layer
- Tool Context Layer
- Task Layer
- Safety Layer

Responsibilities:
- Prompt assembly
- Context ordering
- Context pruning
- Token-efficient composition
- Deterministic formatting

--------------------------------------------------

3. Long-Term Memory

Storage:
- SQLite + Markdown hybrid

Features:
- Conversation summaries
- User preferences
- Important events
- Persistent knowledge
- Semantic retrieval
- Memory tagging

Requirements:
- Lightweight indexing
- Incremental updates
- Retrieval scoring
- Memory expiration policy

--------------------------------------------------

4. Short-Term Memory

Features:
- Active conversation buffer
- Recent actions
- Temporary context
- Session state

Requirements:
- Sliding context window
- Automatic pruning
- Token-aware management

--------------------------------------------------

5. Context Compaction Engine

Purpose:
Reduce token usage while preserving reasoning continuity.

Features:
- Conversation summarization
- Compression pipelines
- Priority preservation
- Important fact extraction

Requirements:
- Compact markdown summaries
- Incremental compaction
- Threshold-triggered execution

--------------------------------------------------

6. Identity System

Features:
- Assistant persona
- Persistent identity
- Behavioral configuration
- Communication style

Stored In:
- Markdown profile
- JSON config

--------------------------------------------------

7. Task Engine

Features:
- Create tasks
- Update tasks
- Complete tasks
- Prioritize tasks
- Local persistence

Storage:
- SQLite

Optional:
- Dependency graph
- Retry policies

--------------------------------------------------

8. Scheduler / Cron Engine

Features:
- Scheduled prompts
- Automated jobs
- Reminder execution
- Background maintenance

Example Jobs:
- Memory compaction
- Daily summaries
- RAG indexing
- Cleanup tasks

--------------------------------------------------

9. Lightweight RAG System (Optional but Preferred)

Goal:
Enable document retrieval without heavy vector databases.

Suggested Approach:
- SQLite FTS5
- Small embeddings
- Markdown document indexing
- Keyword + semantic hybrid retrieval

Supported Files:
- .md
- .txt
- .json

--------------------------------------------------

10. Tool Execution Layer

Features:
- Local shell commands
- File operations
- Markdown editing
- Task management
- Scheduler interaction

Security:
- Sandboxed execution
- Allowlist-based commands

--------------------------------------------------
SUGGESTED PROJECT ARCHITECTURE
--------------------------------------------------

assistant/
├── core/
│   ├── harness/
│   ├── prompts/
│   ├── memory/
│   ├── scheduler/
│   ├── rag/
│   ├── tasks/
│   └── identity/
│
├── adapters/
│   ├── llama_cpp/
│   └── storage/
│
├── data/
│   ├── memory/
│   ├── tasks/
│   ├── summaries/
│   └── profiles/
│
├── configs/
│
├── tests/
│
└── cli/

--------------------------------------------------
FUNCTIONAL REQUIREMENTS
--------------------------------------------------

CLI Commands:

assistant chat
assistant task add
assistant memory search
assistant summarize
assistant schedule add
assistant rag index

--------------------------------------------------
DEVELOPMENT REQUIREMENTS
--------------------------------------------------

Codex Must:
- Build incrementally
- Test every module
- Benchmark memory usage
- Validate on low-resource simulation
- Ensure ARM compatibility
- Produce deployment scripts
- Create reproducible builds

--------------------------------------------------
TESTING REQUIREMENTS
--------------------------------------------------

Mandatory Tests:

Unit Tests:
- Memory
- Scheduler
- Prompt builder
- Task engine

Integration Tests:
- llama.cpp connectivity
- RAG retrieval
- Context compaction

Stress Tests:
- Long conversations
- Low memory conditions
- Multiple scheduled jobs

Raspberry Pi Validation:
- CPU utilization
- RAM utilization
- Thermal stability
- Boot persistence

--------------------------------------------------
DEPLOYMENT REQUIREMENTS
--------------------------------------------------

Deliverables:

1. setup.md
- Full setup documentation

2. install.sh
- Automated deployment script

3. systemd service
- Background service management

4. Example configurations
- LLM config
- Memory config
- Scheduler config

5. Benchmark results
- Token speed
- RAM usage
- Latency

--------------------------------------------------
ENGINEERING PRINCIPLES
--------------------------------------------------

Priorities:
1. Stability
2. Determinism
3. Simplicity
4. Low resource usage
5. Offline-first
6. Maintainability
7. Modular design

--------------------------------------------------
EXPLICIT NON-GOALS
--------------------------------------------------

Avoid:
- Heavy frameworks
- Kubernetes
- GPU assumptions
- Cloud dependencies
- Large vector databases
- Electron GUIs
- Web-scale architectures

--------------------------------------------------
EXPECTED FINAL OUTCOME
--------------------------------------------------

A compact local AI assistant capable of:
- Running continuously on Raspberry Pi 2 WH
- Using local llama.cpp inference
- Managing memory and tasks
- Performing scheduled reasoning
- Supporting lightweight RAG
- Operating fully offline
- Maintaining stable long-term operation with minimal resources

The system should resemble a minimal autonomous cognitive runtime rather than a chatbot wrapper.