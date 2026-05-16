# Raspberry Pi Zero 2 WH LLM Setup

## Goal

Set up a brand new Raspberry Pi Zero 2 WH for:

- lightweight LLM experiments
- remote SSH development
- small Python agent services
- optional tiny local `llama.cpp` inference

This device is best used as a lightweight remote client or controller. It can run very small local models experimentally, but it is not a strong local inference machine.

## Recommended OS

Use:

- `Raspberry Pi OS Lite`
- prefer `64-bit` if available for your image choice

Why:

- headless setup is simpler
- less RAM is wasted on desktop UI
- better fit for SSH, Python, and CLI tooling

## Before first boot

In Raspberry Pi Imager:

1. Choose `Raspberry Pi OS Lite`.
2. Set hostname.
3. Create the user account.
4. Enable `SSH`.
5. Set Wi-Fi to your `2.4G` secured network.
6. Write the image to the SD card.

## First boot

1. Insert the SD card into the Pi.
2. Power on the Pi.
3. Wait about `1-2 minutes`.
4. SSH into the Pi.

Example:

```bash
ssh bitlabs@100.64.1.42
```

or if your SSH config alias exists:

```bash
ssh raspi
```

## Step 1: Update the OS

Run:

```bash
sudo apt update
sudo apt full-upgrade -y
sudo apt clean
sudo reboot
```

After reboot, reconnect by SSH.

## Step 2: Install base tools

Run:

```bash
sudo apt install -y \
  git curl wget rsync jq tmux htop \
  build-essential cmake ninja-build pkg-config \
  python3 python3-venv python3-pip
```

These packages give you:

- basic development tools
- Python environment support
- terminal utilities
- build tools for `llama.cpp`

## Step 3: Verify the device state

Run:

```bash
uname -a
hostname
hostname -I
free -h
df -h /
```

Useful checks:

- confirm the correct hostname
- confirm the current IP
- confirm free memory
- confirm enough SD card space

## Step 4: Optional SSH quality-of-life setup

On your Mac, use a fixed SSH alias:

```text
Host raspi
   HostName 100.64.1.42
   User bitlabs
```

Then connect with:

```bash
ssh raspi
```

Also recommended:

- reserve the Pi IP in your router DHCP settings
- use SSH keys later instead of password login

## Step 5: Python setup for agent or API-based LLM usage

Create a working directory:

```bash
mkdir -p ~/projects
cd ~/projects
python3 -m venv .venv
source .venv/bin/activate
python -m pip install --upgrade pip
```

Install common packages as needed:

```bash
pip install openai requests fastapi uvicorn
```

This Pi is a good fit for:

- calling remote LLM APIs
- running lightweight automation
- small local services
- SSH-based remote control

## Step 6: Optional local inference with llama.cpp

This is only for very small experimental models.

Clone and build:

```bash
cd ~/projects
git clone https://github.com/ggml-org/llama.cpp
cd llama.cpp
cmake -B build
cmake --build build --config Release
```

After build, check the binaries:

```bash
ls -la build/bin
```

## Step 7: Model guidance for this Pi

Be realistic about model size.

Recommended:

- very small GGUF models only
- low-bit quantized models
- tiny test prompts

Not recommended:

- large chat models
- multi-billion parameter models as normal workflow
- anything expecting fast response times

Best practical use:

- use the Pi as a client to a stronger machine or cloud model
- use local inference only for learning and experiments

## Step 8: Useful runtime tools

Install or use:

```bash
sudo apt install -y tmux htop
```

Examples:

Start a persistent session:

```bash
tmux
```

Monitor system load:

```bash
htop
```

Check current users:

```bash
who
```

Check running SSH-related processes:

```bash
ps aux | grep ssh
```

## Step 9: Recommended next improvements

After the base setup works, do these:

1. add an SSH public key
2. disable password login later if desired
3. reserve a fixed IP in the router
4. keep the OS updated regularly
5. keep the project work inside a Python virtual environment

## Minimal setup checklist

Use this order:

1. Flash `Raspberry Pi OS Lite`
2. Enable `SSH`
3. Configure `2.4G` Wi-Fi
4. Boot and SSH in
5. Run `apt update` and `apt full-upgrade`
6. Install base packages
7. Set up Python venv
8. Decide whether the Pi will:
   - call remote LLM APIs
   - run tiny local `llama.cpp` experiments

## Copy-paste quick start

```bash
sudo apt update
sudo apt full-upgrade -y
sudo apt clean
sudo apt install -y \
  git curl wget rsync jq tmux htop \
  build-essential cmake ninja-build pkg-config \
  python3 python3-venv python3-pip

mkdir -p ~/projects
cd ~/projects
python3 -m venv .venv
source .venv/bin/activate
python -m pip install --upgrade pip
pip install openai requests fastapi uvicorn
```

## Optional llama.cpp quick start

```bash
cd ~/projects
git clone https://github.com/ggml-org/llama.cpp
cd llama.cpp
cmake -B build
cmake --build build --config Release
ls -la build/bin
```

## Notes

- This Pi can support multiple SSH sessions, but performance will drop under load.
- VS Code Remote-SSH may be slower than plain terminal SSH on first connect.
- `raspiidb.local` may be less reliable than direct IP on this Mac.
- Prefer `ssh raspi` or `ssh bitlabs@100.64.1.42` if the alias is configured.
