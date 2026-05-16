Setup Guide: Running an LLM on Raspberry Pi Zero 2 W / WH using llama.cpp

Overview

This guide explains how to:

1. Prepare a Raspberry Pi Zero 2 W / WH
2. Cross-compile llama.cpp ARM64 binaries on EC2
3. Transfer the binaries to Raspberry Pi
4. Run a GGUF LLM model locally
5. Test inference successfully

This setup is optimized for:

* Raspberry Pi Zero 2 W / WH
* ARM64 Raspberry Pi OS
* Small quantized GGUF models
* CPU inference only

Recommended models:

* SmolLM2-135M
* SmolLM-360M
* TinyLlama 1.1B (slow)
* Qwen2 0.5B

Recommended quantization:

* Q4_K_M
* Q5_K_M

Avoid:

* Q8 models
* 7B+ models
* GPU backends

⸻

Step 1 — Prepare Raspberry Pi

Update the Raspberry Pi:

sudo apt update && sudo apt upgrade -y

Install required packages:

sudo apt install -y git wget curl build-essential cmake

Verify Raspberry Pi architecture:

uname -m

Expected output:

aarch64

If you see:

armv7l

then your OS is 32-bit.

ARM64 is recommended.

⸻

Step 2 — Prepare EC2 Build Machine

Use Ubuntu EC2.

Install ARM cross-compilers:

sudo apt update && sudo apt install -y \
gcc-aarch64-linux-gnu \
g++-aarch64-linux-gnu \
cmake \
make \
git

Clone llama.cpp:

git clone https://github.com/ggml-org/llama.cpp.git

Enter project:

cd llama.cpp

⸻

Step 3 — Build Static ARM64 Binary

Delete previous builds:

rm -rf build-rpi

Create static ARM64 build:

cmake -B build-rpi \
  -DCMAKE_SYSTEM_NAME=Linux \
  -DCMAKE_SYSTEM_PROCESSOR=aarch64 \
  -DCMAKE_C_COMPILER=aarch64-linux-gnu-gcc \
  -DCMAKE_CXX_COMPILER=aarch64-linux-gnu-g++ \
  -DCMAKE_BUILD_TYPE=Release \
  -DBUILD_SHARED_LIBS=OFF \
  -DCMAKE_EXE_LINKER_FLAGS="-static" \
  -DGGML_NATIVE=OFF \
  -DGGML_OPENMP=OFF

Compile:

cmake --build build-rpi -j8

⸻

Step 4 — Verify ARM Binary

Check architecture:

file ~/llama.cpp/build-rpi/bin/llama-cli

Expected:

ELF 64-bit LSB executable, ARM aarch64, statically linked

Verify static linking:

ldd ~/llama.cpp/build-rpi/bin/llama-cli

Expected:

not a dynamic executable

This is important because Raspberry Pi Bookworm uses GLIBC 2.41.

Static linking avoids GLIBC mismatch issues.

⸻

Step 5 — Download GGUF Model on EC2

Create model folder:

mkdir -p ~/models

Download SmolLM2 model:

wget -O ~/models/SmolLM2-135M-Instruct.Q4_K_M.gguf \
https://huggingface.co/unsloth/SmolLM2-135M-Instruct-GGUF/resolve/main/SmolLM2-135M-Instruct.Q4_K_M.gguf

Verify model:

ls -lh ~/models

⸻

Step 6 — Copy Binary and Model to Raspberry Pi

Copy llama-cli:

scp ~/llama.cpp/build-rpi/bin/llama-cli \
pi@<RASPBERRY_PI_IP>:~/

Copy model:

scp ~/models/SmolLM2-135M-Instruct.Q4_K_M.gguf \
pi@<RASPBERRY_PI_IP>:~/

⸻

Step 7 — SSH into Raspberry Pi

ssh pi@<RASPBERRY_PI_IP>

Make binary executable:

chmod +x ~/llama-cli

⸻

Step 8 — Run LLM Inference

Run inference:

~/llama-cli \
  -m ~/SmolLM2-135M-Instruct.Q4_K_M.gguf \
  -p "Explain Artificial Intelligence in one sentence." \
  -n 32 \
  -t 4 \
  -c 128

Recommended parameters:

Parameter	Description
-t 4	Use 4 CPU threads
-c 128	Small context to reduce RAM
-n 32	Generate 32 tokens

⸻

Step 9 — Benchmark Performance

Run benchmark:

~/llama-cli \
  -m ~/SmolLM2-135M-Instruct.Q4_K_M.gguf \
  -p "Hello" \
  -n 16

Expected speed on Raspberry Pi Zero 2 W:

* 1–4 tokens/sec depending on model size

⸻

Step 10 — Troubleshooting

Error: Exec format error

Cause:

* Running ARM binary on x86 machine

Fix:

* Run binary only on Raspberry Pi

⸻

Error: GLIBC mismatch

Cause:

* Dynamic linking mismatch

Fix:

* Build static binary

⸻

Error: cannot allocate memory

Fix:

Reduce context size:

-c 64

Use smaller model.

⸻

Error: Failed to load model

Verify model exists:

ls -lh ~/SmolLM2-135M-Instruct.Q4_K_M.gguf

⸻

Recommended Lightweight Models

Model	Recommended
SmolLM2-135M	Excellent
SmolLM-360M	Excellent
Qwen2 0.5B	Good
TinyLlama 1.1B	Slow

⸻

Final Notes

Raspberry Pi Zero 2 W / WH is resource constrained:

* 512MB RAM
* ARM Cortex-A53
* CPU-only inference

Best practices:

* Use Q4 quantization
* Use small context windows
* Keep models under 500M parameters
* Use static ARM64 binaries

This setup provides a lightweight local AI inference environment suitable for:

* Edge AI
* Embedded AI
* Offline AI
* Robotics
* IoT AI systems
* Local assistants