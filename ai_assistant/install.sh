#!/usr/bin/env sh
set -eu

ROOT_DIR="$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)"
cd "$ROOT_DIR"

mkdir -p data/conversations data/notes data/memory data/tasks data/summaries data/profiles

for example in llm memory scheduler identity tools telegram; do
  if [ ! -f "config/${example}.json" ]; then
    cp "config/${example}.example.json" "config/${example}.json"
  fi
done

cargo build --release

cat <<'EOF'
Build complete.

Next steps:
1. Run ./target/release/ai_assistant with no arguments to launch Telegram onboarding
2. Or run ./target/release/ai_assistant onboard telegram explicitly
3. After onboarding, run ./target/release/ai_assistant doctor and then ./target/release/ai_assistant serve
EOF
