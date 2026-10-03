# 实测 · Windows 投射与截屏原语

> **目的**：回答提案 [`docs/proposals/windows-backend.md`](../proposals/windows-backend.md)
> 的核心问题——Windows 上"**我们告诉系统标记在哪**（自建分层窗口）+ **截自己的屏**（GDI）"
> 能否满足 backend-contract 的两条原语，以及**哪种原语组合**成立。
> 判据不是"能截到图"，而是**真实 `AnchorCalibrator` + 真实检测器跑出来的残差**。
>
> 环境：Windows 11（OS 10.0.26200），DWM 合成开启，单输出 1920×1080，
> 工作区 1920×1032 @ (0, 0)，系统 DPI 96（**100%，非分数缩放**），
> Rust 1.99.0，`windows-sys` 0.61.2
> 复现：见 [`tools/winprobe/README.md`](../../tools/winprobe/README.md)
> 原始输出：[`windows-backend-primitives.txt`](windows-backend-primitives.txt)
> 日期：2026-10

---

## 结论速览

**四条结论，其中两条与我的初始假设相反。**

| # | 结论 | 证据 |
|---|---|---|
| 1 | **落点精确到 0.000 px**：我们在整数屏幕坐标放的窗口，就在截屏的同一像素上 | 可见性探针 bbox `(200,200)-(231,231)` = 期望 `(200,200)`；20 轮 `centroid_exact` 全中，残差 0.000 px |
| 2 | **"已显示"必须显式同步**：不加同步时纯 `SRCCOPY` 只有 **8/20 = 40%** 命中 | 见下表 `gdi / srccopy / flush=false` |
| 3 | **只有分层窗口能过"不截获鼠标"**：非分层窗口（即使带 `WS_EX_TRANSPARENT`）被 `WindowFromPoint` 命中 **4/4** | 见"输入"一节 |
| 4 | **`PrintWindow(GetDesktopWindow)` 完全不可用**：`ok=0`、0 个标记像素、20/20 轮整帧内容都不同 | `changed_px=39 535 140` ≈ 20 × 全工作区 = 每轮**每个**像素都不同 |

### 设计矩阵（每格 20 轮，两种标记尺寸 8/28 px 交替，位置伪随机）

| 投射 | 光栅操作 | `DwmFlush` | 命中 | 质心精确 | 颜色精确 | 面积正确 | 清除干净 | 残差 |
|---|---|---|---|---|---|---|---|---|
| GDI | `SRCCOPY` | 否 | **8/20 (40%)** | 8/8 | 8/8 | 8/8 | 8/8 | 0.000 px |
| GDI | `SRCCOPY` | 是 | 20/20 | 20/20 | 20/20 | 20/20 | 20/20 | 0.000 px |
| GDI | `SRCCOPY\|CAPTUREBLT` | 否 | 20/20 | 20/20 | 20/20 | 20/20 | 20/20 | 0.000 px |
| GDI | `SRCCOPY\|CAPTUREBLT` | 是 | 20/20 | 20/20 | 20/20 | 20/20 | 20/20 | 0.000 px |
| 分层 | `SRCCOPY` | 否 | 20/20 | 20/20 | 20/20 | 20/20 | 20/20 | 0.000 px |
| 分层 | `SRCCOPY` | 是 | 20/20 | 20/20 | 20/20 | 20/20 | 20/20 | 0.000 px |
| 分层 | `SRCCOPY\|CAPTUREBLT` | 否 | 20/20 | 20/20 | 20/20 | 20/20 | 20/20 | 0.000 px |
| 分层 | `SRCCOPY\|CAPTUREBLT` | 是 | 20/20 | 20/20 | 20/20 | 20/20 | 20/20 | 0.000 px |
| 两者 | `PrintWindow` | 任意 | **0/20** | — | — | — | 0/0（无从检查） | — |

* **"清除干净"只统计标记真的出现在该原语帧里的轮次**（`found` 轮）。原语没看见标记时，
  "清除干净"是平凡真，不能计入——第一版探针犯了这个问题，已修。
