# 交付说明 · Windows 后端（`fidus-backend-windows`）

> 这份文件是**给评审者的交接单**：改了什么、为什么这么改、怎么自己验一遍、哪些还没验。
> 设计与实测依据见[提案](proposals/windows-backend.md)；原始读数见
> [实测记录](measurements/windows-backend-primitives.md) 与
> [常驻探针输出](measurements/windows-backend-live-probe.txt)。
> 日期：2026-10 ｜ 状态：**已实现、已实机验证；未 commit / 未 push；3 项门禁未在本机执行**

---

## 1. 一句话

在没有 Rust 工具链的 Windows 机器上从零搭出可复现的实测环境，先证明"**分层窗口投射 + `BitBlt` 截屏**"
能解出 **0.000 px** 残差的单位仿射、并**否决**了两个看起来更简单的方向，然后按 `backend-contract.md`
实现 **2135 行** Rust + **15 个无显示测试** + 一个常驻实机诊断；**校准器、估计器、Gate 判据一行未改**。

## 2. 交付清单

### 2.1 新增

| 路径 | 性质 | 行数 |
|---|---|---|
| `crates/fidus-backend-windows/src/lib.rs` | crate 文档（含"为什么是这两个原语"） | 60 |
| `crates/fidus-backend-windows/src/api.rs` | 平台中立：`Platform` trait、`MarkerSpec`/`RawFrame`/`ProbeFacts`、能力→环境上下文 | 224 |
| `crates/fidus-backend-windows/src/plan.rs` | 落点量化/校验、`RawFrame → Frame` 校验转换（纯函数） | 114 |
| `crates/fidus-backend-windows/src/projector.rs` | owner 线程 + 命令通道 + 幂等 teardown | 190 |
| `crates/fidus-backend-windows/src/sys.rs` | Win32：DPI、工作区、分层窗口、DIB/`BitBlt`、`DwmFlush`、残留计数、命中测试 | 475 |
| `crates/fidus-backend-windows/src/backend.rs` | `WindowsBackend` + 两个 session + `IoFactory` | 207 |
| `crates/fidus-backend-windows/tests/sim.rs` | **15 个无显示测试**（任何平台可跑） | 450 |
| `crates/fidus-backend-windows/Cargo.toml` | 依赖：`fidus-core`、`thiserror`；`windows-sys` 仅在 `cfg(windows)` | 24 |
| `crates/fidus/src/bin/fidus-windows-probe.rs` | 常驻实机诊断（驱动已发货后端 + 真实 Anchor + 真实 gate） | 415 |
| `tools/winprobe/` | **测量工具**（设计空间：3 截屏 × 2 投射 × 同步有无）+ README | 1353 |
| `docs/proposals/windows-backend.md` | 提案 + 实现/验证记录 + 三问审查 | 269 |
| `docs/measurements/windows-backend-primitives.md` | 设计空间实测记录（含实现后复现对照） | 139 |
| `docs/measurements/windows-backend-primitives.txt` | 测量工具原始输出 | 94 |
| `docs/measurements/windows-backend-live-probe.txt` | 常驻探针原始输出 | 32 |

行数为**文件总行数**（含空行与注释）；Rust 侧合计 2135 行（含 `Cargo.toml` 2159 行）。

### 2.2 修改（`git diff --stat`：11 个文件，+114 / −5）

| 文件 | 改动 |
|---|---|
| `Cargo.toml` | workspace 成员 + `workspace.dependencies` 各加一行 |
| `Cargo.lock` | **纯新增 10 行**（新包 + 成员条目），无版本变动 |
| `crates/fidus/Cargo.toml` | 新增 `windows` feature 与 `fidus-windows-probe` bin；三个后端均为普通 optional 依赖 |
| `crates/fidus/src/builder.rs` | `BackendChoice::Windows`、`build_windows`/`connect_windows`、`Auto` 追加 windows 分支、无后端时的报错信息带上 Windows 构建方式 |
| `crates/fidus/src/lib.rs` | `pub mod windows`（`cfg(all(feature = "windows", windows))`） |
| `crates/fidus-core/src/env.rs` | `CompositorKind::Windows` + `parse` 接受 `windows`/`win32`/`win` |
| `crates/fidus/src/bin/fidus-live-calibrate.rs` | 两处 match 臂（`choice_name`、`compositor_token`）+ `FIDUS_BACKEND=windows` |
| `README.md` | 平台现状、workspace 结构、Windows 探针命令、已知限制两节 |
| `docs/backend-contract.md` | §5 参考实现表加 Windows 行 + 三条可移植的实测结论 |
| `docs/proposals/README.md` | 提案与实测记录两处索引 |
| `.gitignore` | 仓库内本地工具链 `.toolchain/` |

## 3. 怎么验

### 3.1 任何平台（无显示，含 Linux CI）

