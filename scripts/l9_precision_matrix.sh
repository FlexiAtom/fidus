#!/usr/bin/env bash
# 实测 L9 Crosshair 在分数缩放 / 输出旋转下的精度（Niri 专用）。
#
# 目的：回答「L9 在什么场景下不够用」——这是判断 L10 类精修方案
# 是否有存在意义的前置问题（见 docs/proposals/P3-L10-gradient-field.md）。
#
# 安全：无论如何退出（含 Ctrl-C）都恢复 scale=1 / transform=normal。
set -u
cd "$(dirname "$0")/.."

restore() {
  echo ">>> 恢复输出配置 scale=1 transform=normal"
  niri msg output eDP-1 scale 1 >/dev/null 2>&1
  niri msg output eDP-1 transform normal >/dev/null 2>&1
  sleep 1
}
trap restore EXIT INT TERM

run_once() {
  timeout 90 cargo run --release -q -p fidus --bin fidus-live-calibrate 2>&1 \
    | grep -E "calibrated in|quality:|map:|ERROR|rror|failed" | head -5
}

echo "############ 基线 scale=1 transform=normal ############"
niri msg output eDP-1 scale 1 >/dev/null 2>&1
niri msg output eDP-1 transform normal >/dev/null 2>&1
sleep 1
run_once

for s in 1.25 1.5 1.75 2; do
  echo
  echo "############ scale=$s ############"
  niri msg output eDP-1 scale "$s" >/dev/null 2>&1
  sleep 1.5
  niri msg outputs 2>/dev/null | grep -E "Logical size|Scale"
  run_once
done

niri msg output eDP-1 scale 1 >/dev/null 2>&1
sleep 1

for t in 90 180 270; do
  echo
  echo "############ transform=$t ############"
  niri msg output eDP-1 transform "$t" >/dev/null 2>&1
  sleep 1.5
  niri msg outputs 2>/dev/null | grep -E "Logical size|Transform"
  run_once
done

echo
echo "############ 组合：scale=1.5 + transform=90 ############"
niri msg output eDP-1 scale 1.5 >/dev/null 2>&1
niri msg output eDP-1 transform 90 >/dev/null 2>&1
sleep 1.5
niri msg outputs 2>/dev/null | grep -E "Logical size|Scale|Transform"
run_once
