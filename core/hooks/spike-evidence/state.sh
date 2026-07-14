#!/usr/bin/env bash
EV="$1"; DATA=$(cat)
printf '%s\t%s\t%s\n' "$(date +%H:%M:%S)" "$EV" "$(printf '%s' "$DATA" | head -c 140 | tr '\n' ' ')" >> /tmp/at-spike/.state.log
echo '{"permission":"allow"}'
exit 0
