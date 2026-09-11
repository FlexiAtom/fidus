#!/usr/bin/env bash
# 统计 L9 在各 scale 下的质量分布（每档 N 次），量化分数缩放的退化。
# 安全：退出时恢复 scale=1 / transform=normal。
set -u
cd "$(dirname "$0")/.."
N="${1:-6}"

restore() {
  niri msg output eDP-1 scale 1 >/dev/null 2>&1
  niri msg output eDP-1 transform normal >/dev/null 2>&1
}
trap restore EXIT INT TERM

for s in 1 1.25 1.5 1.75 2; do
  niri msg output eDP-1 scale "$s" >/dev/null 2>&1
  sleep 1.5
  fail=0
  echo "=== scale=$s ==="
  for i in $(seq "$N"); do
    out=$(timeout 90 cargo run --release -q -p fidus --bin fidus-calibrate 2>&1 \
          | grep -E "quality:|calibration failed" | head -1)
    if echo "$out" | grep -q "failed"; then
      fail=$((fail+1)); echo "  #$i FAILED"
    else
      echo "$out" | sed -E 's/.*quality: /  #'"$i"' /; s/, samples.*//'
    fi
  done
  echo "  -> 失败 $fail/$N"
done
