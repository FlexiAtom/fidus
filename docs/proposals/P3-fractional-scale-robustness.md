# 提案 · P3 L9 在真实桌面下的稳健性

> **状态：✅ 已落实（2026-09）** — 已提升为方案，写入 [spec §4.1「检测的三重判据」](../spec.md)。
> 裁决记录：§4.2 的架构问题人工选定 **C（多帧互证）**，不给后端加第三个原语。
> 实机 A/B（屏幕有真实活动，噪声底 9/29）：**修复前 12/12 失败 → 修复后 0/30**。
> 日期：2026-09 · 里程碑：P3
> 实测依据：[`docs/measurements/l9-fractional-scaling.md`](../measurements/l9-fractional-scaling.md)

---

## 0. TL;DR

**问题**：在**有正常窗口活动的真实桌面**上，L9 校准失败率高达 **4/12 ~ 7/15**。

**这与分数缩放无关**——scale=1 下同样失败。我最初把它归因于分数缩放，**那是错的**（§1 记录了证伪经过，因为错误归因本身就是要避免的坑）。

**真正的成因**：差分检测隐含"标记是两帧之间唯一的变化"这一前提。真实桌面上**别的窗口会自发重绘**（实测：29 次连续截屏中有 1 次整屏变化 144386 px）。单次校准要采约 28 帧，因此**至少撞上一次的概率约 63%**——与实测失败率吻合。

**核心洞察**：**L9 明明知道自己投的标记是什么颜色，却没用这个信息。** `detect_colored_change` 早已存在且 L0 在用，L9 却调用了不带颜色的 `detect_single_change`。

**但固定颜色不够**——实测发现默认品红会与桌面内容撞色（我的终端配色里就有品红文字，与标记色的距离仅 **33**）。因此方案是**自适应选色**：从本来就要采的基线帧里，选当前屏幕上最罕见的颜色。

| 选色方式 | 与背景的最小色距 |
|---|---|
| 固定品红 `(255,0,255)` | **33 ~ 52**（踩线） |
| **自适应选色** | **117 ~ 128** |

**余量提高约 3 倍，且成本为零**（基线帧本来就要采）。

---

## 1. 先记录我自己的错误归因（这是提案的一部分）

> AGENTS §10 要求提案验算核心假设。**我的第一版提案核心假设是错的，这里完整记录，因为"怎么发现自己错了"比结论更值得留存。**

### 1.1 错误链条

初测发现"分数缩放下 rms 0.308px + 偶发失败"，我据此推断了三个成因，**全部证伪**：

| 我的推断 | 证伪方式 | 结果 |
|---|---|---|
| 面积先验在分数缩放下漂移 | 算物理边长 `round((L+28)s)−round(Ls)` | 面积比仅 **1.12×**，容差 3.0×，不可能 |
| 边缘插值糊掉标记 | `fidus-probe-marker` 实测 | fill ratio **1.000**，满分 |
| 分数缩放使背景重采样抖动 | 同进程连采 8 帧 | **逐像素完全一致**，任何 scale 都一样 |

第三条尤其值得记：我曾测到"scale=1.25 下背景差分 30361 像素"，并把它写进了测量文档当作核心证据。**那个读数是我自己的终端输出**——每次 probe 都是新进程，启动时终端在滚动。同进程内连续采样后，差分恒为 0。

### 1.2 决定性的对照实验

真正定位成因的是这个——**不投任何标记**，单进程内连采 30 帧：

```
共 29 对相邻帧，其中 28 对完全一致，1 对变化 144386 px（141140 超阈值）
```

**桌面自己在动**，与 fidus 无关，与 scale 无关。坏帧的变化区起点固定在 `(18,40)`，正是一个 kitty 窗口的左上角。

### 1.3 教训

**分数缩放确实有系统性残差（0.2px，成因是取整），但那不是失败的原因。** 我把两个同时观察到的现象因果绑定了——而实际上一个无害、一个致命。

> 这正是 AGENTS §8 第一问的变体：**"我用来判决的量，真的是我关心的那个属性吗？"** 这里是"我观察到的相关，真的是因果吗？"

---

## 2. 成因的完整证据链

### 2.1 前提是隐含的