```bash
cargo test -p fidus-backend-windows            # 15/15
cargo test -p fidus-core -p fidus-calibrate    # 本机实测 50 个既有测试全过
cargo clippy --workspace --all-targets -- -D warnings   # ← 本机未执行，见 §6
cargo doc --workspace --no-deps
bash scripts/ci_fidus_test.sh                  # ← 本机未执行，见 §6
```

无显示测试里最关键的一条是 **"投射必须先呈现再返回"**：
`projection_presents_then_synchronises_before_returning` 断言命令序列为
`create → present → sync`。把 `set_markers` 里的 `sync_presentation()` 删掉，它立刻变红——
这正是实机实测"无同步只有 8/20 命中"那个坑的回归。

### 3.2 Windows 实机（会在桌面上闪现标记，退出即清理）

```powershell
cargo run --release --no-default-features --features windows -p fidus --bin fidus-windows-probe
```

期望读数（本机 1920×1080 / 工作区 1920×1032 / DPI 96 / 单输出）：

```
dpi_awareness = PerMonitorV2 (physical=true)      capture_probe = ok
environment = layer_shell:false multi_marker:true capture:Granted compositor:Windows
projection: trials=20 found=20 centroid_exact=20 colour_exact=20 area_exact=20
            clear_clean=20/20 residual_max=0.000px residual_mean=0.000px
input:      four_sentinels=4 intercepted_by_marker=0/4 ; after_teardown_live_marker_windows = 0
gate:       Crosshair = NotSupported{MissingProtocol{zwlr_layer_shell_v1}} ; Anchor = Available
calibrate = OK method Anchor ; map_scale = 1.000000
            rms/max/verification/consistency = 0.000 px ; sample_count = 4
final_live_marker_windows = 0 (must be 0)
```

### 3.3 设计空间复测（质疑"为什么是这两个原语"时）

```bash
cargo build --release --manifest-path tools/winprobe/Cargo.toml
./tools/winprobe/target/release/winprobe.exe
```

会出现被否决的三个格子（`gdi × srccopy × 无同步` 40%、`PrintWindow` 0%、非分层命中 4/4）——
这是保留这个工具的唯一理由：常驻探针只走已发货配置，重测不了备选。

## 4. 实测证据摘要

### 4.1 设计矩阵（每格 20 轮，8/28 px 标记交替，位置伪随机）

| 投射 | 光栅操作 | `DwmFlush` | 命中 | 质心精确 | 清除干净 | 残差 |
|---|---|---|---|---|---|---|
| GDI | `SRCCOPY` | 否 | **8/20** | 8/8 | 8/8 | 0.000 px |
| GDI | `SRCCOPY` | 是 | 20/20 | 20/20 | 20/20 | 0.000 px |
| GDI | `+CAPTUREBLT` | 否 | 20/20 | 20/20 | 20/20 | 0.000 px |
| **分层** | 任意 | 任意 | **20/20** | **20/20** | **20/20** | **0.000 px** |
| 两者 | `PrintWindow` | 任意 | **0/20** | — | — | — |

* 穿透：非分层 `WindowFromPoint` 命中标记 **4/4**；分层 **0/4**。
* teardown：矩阵 8 格 + 2 次 Anchor + 探针全程，残留窗口恒 **0**。
* 端到端：真实 `AnchorCalibrator`（2 pass、4 色哨兵同时投射）解出**单位仿射**
  `[1, ≈0, ≈0, 0, 1, 0]`，rms / verify / consistency **全 0.000 px**（Wayland 分数缩放下同一量最大 0.308 px）。

### 4.2 测试

`cargo test -p fidus-backend-windows` → **15/15 通过**（任何平台可跑）；`fidus-core` + `fidus-calibrate`
既有测试全通过（新增 `CompositorKind::Windows` 未破坏既有 gate 断言）。

## 5. 设计与取舍（每条都有代价说明）

1. **投射 = 分层窗口**（`WS_EX_LAYERED|TOPMOST|TOOLWINDOW|NOACTIVATE|TRANSPARENT` + `UpdateLayeredWindow`）：
   唯一同时满足"落点精确 + 不截获鼠标"的组合；备选（非分层）已被实测否决。
2. **同步 = `DwmFlush`**：Wayland frame callback 的语义等价物；无它时纯 `SRCCOPY` 只有 40% 命中。
   截屏同时带 `CAPTUREBLT`——**这一对是明码标价的冗余**，两条独立路径都实测 100%，保留理由是该保证是契约里最脆的一条。
3. **逻辑空间 = 主显示器工作区**（`SPI_GETWORKAREA`）：尺寸进 `usable_size_hint()`，原点只作投射偏移，
   **两者都不进 `CoordinateFrame`**；判据是契约 §1 的"提示错了会怎样"——错了标记落到工作区外、检测不到、自动重试。
   除它之外，后端**不调用** `GetWindowRect` / `GetClientRect` / `ClientToScreen`。