* `marker_px`（整帧里精确等于标记色的像素数）与 `changed_px` 用于区分
  "**窗口根本没进画面**" 与 "进了画面但位置/颜色不对"：失败格全部是前者（`marker_px=0`），
  没有任何一格出现"位置偏差但颜色正确"，即**没有量化误差**。

### 端到端：真实 L0 Anchor

对两种投射方式各跑一次**真实** `AnchorCalibrator`（2 pass、每种颜色 1 次校验、8 次真实截屏）：

| 投射 | 结果 | 解出仿射 | rms | verify | consistency | 采样 | 捕获 | wall |
|---|---|---|---|---|---|---|---|---|
| 分层 | ✅ OK | `[1, 0, 0, ≈0, 1, ≈0]`，scale **1.000000** | **0.000 px** | 0.000 | 0.000 | 4 | 8 | 499 ms |
| GDI | ✅ OK | 同上（逐位相同） | **0.000 px** | 0.000 | 0.000 | 4 | 8 | 541 ms |

对比 `docs/measurements/l9-fractional-scaling.md`：Wayland 分数缩放下 rms 中位数 0.089–0.208 px、
最差 0.308 px。**Windows 实测与整数缩放的 Wayland 同级（0.000 px）**——因为在此环境下
PerMonitorV2 进程的坐标空间与截屏像素空间是同一个空间，不存在合成器侧的缩放取整。

`anchor_residual_windows_after_teardown=false`（两种设计）：`FindWindowW` 在我们的窗口类上
返回 null，即校准结束后**无残留窗口**——这是 `niri msg layers` / `xwininfo -root -tree`
的 Windows 等价物。

---

## 成因：为什么必须同步，"分层"又为什么不同

**为什么 `SRCCOPY` 不加同步只有 40%**：`ShowWindow` + `UpdateWindow` 只保证**我们的**
绘制进入窗口重定向面，DWM 把它合成到屏幕扫描输出是异步的（下一次 vblank）。截屏
`BitBlt(GetDC(NULL))` 读的是**已扫描输出的桌面**。因此"刚 show 完立刻截"有约 60% 的概率
截到旧内容——**这正是 backend-contract §2.2 说的最容易错的那条**，在 Windows 上以
40% 命中率的形式显形。

**三种都能修正它**（各自都测到 100%）：

1. `DwmFlush()`：阻塞到下一次合成完成——语义上就是 Wayland 的 frame callback；
2. `CAPTUREBLT`：该光栅操作要求 DWM 把分层/顶层窗口纳入合成结果，实测同样 20/20；
3. 分层窗口 + `UpdateLayeredWindow`：内容经 DWM 同步提交，**不加同步也 20/20**。

**为什么选分层窗口**：只有它同时满足"不截获鼠标"。非分层窗口即使带 `WS_EX_TRANSPARENT`
仍被 `WindowFromPoint` 命中 4/4；分层 + `WS_EX_TRANSPARENT` 为 0/4。

**一个被证伪的机制假设**：我原以为穿透来自 `DefWindowProc` 对 `WS_EX_TRANSPARENT` 窗口的
`WM_NCHITTEST → HTTRANSPARENT`。实测**两种设计都返回 1（`HTCLIENT`）**，而穿透行为只在
分层设计上出现——所以穿透发生在**命中测试选择**阶段，而不是 `WM_NCHITTEST` 的返回值。
这条记下来，免得实现时靠一个不成立的机制去"解释"行为。

## 未测量（与证伪方式）

