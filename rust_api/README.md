# rust_api

Pi-safe Rust inference API that wraps local `llama.cpp` `llama-cli` behind a narrow OpenAI-style surface:

- `POST /v1/chat/completions`
- `GET /health`

The public success payload intentionally excludes raw `stdout`, `stderr`, binary paths, model paths, prompt echo, and timing/debug internals.

## Runtime discovery

By default the server searches for:

- `llama-cli` in `../ec2_bin`, `../llama.cpp/build/bin`, and similar common paths
- `SmolLM2-135M-Instruct.Q4_K_M.gguf` in the project root and related model paths

Override with environment variables if needed:

```bash
LLAMA_BINARY=/absolute/path/to/llama-cli
LLAMA_MODEL=/absolute/path/to/model.gguf
LLAMA_MODEL_ALIAS=smollm2-135m-instruct
LLAMA_HOST=0.0.0.0
LLAMA_PORT=8080
LLAMA_THREADS=2
LLAMA_TIMEOUT=180
LLAMA_CONTEXT_SIZE=128
LLAMA_MAX_CONCURRENCY=1
```

## Run

```bash
cd rust_api
cargo run
```

## Example request

```bash
curl -s http://127.0.0.1:8080/v1/chat/completions \
  -H 'content-type: application/json' \
  -d '{
    "model": "smollm2-135m-instruct",
    "messages": [
      {"role": "system", "content": "You are concise."},
      {"role": "user", "content": "Write a haiku about edge inference."}
    ],
    "max_tokens": 64,
    "temperature": 0.7,
    "top_p": 0.95,
    "stream": false
  }'
```

## SSE streaming

Set `"stream": true` to receive lightweight SSE chunks as the local `llama-cli` process writes tokens:

```bash
curl -N http://127.0.0.1:8080/v1/chat/completions \
  -H 'content-type: application/json' \
  -d '{
    "messages": [
      {"role": "user", "content": "Reply with a short sentence."}
    ],
    "max_tokens": 32,
    "stream": true
  }'
```

The stream emits OpenAI-style `chat.completion.chunk` payloads and ends with `data: [DONE]`.

## Example response

```json
{
  "id": "chatcmpl-...",
  "object": "chat.completion",
  "created": 1710000000,
  "model": "smollm2-135m-instruct",
  "choices": [
    {
      "index": 0,
      "message": {
        "role": "assistant",
        "content": "Edge winds hum softly..."
      },
      "finish_reason": "stop"
    }
  ]
}
```

## Notes

- Streaming uses SSE and forwards small stdout chunks directly instead of buffering the full completion first.
- Requests are capped to a small body size and a single in-flight inference by default.
- Busy-device conditions return `429 rate_limit_exceeded` immediately instead of waiting in a queue.