`detect_single_change` 的语义是"找出两帧之间**唯一**的连通变化"。这要求：**标记是唯一变化的东西**。

这个前提**从未被写下来**，也从未被检验。它在以下环境成立：干净的测试桌面、仿真测试（`FakeIo` 的背景完全静止）。它在**真实桌面上不成立**。

### 2.2 概率推算与实测吻合

单次校准的采帧数：2 pass × (4 主标记 + 3 验证位置) × 2 帧 = **约 28 帧**。

实测单帧间隔撞上背景变化的概率 p ≈ 1/29 ≈ 3.4%：

```
1 − (1 − 0.034)^28 = 62.6%
```

实测失败率 4/12 (33%)、5/12 (42%)、7/15 (47%)，**同一量级**。差异来自重试有时能救回来。

### 2.3 失败形态印证

```
last detector error: 68 plausible regions found; measurement ambiguous
last detector error: 26 plausible regions found; measurement ambiguous
calibration accuracy below threshold: 287.424 px > 1.000 px (residual)
```

**"68 个 plausible region"不是标记判错，是背景碎成了 68 块**，每块都通过了 fill + area 门控。

而 `287px 残差`更危险：说明**某次测量选中了背景碎块当作标记**，并被当成合法对应点喂进了最小二乘。虽然最终被残差门控拦下（原则四生效），但**它证明了错误测量能进入求解**。

### 2.4 重试为什么救不回来

```rust
// Burn a tick before retrying so a settling compositor or an
// animated background can diverge less between baseline and post.
let _ = rng.next_u64();
```

**注释声称在等待，实际 `rng.next_u64()` 耗时约 1 纳秒。** 三次重试几乎同时发生，撞上同一次背景重绘事件的概率高度相关。

> 这是 AGENTS §1 表格里同型的缺陷：**注释声称的行为与实际行为不符**。

---

## 3. 方案

### 3.1 改动一（核心）：L9 使用「差分 ∧ 颜色」判据，且颜色自适应

**L9 已经知道 `cfg.style.rgba`，只是没传给检测器。**

```rust
// crosshair.rs: measure()
match detect_colored_change(
    &baseline, &post,
    marker_rgba,           // 自适应选出，见下
    cfg.color_tolerance,   // 新增配置，默认 40
    expected_area,
    &self.detect,
) { ... }
```

**颜色不能写死**（§4.1 实测：默认品红与终端配色的距离仅 33，踩线）。改为从基线帧自适应选取：

```rust
/// Picks the marker color that is *furthest* from anything currently on
/// screen, measured on the baseline frame we already capture anyway.
///
/// Why: a fixed color can collide with desktop content — the default
/// magenta sits only 33 units away from this machine's terminal palette,
/// leaving no usable tolerance. Measured across three real screenshots,
/// adaptive selection keeps a margin of 117–128 instead.
///
/// Failure mode: if the screen genuinely contains every saturated color
/// (a color-picker or test pattern fullscreen), the best margin is small;
/// we then report EnvironmentTooColorful rather than pick a doomed color
/// and fail obscurely later.
fn pick_marker_color(baseline: &Frame) -> Option<[u8; 4]>;
```

**为什么这是对的而非权宜之计**：

- 标记颜色是 **fidus 自己选的**，不是平台报告的——**不违反零信任**；
- 差分仍然保留（静态的同色壁纸在两帧中相同，会被差分抵消），**颜色是叠加的第二判据，不是替代**；
- 规格 §4.3 已经把"纯找色"记为 L0 的 P2-d 缺陷——**这里不是纯找色，是"差分 ∧ 颜色"**，与 L0 修复后的做法一致；
- `detect_colored_change` 已存在并经 L0 验证，**不引入新代码路径**。

**失效模式**（AGENTS §1 三段式）：若屏幕上**恰好有一个同色物体同时出现**，颜色判据失效，退化回当前行为（多候选 → `Ambiguous` → 重试）。这不会产出错误坐标，只会失败。攻击者若能任意绘制同色块，本就能干扰任何视觉方案——**但 `Ambiguous` 保证了它无法伪造出一个被接受的坐标**。

### 3.2 改动二：重试要真的退避

把空转的 `rng.next_u64()` 换成真实等待，并**随机化间隔**（避免与周期性重绘——如 1Hz 光标闪烁——共振）：

