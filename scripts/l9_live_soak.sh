#!/usr/bin/env bash
# L9 实机耐受测试 —— 按 AGENTS §11：假设人正在用这台电脑。
#
# 协议（前两条是踩过坑才定下的）：
#   1. 所有输出重定向到文件，跑完再读。脚本自己的打印也是屏幕内容，
#      在屏幕上滚动会污染被测量的画面。
#   2. 交错 A/B 而非先跑完一组再跑另一组，消除时间漂移
#      （同一条件在安静时段与繁忙时段的失败率可以差出 7/15）。
#   3. 同时记录背景噪声底，否则无法区分"修好了"与"这次桌面恰好安静"。
#
# 用法：bash scripts/l9_live_soak.sh [每档次数]
set -u
cd "$(dirname "$0")/.."
N="${1:-15}"
OUT=".soak"
rm -rf "$OUT"; mkdir -p "$OUT"

restore() {
  niri msg output eDP-1 scale 1 >/dev/null 2>&1
  niri msg output eDP-1 transform normal >/dev/null 2>&1
}
trap restore EXIT INT TERM

# 背景噪声底：不投任何标记，单进程内连采 30 帧。
# 单进程很关键 —— 每次新起进程会让终端滚动，测到的是自己的输出。
if [ -x ./target/release/fidus-probe-marker ]; then
  FIDUS_PROBE_BURST=30 timeout 120 ./target/release/fidus-probe-marker \
    > "$OUT/noise_floor.txt" 2>&1
fi

run_one() { # scale, index
  niri msg output eDP-1 scale "$1" >/dev/null 2>&1
  sleep 1.2
  timeout 90 ./target/release/fidus-calibrate > "$OUT/s$1_$2.log" 2>&1
}

for i in $(seq "$N"); do
  run_one 1 "$i"
  run_one 1.25 "$i"
done
restore

{
  echo "=== L9 实机耐受测试 (每档 $N 次) ==="
  echo
  if [ -f "$OUT/noise_floor.txt" ]; then
    dirty=$(grep -c "pair:" "$OUT/noise_floor.txt" 2>/dev/null || echo 0)
    quiet=$(grep -c "0 changed, 0 above" "$OUT/noise_floor.txt" 2>/dev/null || echo 0)
    echo "背景噪声底: $((dirty - quiet))/$dirty 对相邻帧发生变化"
    echo "  （全为 0 说明测试期间桌面是静止的，结论的说服力相应打折）"
    echo
  fi
  for s in 1 1.25; do
    f=0
    for i in $(seq "$N"); do
      grep -qE "calibration failed|below threshold" "$OUT/s${s}_$i.log" && f=$((f+1))
    done
    echo "scale=$s : 失败 $f/$N"
  done
  echo
  echo "--- 失败原因分布（空 = 无失败）---"
  cat "$OUT"/s*.log 2>/dev/null \
    | grep -oE "(last detector error: .*|below threshold: [0-9.]+)" | sort | uniq -c
  echo
  echo "--- 质量分布 ---"
  for s in 1 1.25; do
    echo "scale=$s:"
    cat "$OUT"/s${s}_*.log 2>/dev/null \
      | grep -oE "rms [0-9.]+px, max [0-9.]+px, verify [0-9.]+px, consistency [0-9.]+px" \
      | sed 's/^/  /'
  done
} > "$OUT/report.txt" 2>&1

cat "$OUT/report.txt"
