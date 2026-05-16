from __future__ import annotations

import asyncio
import os
import platform
import re
import signal
from dataclasses import dataclass
from pathlib import Path
from typing import Iterable

from fastapi import FastAPI, Query
from fastapi.responses import JSONResponse
from pydantic import BaseModel, Field

MODEL_FILENAME = "SmolLM2-135M-Instruct.Q4_K_M.gguf"
APP_DIR = Path(__file__).resolve().parent
PROJECT_ROOT = APP_DIR.parent

BINARY_CANDIDATES = (
    PROJECT_ROOT / "ec2_bin" / "llama-cli",
    PROJECT_ROOT / "ec2-build-rpi-bin" / "llama-cli",
    PROJECT_ROOT / "llama.cpp-bin" / "llama-cli",
    PROJECT_ROOT / "llama.cpp" / "build" / "bin" / "llama-cli",
    PROJECT_ROOT / "bin" / "llama-cli",
    APP_DIR / "llama-cli",
    Path.home() / "projects" / "llama.cpp" / "build" / "bin" / "llama-cli",
    Path.home() / "ec2_bin" / "llama-cli",
    Path.home() / "ec2-build-rpi-bin" / "llama-cli",
    Path.home() / "llama.cpp-bin" / "llama-cli",
    Path.home() / "llama.cpp" / "build" / "bin" / "llama-cli",
)

MODEL_CANDIDATES = (
    PROJECT_ROOT / "ec2_bin" / MODEL_FILENAME,
    PROJECT_ROOT / "ec2-build-rpi-bin" / MODEL_FILENAME,
    PROJECT_ROOT / "llama.cpp-bin" / MODEL_FILENAME,
    PROJECT_ROOT / MODEL_FILENAME,
    PROJECT_ROOT / "models" / MODEL_FILENAME,
    PROJECT_ROOT / "llama-run" / "models" / MODEL_FILENAME,
    APP_DIR / MODEL_FILENAME,
    Path.home() / "models" / MODEL_FILENAME,
    Path.home() / "projects" / "llama.cpp" / "models" / MODEL_FILENAME,
    Path.home() / "projects" / "llama.cpp" / MODEL_FILENAME,
    Path.home() / "ec2_bin" / MODEL_FILENAME,
    Path.home() / "ec2-build-rpi-bin" / MODEL_FILENAME,
    Path.home() / "llama.cpp-bin" / MODEL_FILENAME,
    Path.home() / "llama-run" / "models" / MODEL_FILENAME,
)

ANSI_ESCAPE_RE = re.compile(r"\x1B(?:[@-Z\\-_]|\[[0-?]*[ -/]*[@-~])")
SEGFAULT_RETURN_CODES = {-signal.SIGSEGV, 139}


class GenerateRequest(BaseModel):
    prompt: str = Field(min_length=1)
    max_tokens: int = Field(default=64, ge=1)
    temperature: float = Field(default=0.7, ge=0.0)


@dataclass(slots=True)
class CommandResult:
    args: list[str]
    returncode: int
    stdout: str
    stderr: str


app = FastAPI(title="raspi-llama-api")
request_semaphore = asyncio.Semaphore(1)
request_semaphore_limit = 1


def first_existing_path(
    env_name: str,
    candidates: Iterable[Path],
    *,
    executable: bool = False,
) -> Path:
    explicit = os.environ.get(env_name)
    if explicit:
        return Path(explicit).expanduser()

    for candidate in candidates:
        resolved = candidate.expanduser()
        if resolved.exists() and (not executable or os.access(resolved, os.X_OK)):
            return resolved

    return next(iter(candidates)).expanduser()


def parse_int(value: object, default: int, *, minimum: int | None = None) -> int:
    try:
        parsed = int(value)
    except (TypeError, ValueError):
        parsed = default

    if minimum is not None:
        parsed = max(minimum, parsed)

    return parsed


def parse_float(value: object, default: float, *, minimum: float | None = None) -> float:
    try:
        parsed = float(value)
    except (TypeError, ValueError):
        parsed = default

    if minimum is not None:
        parsed = max(minimum, parsed)

    return parsed


