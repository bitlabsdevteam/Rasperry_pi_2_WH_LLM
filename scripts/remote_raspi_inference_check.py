from __future__ import annotations

import asyncio
import json
import os
import sys
from pathlib import Path


def main() -> None:
    project_root = Path(__file__).resolve().parents[1]
    api_dir = project_root / "raspi_llama_api"
    os.chdir(api_dir)
    sys.path.insert(0, str(api_dir))

    from fastapi.testclient import TestClient

    from app import app, check_runtime

    errors, cfg = asyncio.run(check_runtime())
    print("=== runtime ===")
    print(
        json.dumps(
            {
                "errors": errors,
                "binary": cfg["binary"],
                "model": cfg["model"],
                "threads": cfg["threads"],
                "timeout": cfg["timeout"],
                "project_root": cfg["project_root"],
                "binary_exists": Path(cfg["binary"]).exists(),
                "model_exists": Path(cfg["model"]).exists(),
            },
            indent=2,
        )
    )

    print("=== test-inference ===")
    with TestClient(app) as client:
        response = client.get(
            "/test-inference",
            params={
                "prompt": "Reply with exactly: inference test ok",
                "max_tokens": 8,
                "temperature": 0.2,
            },
        )
        print(f"status={response.status_code}")
        print(response.text)


if __name__ == "__main__":
    main()
