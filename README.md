# Raspberry Pi Zero 2 WH LLM Sandbox

This repository packages a small local-LLM playground for a Raspberry Pi Zero 2 WH. It contains two API implementations that shell out to `llama-cli`, helper scripts for remote checks, and setup notes for reproducing the environment on the Pi and on an EC2 build host.

## What is in this repo

- `raspi_llama_api/`: FastAPI service that runs `llama-cli` and exposes a lightweight JSON API.
- `rust_api/`: Axum-based API with OpenAI-style `/v1/chat/completions` support and SSE streaming.
- `ec2_bin/`: Cross-compiled `llama.cpp` binaries copied from the EC2 build machine.
- `scripts/`: Helper scripts for SSH and remote inference checks.
- `setup.md`: Pi provisioning notes.
- `Rasperry_LLM_Setup.MD`: Cross-compilation and deployment guide.

## Architecture

The workflow is split across two machines:

1. Build `llama.cpp` ARM64 binaries on a stronger Linux box such as EC2.
2. Copy those binaries and a small GGUF model onto the Raspberry Pi.
3. Start either the Python or Rust API on the Pi.
4. Send prompts to the API, which invokes `llama-cli` underneath.

This keeps the Pi focused on inference and API serving while heavy compilation happens elsewhere.

## Components

### Python API

`raspi_llama_api/app.py` is a FastAPI server that:

- locates `llama-cli` from several candidate directories
- locates the `SmolLM2-135M-Instruct.Q4_K_M.gguf` model
- serializes requests through a semaphore by default
- runs `llama-cli` with timeout and output cleanup logic

Install dependencies:

```bash
python3 -m venv .venv
source .venv/bin/activate
pip install -r raspi_llama_api/requirements.txt
```

Run the server:

```bash
uvicorn raspi_llama_api.app:app --host 0.0.0.0 --port 8000
```

### Rust API

`rust_api/` is an Axum service that exposes:

- `GET /health`
- `POST /v1/chat/completions`
- server-sent events when `stream=true`

Build and run:

```bash
cd rust_api
cargo run
```

By default, the Rust service binds to the host and port configured in its environment-driven config and forwards prompts to the local `llama-cli` runtime.

## Model and binary expectations

The APIs expect:

- a `llama-cli` executable in one of the checked binary locations
- the `SmolLM2-135M-Instruct.Q4_K_M.gguf` model in one of the checked model locations

The large GGUF model files are intentionally excluded from git because standard GitHub repositories reject files larger than 100 MB.

## Security notes

- Secrets such as `cred.txt` and `my_clawbot_key.pem` are ignored by git.
- Local virtual environments, logs, Rust build output, and GGUF model files are also ignored.
- Avoid committing host-specific SSH configuration or live credentials into the repository.

## Suggested quick start

1. Provision the Pi using [setup.md](/Users/davidbong/Documents/Rasperry_2WH/setup.md).
2. Build or copy the ARM64 `llama.cpp` binaries using [Rasperry_LLM_Setup.MD](/Users/davidbong/Documents/Rasperry_2WH/Rasperry_LLM_Setup.MD).
3. Place the model and binaries in the expected paths.
4. Start either the Python API or the Rust API.
5. Verify the runtime with the Rust `/health` endpoint or a test generation request.

## Repository status

This repo is intended to track the project code, setup notes, helper scripts, and deployable binaries. Very large model files and machine-specific secrets stay outside version control.