def runtime_config() -> dict[str, str]:
    binary = first_existing_path("LLAMA_BINARY", BINARY_CANDIDATES, executable=True)
    model = first_existing_path("LLAMA_MODEL", MODEL_CANDIDATES)

    return {
        "binary": str(binary),
        "binary_dir": str(binary.parent),
        "model": str(model),
        "project_root": str(PROJECT_ROOT),
        "host": os.environ.get("LLAMA_HOST", os.environ.get("FLASK_HOST", "0.0.0.0")),
        "port": os.environ.get("LLAMA_PORT", os.environ.get("FLASK_PORT", "8080")),
        "threads": str(parse_int(os.environ.get("LLAMA_THREADS"), min(os.cpu_count() or 1, 4), minimum=1)),
        "timeout": str(parse_int(os.environ.get("LLAMA_TIMEOUT"), 180, minimum=1)),
        "context_size": str(parse_int(os.environ.get("LLAMA_CONTEXT_SIZE"), 128, minimum=32)),
        "max_concurrency": str(parse_int(os.environ.get("LLAMA_MAX_CONCURRENCY"), 1, minimum=1)),
        "test_prompt": os.environ.get("LLAMA_TEST_PROMPT", "Reply with exactly: inference test ok"),
    }


def build_runtime_env(binary: Path) -> dict[str, str]:
    env = os.environ.copy()
    lib_dir = str(binary.parent)
    library_keys = ("LD_LIBRARY_PATH", "DYLD_LIBRARY_PATH")

    for key in library_keys:
        current = env.get(key, "")
        env[key] = lib_dir if not current else f"{lib_dir}{os.pathsep}{current}"

    return env


def clean_cli_text(text: str) -> str:
    stripped = ANSI_ESCAPE_RE.sub("", text)
    return stripped.replace("\r\n", "\n").replace("\r", "\n")


def extract_generation(stdout: str, prompt: str) -> str:
    lines = [line.strip() for line in clean_cli_text(stdout).splitlines()]
    output_lines: list[str] = []

    skip_prefixes = (
        "Loading model...",
        "build      :",
        "model      :",
        "modalities :",
        "available commands:",
        "/exit",
        "/regen",
        "/clear",
        "/read ",
        "/glob ",
        "[ Prompt:",
        "Exiting...",
    )

    for line in lines:
        if not line:
            continue
        if line.startswith(">") and line[1:].strip() == prompt.strip():
            continue
        if any(line.startswith(prefix) for prefix in skip_prefixes):
            continue
        if set(line) <= {"▄", "█", "▀", " "}:
            continue
        output_lines.append(line)

    return "\n".join(output_lines).strip()


def ensure_runtime_semaphore() -> None:
    global request_semaphore
    global request_semaphore_limit
    cfg = runtime_config()
    max_concurrency = parse_int(cfg["max_concurrency"], 1, minimum=1)
    if request_semaphore_limit != max_concurrency:
        request_semaphore = asyncio.Semaphore(max_concurrency)
        request_semaphore_limit = max_concurrency


async def run_command(
    cmd: list[str],
    *,
    binary: Path,
    timeout: int,
) -> CommandResult:
    process = await asyncio.create_subprocess_exec(
        *cmd,
        stdout=asyncio.subprocess.PIPE,
        stderr=asyncio.subprocess.PIPE,
        cwd=str(binary.parent),
        env=build_runtime_env(binary),
    )

    try:
        stdout_bytes, stderr_bytes = await asyncio.wait_for(process.communicate(), timeout=timeout)
    except TimeoutError as exc:
        process.kill()
        await process.wait()
        raise asyncio.TimeoutError from exc

    return CommandResult(
        args=cmd,
        returncode=process.returncode,
        stdout=stdout_bytes.decode("utf-8", errors="replace"),
        stderr=stderr_bytes.decode("utf-8", errors="replace"),
    )


def build_inference_command(
    prompt: str,
    *,
    max_tokens: int,
    temperature: float,
    cfg: dict[str, str],
) -> list[str]:
    return [
        cfg["binary"],
        "-m",
        cfg["model"],
        "-p",
        prompt,
        "-n",
        str(max_tokens),
        "-t",
        cfg["threads"],
        "-c",
        cfg["context_size"],
        "--temp",
        str(temperature),
        "--simple-io",
        "--no-display-prompt",
        "--single-turn",
        "--log-disable",
        "--no-warmup",
    ]


async def run_inference(
    prompt: str,
    *,
    max_tokens: int,
    temperature: float,
    cfg: dict[str, str],
) -> CommandResult:
    binary = Path(cfg["binary"])
    cmd = build_inference_command(
        prompt,
        max_tokens=max_tokens,
        temperature=temperature,
        cfg=cfg,
    )
    return await run_command(cmd, binary=binary, timeout=parse_int(cfg["timeout"], 180, minimum=1))


async def check_runtime() -> tuple[list[str], dict[str, str]]:
    cfg = runtime_config()
    ensure_runtime_semaphore()
    binary = Path(cfg["binary"])
    model = Path(cfg["model"])
    errors: list[str] = []

    if not binary.exists():
        errors.append(f"llama binary not found: {binary}")
    elif not os.access(binary, os.X_OK):
        errors.append(f"llama binary is not executable: {binary}")

    if not model.exists():
        errors.append(f"model not found: {model}")

    if not errors:
        try:
            probe = await run_command([str(binary), "--help"], binary=binary, timeout=10)
            if probe.returncode != 0:
                errors.append(
                    f"llama binary probe failed with code {probe.returncode}: "
                    f"{(probe.stderr or probe.stdout).strip()}"
                )
        except OSError as exc:
            errors.append(f"llama binary probe failed: {exc}")
        except asyncio.TimeoutError:
            errors.append("llama binary probe timed out")

    return errors, cfg