```rust
/// Wait before retrying. Background redraws (cursor blink, window
/// animations) are *events in time*, so retrying immediately lands inside
/// the same event with high probability. The delay is randomized to avoid
/// phase-locking with a periodic repaint such as a 1 Hz cursor blink.
///
/// Failure mode: a delay that is too long makes calibration sluggish;
/// bounded by RETRY_DELAY_MAX. Too short and it does not decorrelate —
/// which is exactly the current bug, where the "burn a tick" comment
/// describes a wait that takes about a nanosecond.
```

**这需要 io 提供等待能力**——`CalibrationIo` 目前没有 sleep 原语。**这是本提案最需要审的地方，见 §4.2。**

### 3.3 改动三：仿真测试要模拟真实桌面

`FakeIo` 的背景完全静止，**所以这个 bug 在仿真里不存在**。加 `Noise::ForeignRedraw { probability, region }`：在随机帧注入一块与标记无关的大面积变化。

**必须先写出会失败的测试，再改实现。**

---

## 4. 自审：落实时会出什么问题

### 4.1 ✅ 颜色容差选多少？——已补测，且推翻了我的第一个取值

我最初写"默认 ±40，实测支持"。**补测后发现那是错的**，记录经过：

在三张真实截图上测"背景中有多少像素落在容差内"：

| 容差 | desktop1 | desktop2 | fail_6 |
|---|---|---|---|
| ±30 | 0 | 0 | 0 |
| **±40** | **1831** | **158** | 0 |
| ±60 | 2174 | 244 | 742 |

**±40 会在真实桌面上误匹配 1831 个像素**——那是我终端里的品红文字。再看余量：

| 截图 | 背景中最接近品红的色距 |
|---|---|
| desktop1 | **34** |
| desktop2 | **33** |
| fail_6 | 52 |

**±30 只以 3 个单位的余量通过——这是踩线，不是安全。** 任何换了配色方案的用户都可能把它压到 0。

**这正是 AGENTS §8 要防的**：`color_tolerance` 是判决用的量，若不实测就会拿一个"典型情况下对"的常数当依据。

**结论**：固定颜色本身就是错的思路，改为自适应选色（§3.1），余量从 33 提到 117–128。容差 ±40 在自适应选色下是安全的（余量是它的 3 倍）。

**仍需实现时校验**：`pick_marker_color` 必须返回实测余量，余量 < 2×tolerance 时拒绝（§3.1 的 `EnvironmentTooColorful`）。

### 4.2 🔴 `CalibrationIo` 该不该有 sleep 原语？

**这是架构问题，不是实现细节。** 规格反复强调后端只适配"投射 + 截屏"两个原语（AGENTS §9）。**加第三个原语是在动架构约束。**

三个选项：

| 选项 | 优点 | 缺点 |
|---|---|---|
| A. 加 `io.wait(Duration)` 原语 | 直接 | **破坏"只有两个原语"的契约**，每个后端都要实现 |
| B. 校准器内部用 `std::thread::sleep` | 不动契约 | 让 `fidus-calibrate` 依赖 std 时间，**仿真测试会真的睡** |
| C. 不等待，改为**多采几帧取多数** | 不动契约、不睡 | 采帧成本上升，但帧本来就要采 |

**我倾向 C**，理由：它把"等待"换成了"更多证据"，而**更多证据正是概率池的思路**。且它在仿真中完全可控（`FakeIo` 可以精确控制哪帧是坏的）。

**但 C 有个问题**：若背景变化持续数秒（视频播放），多采几帧全是坏的。此时应当**诚实失败**而非无限采。

**请裁决 A / B / C。**

### 4.3 🟡 颜色判据会不会掩盖"标记根本没投上"？

若投射失败（surface 没显示），差分为空 → `NotFound`。加了颜色判据后仍是 `NotFound`，**行为不变**。✅ 无风险。

### 4.4 🟡 `expected_area` 先验还需要吗？

加了颜色后，面积先验的价值下降。但**不应删除**——它防的是"同色但尺寸差很多"的情况（如同色壁纸大色块）。**保留，但它不再是主判据。**

> 注意 §1 已证明面积先验**不是**失败成因，所以这里不做改动，避免"顺手改一改"引入新变量。

