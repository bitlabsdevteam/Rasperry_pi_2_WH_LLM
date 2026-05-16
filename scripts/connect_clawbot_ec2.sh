#!/bin/zsh

set -euo pipefail

HOST="ec2-3-112-109-215.ap-northeast-1.compute.amazonaws.com"
KEY="/Users/davidbong/Documents/Rasperry_2WH/my_clawbot_key.pem"
KNOWN_HOSTS="/private/tmp/clawbot_ec2_known_hosts"

if [[ ! -f "$KEY" ]]; then
  echo "SSH key not found: $KEY" >&2
  exit 1
fi

if [[ $# -gt 0 ]]; then
  USERNAME="$1"
else
  USERNAME="ubuntu"
fi

exec ssh \
  -i "$KEY" \
  -o IdentitiesOnly=yes \
  -o StrictHostKeyChecking=accept-new \
  -o UserKnownHostsFile="$KNOWN_HOSTS" \
  "${USERNAME}@${HOST}"