def build_response_payload(
    *,
    mode: str,
    prompt: str,
    cfg: dict[str, str],
    result: CommandResult,
    warning: str | None = None,
) -> dict[str, object]:
    return {
        "ok": True,
        "mode": mode,
        "prompt": prompt,
        "output": extract_generation(result.stdout, prompt),
        "stdout": clean_cli_text(result.stdout).strip(),
        "stderr": clean_cli_text(result.stderr).strip(),
        "binary": cfg["binary"],
        "model": cfg["model"],
        "returncode": result.returncode,
        "warning": warning,
    }


async def inference_response(
    prompt: str,
    *,
    max_tokens: int,
    temperature: float,
    mode: str,
) -> dict[str, object]:
    errors, cfg = await check_runtime()
    if errors:
        return {
            "ok": False,
            "error": "runtime check failed",
            "details": errors,
            "binary": cfg["binary"],
            "model": cfg["model"],
            "status_code": 500,
        }

    try:
        async with request_semaphore:
            result = await run_inference(
                prompt,
                max_tokens=max_tokens,
                temperature=temperature,
                cfg=cfg,
            )
    except OSError as exc:
        return {
            "ok": False,
            "error": str(exc),
            "binary": cfg["binary"],
            "model": cfg["model"],
            "status_code": 500,
        }
    except asyncio.TimeoutError as exc:
        return {
            "ok": False,
            "error": "llama-cli timed out",
            "binary": cfg["binary"],
            "model": cfg["model"],
            "status_code": 504,
        }

    output = extract_generation(result.stdout, prompt)
    if result.returncode == 0:
        return build_response_payload(mode=mode, prompt=prompt, cfg=cfg, result=result)

    if output and result.returncode in SEGFAULT_RETURN_CODES:
        return build_response_payload(
            mode=mode,
            prompt=prompt,
            cfg=cfg,
            result=result,
            warning="llama-cli terminated after producing output; returning the generated text",
        )

    return {
        "ok": False,
        "error": "llama-cli failed",
        "mode": mode,
        "returncode": result.returncode,
        "stdout": clean_cli_text(result.stdout).strip(),
        "stderr": clean_cli_text(result.stderr).strip(),
        "binary": cfg["binary"],
        "model": cfg["model"],
        "status_code": 500,
    }


@app.get("/health")
async def health() -> JSONResponse:
    errors, cfg = await check_runtime()
    binary = Path(cfg["binary"])
    model = Path(cfg["model"])
    payload = {
        "ok": not errors,
        "binary": cfg["binary"],
        "binary_dir": cfg["binary_dir"],
        "model": cfg["model"],
        "project_root": cfg["project_root"],
        "machine": platform.machine(),
        "binary_exists": binary.exists(),
        "binary_executable": os.access(binary, os.X_OK),
        "model_exists": model.exists(),
        "max_concurrency": parse_int(cfg["max_concurrency"], 1, minimum=1),
        "errors": errors,
    }
    return JSONResponse(payload, status_code=200 if not errors else 500)


@app.post("/generate")
async def generate(payload: GenerateRequest) -> JSONResponse:
    prompt = payload.prompt.strip()
    if not prompt:
        return JSONResponse({"ok": False, "error": "prompt is required"}, status_code=400)

    response = await inference_response(
        prompt,
        max_tokens=payload.max_tokens,
        temperature=payload.temperature,
        mode="generate",
    )
    return JSONResponse(response, status_code=parse_int(response.pop("status_code", 200), 200, minimum=100))


@app.get("/test-inference")
async def test_inference(
    prompt: str | None = Query(default=None),
    max_tokens: int = Query(default=16, ge=1),
    temperature: float = Query(default=0.2, ge=0.0),
) -> JSONResponse:
    cfg = runtime_config()
    resolved_prompt = (prompt or cfg["test_prompt"]).strip()

    response = await inference_response(
        resolved_prompt,
        max_tokens=max_tokens,
        temperature=temperature,
        mode="test-inference",
    )
    return JSONResponse(response, status_code=parse_int(response.pop("status_code", 200), 200, minimum=100))


if __name__ == "__main__":
    import uvicorn

    cfg = runtime_config()
    uvicorn.run(app, host=cfg["host"], port=parse_int(cfg["port"], 8080, minimum=1))