### 4.5 🟡 分数缩放的 0.2px 残差怎么办？

**本提案不处理。** 它是真实的（成因：投射端取整），但**无害**：调用方拿 `CoordinateFrame` 摆窗口，窗口位置本身是整数逻辑像素，0.2px 不改变任何 `round()` 结果。

**先修会坏的，不修不好看的。** 已在测量文档中记录，留待将来有实际需求时再提案。

### 4.6 🟢 `fidus-probe-marker` 该不该保留？

**应该。** 它是定位本 bug 的关键工具，且 AGENTS §6 的精神是"踩过的坑留下可复现工具"。已设 `required-features`，不破坏 `--no-default-features` 构建。

需要补：把 `FIDUS_PROBE_CYCLE` / `FIDUS_PROBE_BURST` 写进 README 的排障章节。

### 4.7 🟢 实机验证怎么做才算数？

**教训**：我前面所有"批次对比"都被自己的终端输出污染过。

验证协议必须固定：
- 所有输出重定向到文件，**跑完再读**；
- 交错 A/B（而非先跑完一组再跑另一组），消除时间漂移；
- **同时记录背景噪声底**（`FIDUS_PROBE_BURST`），否则无法区分"修好了"和"这次桌面恰好安静"。

---

## 5. 实施顺序

1. 补测颜色容差（§4.1），确定 `color_tolerance` 默认值。
2. 加 `Noise::ForeignRedraw` 仿真 + **会失败的回归测试**（§3.3）。
3. 改动一：L9 接颜色判据。验证回归测试转绿。
4. 按裁决结果实施改动二（A/B/C 之一）。
5. 实机验证，按 §4.7 的协议。

---

## 6. 验收标准

- [x] 真实桌面（有窗口活动）下 scale=1 与 1.25 各 15 次，**失败 0 次** — 噪声底 9/29 确认桌面确实在动
- [x] 仿真回归测试在改动前**确实失败**（变异验证）— 见下方"变异验证结果"
- [x] scale=1 残差仍为 **0.000px**，测试从 98 增至 **100** 全绿
- [x] `cargo clippy` 零警告，`cargo doc --workspace --no-deps` 零警告 — 通过将 live smoke-test 二进制改名为 `fidus-live-calibrate` 清除了 bin/lib 同名冲突
- [x] 每个新增判据都有 AGENTS §1 的三段式失效模式注释
- [x] 实机验证按 §4.7 协议，脚本固化为 `scripts/l9_live_soak.sh`

### 变异验证结果

| 关掉的机制 | `foreign_window_repaints…` | `same_colored_repaints…` |
|---|---|---|
| 颜色判据（tolerance→255） | **FAILED**（2 plausible regions） | — |
| 多帧互证（corroborations→1） | ok ⚠️ | **FAILED**（残差 272px） |

第一次变异验证暴露了一个问题：**`corroborations` 在原测试下是个没被测到的旋钮**（AGENTS §7）——颜色判据一个人就扛下来了。因此补了 `ForeignRedrawSameColor`：干扰物用**标记自己的颜色**重绘，颜色判据原理上失效，只有"再看一次"能救。**两条机制现在各自被独立证明是承重的。**

---

## 7. 实测环境与外推边界

本次实机数据来自 Arch Linux + Niri + 接近默认的 kitty：kitty 配置只有 `shell /usr/bin/fish`，没有透明度、主题、阴影等美化设置；Niri 配置仍使用默认动画和焦点环，并运行 waybar、mako、swww。它验证了“用户正在使用电脑时，普通窗口会重绘”这一问题，但**不代表所有桌面像素行为相同**。

尚未由本次数据覆盖的场景包括：透明/模糊/阴影主题、复杂终端配色、视频或网页动画、频繁 compositor 动画、其他 Wayland 合成器。它们不是设计上的豁免；它们是后续实机验证矩阵中的场景。颜色自适应与多帧互证应降低风险，但持续且同色的干扰仍按 §6 的诚实失败处理。

## 8. 明确不做

- **不碰投射端取整**（§4.5）。
- **不删面积先验**（§4.4）。
- **不改分数缩放相关的任何东西**——已证明与本 bug 无关，动它只会引入新变量。