| 项 | 为什么没测 | 怎么证伪 |
|---|---|---|
| **125% / 150% 缩放下的落点精度** | 本机 `dpi_for_system = 96`，单输出，无第二个显示器；改缩放要改用户显示配置（侵入） | 在缩放 ≠ 100% 的机器上跑同一探针。若仿射不再是 `[1,0,0,0,1,0]` 或残差 > 0，则实现必须改为按物理像素换算后再落点 |
| **真实鼠标点击穿透** | 合成输入（`SendInput`）会移动用户鼠标并可能点到下层窗口 | 在受控受害窗口上手点一次（contract §4 清单项）；自动化的 `WindowFromPoint` 代理已通过 |
| 多显示器 / 混合 DPI | 单输出 | 多输出机器上跑探针；gate 对 `multi_monitor_count > 1` 已判 `Degraded`，spec §10 问题 4 未决 |
| 运行中 DPI 变更（`WM_DPICHANGED`） | 需要真实缩放切换 | 切换缩放后重跑同一位置，看落点是否漂移 |
| HDR / 宽色域输出 | 本机 SDR | HDR 机器上跑探针，看标记像素是否仍逐字节等于 `#FF00FF`（检测器容差 8/通道） |
| 独占全屏 / UAC 安全桌面 / 远程会话 | 会打断用户 | 探针在连接期 1×1 截屏失败即报 `Revoked`，gate 诚实拒绝（不产出假坐标） |

## 实现后的复现：常驻诊断驱动的是同一个后端

提案阶段的数字来自一个**独立测量工具**（[`tools/winprobe/`](../../tools/winprobe/)）。它是唯一能重测
**被否决的备选**（`PrintWindow`、非分层窗口、无同步）的地方，因此保留；但它测的是自己那份私有原语。

实现完成后，同一批读数由**常驻诊断** `fidus-windows-probe` 用**已发货的后端**
（[`crates/fidus-backend-windows`](../../crates/fidus-backend-windows/)，经 `FidusBuilder` 组装）复现：

| 读数 | `tools/winprobe`（提案阶段） | `fidus-windows-probe`（已发货后端） |
|---|---|---|
| 命中 | 20/20 | 20/20 |
| 质心精确 | 20/20 | 20/20 |
| 颜色精确 | 20/20 | 20/20 |
| 面积正确 | 20/20 | 20/20 |
| 清除干净 | 20/20 | 20/20 |
| 落点残差 | 0.000 px | 0.000 px |
| 解出仿射 | 单位阵 | 单位阵（scale `1.000000`，系数 `[1, ≈0, ≈0, 0, 1, 0]`） |
| 鼠标命中标记窗口 | 分层 0/4、非分层 4/4 | **0/4** |
| teardown 后残留窗口 | 0 | 0 |
| 单次工作区截屏 | min 4.28 / median 30.80 ms | min 22.07–25.04 / median 32.50–41.96 ms（两次运行各 n=40） |

原始输出：[`windows-backend-live-probe.txt`](windows-backend-live-probe.txt)。

**关键三项（穿透、残留、残差）一致**；唯一有差异的是截屏耗时，两者同量级但**跨运行波动明显**
（同一份代码两次运行中位 32.5 → 42.0 ms，桌面自身活动也在被测），差异主要来自探针侧拷贝次数与
DIB 分配命中，**不是后端行为差异**。后端每次都新分配一张 8 MB DIB，这个取舍与其理由见 README「已知限制」。

同一份输出还给出 gate 的实测回答：`Crosshair → NotSupported{MissingProtocol{zwlr_layer_shell_v1}}`、
`Anchor → Available`——**L0 是因为"能同时投四个标记"这个能力被选中的，不是因为平台身份**。

## 原始数据

* 完整 stdout：[`windows-backend-primitives.txt`](windows-backend-primitives.txt)（94 行）
* 采集工具：[`tools/winprobe/`](../../tools/winprobe/)（独立 workspace，不进主 workspace 的
  `cargo test --workspace` 与离线 vendor 门禁）
* 关键环境读数：`dpi_awareness = PerMonitorV2`、`dwm_composition enabled=1`、
  `capture_sanity changed_over_250ms = 0`（静止桌面逐像素恒定，说明截的**不是**冻结画面）、
  `nonblack=99.81%`、`distinct=718`
* 单次工作区截屏耗时：`n=408 min 4.28ms median 30.80ms max 52.72ms`（**含**探针侧的
  DIB 分配与整帧 `Vec` 拷贝；实现应按帧复用缓冲区，此数是上界）