4. **窗口归一个 owner 线程**：Win32 窗口是线程亲和的，而 `CalibrationIo: Send`、`Drop` 可能落在任意线程；
   手工 `unsafe impl Send` 的失效形态是**残留窗口**（违反原则五）。代价：一个线程 + 两个通道。
5. **能力而非身份**：`has_layer_shell=false` → L9 被 Gate 以 `MissingProtocol` **提前**拒绝（不会误导调用方去申请帮不上忙的截屏权限）；
   `multi_marker_projection=true` → L0 Anchor `Available`。Windows 上跑 L0 是能力决定的结果。

## 6. 未执行的门禁（不得当成通过）

| 门禁 | 为什么没跑 | 状态 |
|---|---|---|
| `cargo clippy -- -D warnings` | 本机 rustup 未装 clippy；下载组件时本地代理整个掉线（`static.rust-lang.org` / `rsproxy.cn` 均拒连），反复失败 | **未执行**。已按 clippy 默认 lint 逐项收紧新代码（`Win32::new`→`connect` 避开 `new_without_default`、`BITMAPINFO` 显式构造避开 `field_reassign_with_default`、回复通道提取类型别名），但那是**推断不是读数** |
| Linux `cargo test/clippy/doc --workspace`（离线 vendor 门禁） | 本机无 Linux 目标 std；`vendor/` 在 Windows 检出时被 CRLF 改写，`thiserror` 的 `.cargo-checksum.json` 不匹配 | **未执行**。已按"对 Linux 零改动"设计：三个后端保持普通 optional 依赖、`default = ["wayland-layer","x11"]` 不变、`Cargo.lock` 纯新增 10 行 |
| `cargo metadata --filter-platform x86_64-unknown-linux-gnu` | 需要 `linux-raw-sys`，代理掉线后取不到 | **未执行**。已用 Windows 平台过滤跑通 `cargo metadata --locked --offline`（exit 0），证明 lock 与清单一致 |

## 7. 未验证的边界（附证伪方式）

| 项 | 证伪方式 |
|---|---|
| **非 100% 缩放**（本机 DPI 96、单输出） | 在缩放 ≠ 100% 的机器重跑 `fidus-windows-probe`；若仿射不再是 identity 或残差 > 0，落点换算必须改（改动范围在后端内部） |
| **真实鼠标点击穿透** | 在受控受害窗口上手点一次；当前只有 `WindowFromPoint` 代理证据（契约 §4 清单项） |
| 多显示器 / 混合 DPI | 多输出机器跑探针；Gate 已对 `multi_monitor_count > 1` 判 `Degraded`（spec §10 问题 4 未决） |
| 运行中 DPI 变更（`WM_DPICHANGED`） | 切换缩放后重跑同一位置，看落点是否漂移 |
| HDR / 宽色域 | HDR 机器跑探针，看标记是否仍逐字节等于哨兵色（检测器容差 8/通道） |
| 独占全屏 / UAC 安全桌面 / 远程会话 | 探针在连接期 1×1 截屏失败即报 `Revoked`，Gate 诚实拒绝（不产出假坐标） |

## 8. 评审需要决定的三件事

1. **是否合并**：`--no-default-features --features windows` 的 opt-in 方式是否可接受
   （原因：`wayland-sys` 在 Windows 上编译不过，而 Cargo 无法表达"只在某平台默认启用该 feature"，
   也无法让 `fidus-probe-marker` 的 `required-features` 按平台生效）。
2. **是否剪掉 `CAPTUREBLT`**：它与 `DwmFlush` 是两条独立保证，剪任一条都只少一处成本，但保证从双点变单点。
3. **是否保留 `tools/winprobe`**：它 1353 行、独立 workspace、不进 CI，是唯一能重测被否决备选的地方。

## 9. 本机复现的前置条件（踩过的环境坑）

* 沙箱内 shell 无法直连外网，需经本机代理 `http://127.0.0.1:65532`（`git`、`cargo`、`rustup` 均要）。
* MSVC 链接器需要能写临时目录：把 `TMP`/`TEMP` 指到工作区内，否则 `LNK1104`。
* `vendor/` 在本机被 CRLF 改写导致离线 vendor 门禁不可用（**这是 Windows 检出问题，不是本改动的缺陷**；
  Linux CI 不受影响）。Windows 上请走 registry。
* 仓库内本地工具链在 `.toolchain/`（已 gitignore），未改系统 PATH。

## 10. 后续（各自需单独授权 / 属于另一个工作项）

* 在 Linux 上跑一次完整离线门禁（§3.1 的四条命令），把输出补进提案 §9.4。
* 在缩放 ≠ 100% 的 Windows 机器上重跑探针，补齐 §7 第一项。
* macOS 后端是**另一个 proposal**（`NSPanel` + `CGWindowListCreateImage` + 屏幕录制权限状态机，
  `PermissionState::RequiresRestart` 已预留）。
* commit / push / 上游 PR 属于对外不可逆动作，按 AGENTS 禁忌 7 需你单独授权。
