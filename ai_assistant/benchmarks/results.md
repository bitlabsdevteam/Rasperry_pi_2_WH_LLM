# Benchmark Results

These values include both local dry-run baselines and live Raspberry Pi validation notes.

## Metrics

- Release help/startup latency: `0.78s real` via `/usr/bin/time ./target/release/ai_assistant help`
- CLI task list latency: `0.01s real` via `/usr/bin/time ./target/release/ai_assistant task list`
- Degraded chat latency without llama.cpp server: `0.05s real` via `/usr/bin/time ./target/release/ai_assistant chat --message "ping"`
- Peak memory (developer machine): not collected because sandboxed `ps` access was denied during background-process inspection
- Raspberry Pi ARM compile check: `cargo check --target armv7-unknown-linux-gnueabihf` passed locally
- Raspberry Pi on-device test suite: `cargo test` passed on `raspiidb` after installing `sqlite3`
- Raspberry Pi service idle RSS: `2036 KB` with `./target/release/ai_assistant serve`
- Raspberry Pi temperature samples: `55.8C` at service idle, `63.4C` during local inference
- Raspberry Pi reboot persistence: `ai_assistant.service` remained active after reboot and `data/assistant.db` persisted
- Raspberry Pi llama.cpp speed samples: `Prompt: 11.2 t/s | Generation: 5.9 t/s` and `Prompt: 11.0 t/s | Generation: 5.6 t/s` with `SmolLM2-135M-Instruct.Q4_K_M.gguf`

## Raspberry Pi Validation Plan

- Extend the validation window to several hours to establish longer thermal and scheduler stability.
- Capture repeated inference latency samples under realistic workloads.
- Re-run the same commands after any config or model changes.
