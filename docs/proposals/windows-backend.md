# 提案 · Windows 后端（`fidus-backend-windows`）

> **想读懂设计** → [`docs/spec.md`](../spec.md)；**后端要满足什么** → [`docs/backend-contract.md`](../backend-contract.md)
> **实测记录** → [`docs/measurements/windows-backend-primitives.md`](../measurements/windows-backend-primitives.md)
> **复现脚本** → [`tools/winprobe/`](../../tools/winprobe/)
> 日期：2026-10 ｜ 状态：**待人工审阅**（提案未获批准前不写实现代码）

---

## 0. 一句话

在 Windows 上用**分层窗口**（`WS_EX_LAYERED` + `UpdateLayeredWindow`，我们自己选的坐标）
投射标记，用 **`BitBlt(GetDC(NULL))`** 截取工作区像素，把一个约 400 行的
`fidus-backend-windows` 挂到 `FidusBuilder` 上；**校准器与估计器一行不改**。
两条原语已在实机验证到 **0.000 px** 残差，四个候选方向里**两个被实测否决**。

## 1. 解决谁的问题（这一节必须答得诚实）

**先说清楚：Windows 与 Wayland 不同，平台 API 在 Windows 上是"能用"的。**
`GetWindowRect` 在单屏 100% 缩放下是准的。所以本节不能靠"平台不给坐标"这套理由，
必须回答**到底哪个真实场景不够用**：

| 场景 | 现有方案（Win32 直读） | 不够用在哪 |
|---|---|---|
| **跨平台调用方**（同一份代码要在 Wayland/X11/Windows 上跑，例如 `fidus-py` 的宿主） | 在 Windows 上必须**另写一条分支**读 `GetWindowRect` | 拿到的量纲与其余平台不同：一边是"测量出来的、带质量指标（rms/verify/consistency）的仿射"，另一边是"平台报的一个矩形"。**同一个 API 返回两种不同性质的东西**，调用方无法对二者做同一套校验 |
| **混合 DPI 多屏** | 未声明 DPI 感知时 Windows 会**虚拟化**坐标；跨屏移动后 `GetWindowRect` 与实际像素不一致（README 已记录） | 这是**会静默产出错误位置**的那一类值——正是原则一针对的形态；fidus 的做法是实测后给出带残差的映射，错了能被拒绝门限抓住 |
| **"我的可见内容在哪"** | `GetWindowRect` 给的是**窗口框架**矩形（含不可见缩放宽边/阴影），要拿到可见内容还要 `DwmGetWindowAttribute(DWMWA_EXTENDED_FRAME_BOUNDS)` + 客户区换算 | 调用方要自己拼一条平台专属链路，而这条链路的每一步都不会告诉它"你拼错了" |

**同时必须承认代价与边界**：fidus 不做窗口枚举、不做身份识别、只定位**调用方自身**窗口的
内容；调用方若确定只在单屏 100% 缩放下运行，**这个后端买不到任何东西**，直接用
`GetWindowRect` 即可。本提案的价值全在上面三行，以及**与其他平台语义一致**这一条。

> 这也是本提案可能得出"不做"的地方：如果维护者认为"跨平台语义统一"不足以支撑一个
> 后端，那么正确的结论是**否决本提案**，而不是先实现再看。§6 列出了被实测否决的方向，
> §7 列出了实现成本，供这个判断使用。

## 2. 核心假设（已实测，数字来自实机）

| 假设 | 结果 |
|---|---|
| **A1 落点精确**：整数屏幕坐标放窗口 → 截屏同一像素 | ✅ **0.000 px**，20/20 轮质心/颜色/面积全中，无任何位置偏差样本 |
| **A2 端到端可用**：真实 `AnchorCalibrator` 能解出坐标帧 | ✅ 解出**单位仿射**（scale 1.000000），rms / verify / consistency **全 0.000 px**，4 采样 2 pass，8 次真实截屏，499–541 ms |
| **A3 "已显示/已消失"可保证** | ✅ 需要显式同步：无同步的纯 `SRCCOPY` 仅 **40%** 命中；加 `DwmFlush` 或 `CAPTUREBLT` 或分层提交后 **100%** |
| **A4 标记不截获鼠标** | ✅ **仅分层窗口成立**（`WindowFromPoint` 命中 0/4；非分层 4/4） |
| **A5 不留残留窗口** | ✅ clear 后与 `Drop` 后 `FindWindowW` 均为 null（8 格矩阵 + 2 次 Anchor 全部 `residual=false`） |
| **A6 截的不是冻结画面** | ✅ 静止桌面连采差分 `changed_over_250ms = 0`，非黑像素 99.81%，采样到 718 种颜色；`BitBlt` 失败会真实返回 0（`PrintWindow` 那格就是活证据） |

完整表与逐格数字见实测记录；此处不复制第二遍。

## 3. 被实测否决的方向

| 方向 | 否决依据 |
|---|---|
| **`PrintWindow(GetDesktopWindow, PW_RENDERFULLCONTENT)` 作截屏原语** | `ok=0`、标记像素 **0**、20/20 轮整帧全变（`changed_px=39 535 140` ≈ 20 × 全工作区）。它返回的是一帧**每轮都不同**的内容，正好是 contract §2.1 说的"别拿黑帧/假帧充数" |
| **非分层 GDI 窗口作投射原语** | "不截获鼠标"失败 4/4（即使带 `WS_EX_TRANSPARENT`）→ 违反 contract §2.2 与 spec §11.4 |
| **"穿透靠 `WM_NCHITTEST → HTTRANSPARENT`"（我的机制假设）** | 实测两种设计都返回 `HTCLIENT(1)`，穿透行为却只在分层设计出现 → 机制假设**证伪**；实现不得依赖这个解释 |
| **对窗口句柄手工 `unsafe impl Send`** | 窗口是**线程亲和**的：`DestroyWindow` 必须在创建它的线程上调用。手工 `Send` 会让"跨线程 Drop"静默泄漏窗口（原则五）。改用**专用 owner 线程**（§4 D5） |
| 无同步 + 纯 `SRCCOPY` | 40% 命中，等于把"标记未上屏"当成"环境有问题"，测量全部作废 |

## 4. 设计决策与成本对账

| # | 决策 | 依据 / 代价对账 |
|---|---|---|
| **D1** | 投射 = `WS_EX_LAYERED \| TOPMOST \| TOOLWINDOW \| NOACTIVATE \| TRANSPARENT` + `UpdateLayeredWindow`（预乘 ARGB 纯色位图） | 唯一同时满足"精确落点 + 不截获鼠标"的组合。备选（非分层）已实测否决；代价：每次 show 要建一张 `size×size` DIB（8–28 px 见方，可忽略） |
| **D2** | 同步 = show 批次后与 clear 批次后各一次 `DwmFlush()` | 分层设计**不加也 100%**，但非分层/其它 GPU 组合上不加只有 40%。它是 Wayland frame callback 的语义等价物，且有实测差异作为存在理由。代价：每次测量约 1–2 帧（16 ms 量级），Anchor 全程仍 < 550 ms |
| **D3** | 截屏 = `BitBlt(GetDC(NULL), …, SRCCOPY \| CAPTUREBLT)` → 32bpp **top-down** DIB（`biHeight = -h`），区域 = **主显示器工作区** | `CAPTUREBLT` 是"把分层窗口纳入合成结果"的文档化开关，实测非分层无同步的格子靠它从 40% → 100%，零额外系统调用。备选（去掉它）实测也能过，但把保证压在一件文档说"需要"的事情上；这条冗余**明码标价**留在提案里，评审若认为该剪，剪它 |
| **D4** | 逻辑空间 = **主显示器工作区**（`SPI_GETWORKAREA`）：尺寸进 `usable_size_hint()`，**原点只作投射偏移**，两者都不进 `CoordinateFrame` | 与 layer-shell 的 usable-area 语义对齐（不把标记放到任务栏底下）。判据是 contract §1 的"提示错了会怎样"：提示错了 → 标记落到工作区外 → 截不到 → 检测失败 → 重试，**自我纠正**，不会静默产出错误坐标。备选（逻辑空间 = 整屏）被拒：多一个"标记可能被任务栏盖住"的失败源 |
| **D5** | 所有窗口归一个**专用 owner 线程**（通道收发命令，`Drop` join 回收）；截屏留在调用线程 | 窗口线程亲和 + `Drop` 可能在任意线程 → 只有 owner 线程能保证"销毁发生在创建线程"。备选（手工 `Send` + 外来线程报错）的失效形态是**残留窗口**，直接违反原则五。代价：一个线程 + 两个通道，约 100 行 |
| **D6** | DPI = `SetProcessDpiAwarenessContext(PER_MONITOR_AWARE_V2)`（实测接受），退化到 `SetProcessDPIAware` 并如实上报 | 让进程坐标 = 物理像素，落点才不会先被系统缩放一次（Wayland 分数缩放的教训在此**不适用**，因为实测同空间）。代价：无 |
| **D7** | 依赖 = `windows-sys = "0.61"`（**已在 `vendor/` 与 `Cargo.lock`**，被 `rustix`/`errno` 间接引入），不新增任何第三方 crate | 用官方绑定而不是手写 `extern`：省掉 ABI 自证，且**不触发 `cargo vendor` 重建**（那是 96 MB 仓库里的派生输入，契约上"必须随 lockfile 一起再生"）。许可证 MIT OR Apache-2.0，与仓库一致 |
| **D8** | `CompositorKind` 增加 `Windows` 变体（而不是 `Other("windows")`） | 与既有 `X11` 变体同类（平台而非"其它"），日志/诊断可判别。代价：`env.rs` 一个变体 + `fidus-live-calibrate` 一个 match 臂（2 行） |
| **D9** | feature 接线：`fidus` 增 `windows` feature；wayland/x11 依赖移入 `[target.'cfg(unix)'.dependencies]`，windows 依赖放 `[target.'cfg(windows)'.dependencies]`，三者都进 `default` | 目标是让**两个平台 `cargo build` 都开箱可用**。**这是本提案唯一无法在 Linux 上编译验证的改动**：后备方案是保持 `default = ["wayland-layer","x11"]`、`windows` 仅 opt-in（对 Linux CI 零风险，代价是 Windows 用户要写 `--no-default-features --features windows`）。实现时先用 `cargo metadata --filter-platform x86_64-unknown-linux-gnu` + Windows 侧真实构建验证，**验不过就退回后备方案**，并在 README 写明 |

## 5. 内部自洽检查（AGENTS §10 第 3 问）

* **零信任（原则一）**：后端不调用 `GetWindowRect` / `GetClientRect` / `ClientToScreen`，
  也不调用任何"报告窗口几何"的 API。唯一进入后端的平台几何量是 `SPI_GETWORKAREA`
  （可用区**尺寸**进提示、**原点**进投射偏移）与 `GetSystemMetrics` 的输出数/尺寸——两者都是
  **提示类**（错了会被重试自我纠正），**都不进 `CoordinateFrame`、不进概率池**（D4 的判据）。
* **概率池纯净（原则四）**：帧里的坐标全部来自我们自建的标记质心；实测解出的是**单位仿射**，
  而它是**测量得到**的（`CoordinateFrame::new` 只接受 `SolvedMap`，类型层面无法"声明"）。
  注意这条要写清楚：Windows 上"identity"不是我们写死的常量，而是 4 个检测质心解出来的结果——
  一旦平台将来改了行为（分数缩放、DPI 虚拟化），它会**自己变成非 identity 或被门限拒绝**。
* **Teardown（原则五）**：owner 线程 + 三条路径（成功 / 失败 / `Drop`）都走
  `destroy_projector`，幂等；实测 `FindWindowW` 无残留。**不是"调用过就算"，是有读数。**
* **能力探测（§3）**：`has_layer_shell=false`（L9 不可用，如实报）、
  `multi_marker_projection=true`（L0 可用）、连接期做一次 1×1 真实截屏决定
  `screen_capture_permission`（失败 → `Revoked` → gate 诚实拒绝，不校准到一半才炸）。
  **不改 gate 的任何判据**：Windows 走 L0 是"能力决定的"，不是"平台身份决定的"。
* **与既有后端不打架**：`Auto` 顺序追加 Windows；在 Windows 上 Wayland/X11 连接必然失败，
  落到 windows 分支。不改变既有平台的任何默认行为（D9 的后备方案进一步保证这点）。
* **README 一致性**：README「已知限制」现写"Windows / macOS 后端未实现……Windows: 分层窗口 +
  BitBlt"——本提案**正是这条预言的实现**，并把它从"未实现"移出（macOS 仍留待另案）。
  实现阶段同步更新 README、`backend-contract.md` §5 参考实现表（增加一行 Windows）。

## 6. 边界与未测量项（附证伪方式）

见实测记录「未测量」表：**非 100% 缩放下的落点精度**（本机 96 DPI）、**真实鼠标点击穿透**
（只有 `WindowFromPoint` 代理证据）、多显示器/混合 DPI、运行中 DPI 变更、HDR、
独占全屏/UAC 安全桌面/远程会话。

其中**只有第一条会威胁 A1 的普适性**，因此它是本提案的第一验收门槛：
实现后在缩放 ≠ 100% 的机器上重跑探针，**若仿射不再是 identity 或残差 > 0，则落点换算必须改**
（改动范围在后端内部，不涉及规范）。

## 7. 实现计划（获批后执行，含无显示测试）

| 产物 | 内容 | 规模 |
|---|---|---|
| `crates/fidus-backend-windows/src/sys.rs` | `windows-sys` 之上的类型化薄封装（DPI、工作区、DIB、BitBlt、`DwmFlush`、窗口类/窗口） | ~200 行 |
| `crates/fidus-backend-windows/src/projector.rs` | owner 线程 + 命令通道 + 分层标记窗口生命周期（幂等 destroy、`Drop` join） | ~180 行 |
| `crates/fidus-backend-windows/src/capture.rs` | 复用缓冲的工作区截屏 → `Frame`（top-down、`Xrgb8888`、stride 校验、失败 `Err`） | ~120 行 |
| `crates/fidus-backend-windows/src/lib.rs` | `WindowsBackend`：`probe_environment` / `primitives_available` / `IoFactory` / 两个 session | ~200 行 |
| `crates/fidus-backend-windows/src/bin/fidus-windows-probe.rs` | 把 `tools/winprobe` 的结果迁成**常驻**实机诊断（对齐 contract §4 清单，macOS/X11 各有等价物） | 迁移 |
| 依赖接线 | workspace members / `workspace.dependencies` / `crates/fidus/{Cargo.toml,builder.rs}` / `Cargo.lock` | 小 |
| 文档 | README 已知限制与后端表、`backend-contract.md` §5 增一行、`CompositorKind::Windows` | 小 |

**无显示仿真测试**（AGENTS §6：每个坑配一个不需要显示服务器的测试），至少覆盖：

1. **DIB → `Frame` 的纯函数**：top-down 行序、`stride`、字节序（BGRA ↔ `Xrgb8888`）、
   几何溢出/截断拒绝；
2. **落点量化**：非整数/NaN/超范围逻辑坐标 → 取整规则与拒绝路径（对齐 X11 的
   `validate_marker_geometry` 模式）；
3. **`BitBlt` 失败必须 `Err`，不得返回黑帧**：用注入的假 sys 层断言错误而非"全黑成功"；
4. **"已显示"必须同步**：用假 sys 层断言 show 路径**确实调用了同步原语**——
   这正是 40% 命中那个坑的回归测试（把同步调用删掉，测试必须红）；
5. **投影命令序列**：show 替换旧集合（先清后建）、clear 幂等、`Drop` 回收、
   外来线程调用不产生残留；
6. **能力 → `EnvironmentContext` → `ProbeGate`**：windows 语境下 L9 必须
   `MissingProtocol`、L0 必须 `Available`；`screen_capture_permission` 失败 →
   两者都 `PermissionRequired`（复现 X11 的连接期诚实拒绝模式）。

验收：`cargo test -p fidus-backend-windows -p fidus-core` 通过 + 真机探针复跑 + Linux 侧
`cargo metadata --filter-platform` 与（若可行）交叉 `cargo check`，并把真实输出贴进
`docs/measurements/` 与审查记录。

## 8. 复现（核心假设）

```bash
# 需要 Windows + MSVC 工具链；探针是独立 workspace，不进主 workspace 的离线门禁
export CARGO_HOME="$PWD/.toolchain/cargo" RUSTUP_HOME="$PWD/.toolchain/rustup"
export PATH="$CARGO_HOME/bin:$PATH" TMP="$PWD/.toolchain/tmp" TEMP="$PWD/.toolchain/tmp"
cargo build --release --manifest-path tools/winprobe/Cargo.toml
./tools/winprobe/target/release/winprobe.exe | tee docs/measurements/windows-backend-primitives.txt
```

探针通过**真实** `CalibrationIo`/`CaptureIo` 与**真实** `AnchorCalibrator` 测量，
不是仿制品；它只闪标记、不注入输入、不改显示设置，退出路径全部销毁窗口。
Windows 上不要加 `--offline --config .cargo/config.ci.toml`：`vendor/` 在 Windows 检出时
被改写为 CRLF，`thiserror` 的校验和不匹配（详情见 `tools/winprobe/README.md`）。

## 9. 实现与验证记录（获批后执行）

**状态：已实现并实机验证；仍有 3 项门禁未在本机执行（下方逐条列出，不得当作通过）。**

### 9.1 产物

| 文件 | 内容 | 实际规模 |
|---|---|---|
| `crates/fidus-backend-windows/src/api.rs` | 平台中立：`Platform` trait、`MarkerSpec`/`RawFrame`/`ProbeFacts`、能力→`EnvironmentContext` 映射 | 224 行 |
| `crates/fidus-backend-windows/src/plan.rs` | 落点量化/校验 + `RawFrame → Frame` 校验转换（纯函数） | 114 行 |
| `crates/fidus-backend-windows/src/projector.rs` | owner 线程 + 命令通道 + 幂等 teardown（D5） | 190 行 |
| `crates/fidus-backend-windows/src/sys.rs` | Win32：DPI、工作区、分层窗口、DIB/BitBlt、`DwmFlush`、残留窗口计数、命中测试 | 475 行 |
| `crates/fidus-backend-windows/src/backend.rs` | `WindowsBackend` + 两个 session + `IoFactory` | 207 行 |
| `crates/fidus-backend-windows/src/lib.rs` | crate 文档（含"为什么是这两个原语"的实测依据）+ 模块声明 | 60 行 |
| `crates/fidus-backend-windows/tests/sim.rs` | **15 个无显示测试**（在任何平台跑，含 Linux CI） | 450 行 |
| `crates/fidus/src/bin/fidus-windows-probe.rs` | 常驻实机诊断（驱动已发货后端 + 真实 Anchor + 真实 gate） | 415 行 |
| 接线 | workspace members / `workspace.dependencies` / `fidus` feature+bin / `builder.rs` / `lib.rs` / `CompositorKind::Windows` | 11 个文件 +114/−5 行 |

新增 Rust 共 **2135 行**（含 `Cargo.toml` 则 2159 行）；另加测量工具 `tools/winprobe`
**1353 行**（含 README 与 lockfile，独立 workspace、不进 CI 门禁）。
行数为**文件总行数**（含空行与注释）。
逐项交付说明见 [`docs/windows-backend-delivery.md`](../windows-backend-delivery.md)。

无显示测试覆盖提案 §7 的六项：投影命令序列（**同步调用缺失即变红**——对应 40% 命中那个坑）、
替换语义、清空幂等、`Drop` 回收、外来线程驱动、批内失败的 fail-closed、落点量化与拒绝、
`RawFrame` 校验、能力→gate（L9 缺协议 / L0 可用 / 权限被撤 / 无显示器 / DPI 诚实上报）。
在本机 Windows 上：`cargo test -p fidus-backend-windows` **15/15 通过**；
`cargo test -p fidus-core -p fidus-calibrate` 全部通过（含 `fidus-core/tests/gate.rs` 等）。

### 9.2 实机验证（已发货后端）

原始输出 [`docs/measurements/windows-backend-live-probe.txt`](../measurements/windows-backend-live-probe.txt)：
落点 **20/20**、质心/颜色/面积 **20/20**、清除 **20/20**、残差 **0.000 px**、
`WindowFromPoint` 命中标记 **0/4**、teardown 后残留窗口 **0**、真实 `AnchorCalibrator`
解出**单位仿射**（rms/verify/consistency **全 0.000 px**），gate 回答
`Crosshair: MissingProtocol` / `Anchor: Available`。与提案阶段的独立测量工具读数对照表见
[实测记录](../measurements/windows-backend-primitives.md#实现后的复现常驻诊断驱动的是同一个后端)。

### 9.3 与计划的偏离（连同理由）

| 计划 | 实际 | 理由 |
|---|---|---|
| **D9**：wayland/x11 依赖移入 `[target.'cfg(unix)'.dependencies]`，三者都进 `default` | 三个后端保持**普通 optional 依赖**，`windows` 仅 opt-in | 实测 `wayland-sys` 在 Windows 上编译不过（`use std::os::unix::io::RawFd`），且 `fidus-probe-marker` 的 `required-features = ["wayland-layer"]` 一旦进 default 就会在 Windows 上被构建——Cargo 无法表达"按平台启用默认 feature"或"按平台跳过 bin"。选定的接线让 **Linux 清单与改动前几乎逐字节相同**（只多一个 optional 依赖、一个 feature、一个 bin），`Cargo.lock` 变更为**纯新增 10 行**（`git diff --stat` 可查），`cargo metadata --locked --offline` 在 Windows 上通过。代价：Windows 用户要写 `--no-default-features --features windows`（README 已写明） |
| §7：把 `tools/winprobe` 迁成常驻诊断 | 常驻诊断**新增**，`tools/winprobe` **保留** | 常驻诊断只走已发货配置，无法重测**被否决的备选**（`PrintWindow`/非分层/无同步）。保留测量工具才能让"为什么是这两个原语"的调查可复现；它已明确标注为测量工具、独立 workspace、不进 CI 门禁 |
| §4 D3：复用截屏缓冲 | **未实现**，每次分配一张 8 MB DIB | 实测中位 32.5 ms（n=40），估计器节奏 ~500 ms，占比约 6%；复用要多一份会残留陈旧像素的状态。取舍与数字写进 README「已知限制」，不是遗漏 |

### 9.4 门禁记录

```text
review_level: full
reviewer: 主代理（实现者自审，非批准）
input_revision: 工作树（HEAD 51c0069 + 本提案的改动），无 commit、无 push
decision: conditional
skipped_checks:
  - cargo clippy -- -D warnings：本机 rustup 未装 clippy 组件，且本地代理在过程中掉线，
    组件下载反复失败（static.rust-lang.org / rsproxy.cn 均 "Could not connect to server"），
    未执行。新代码按 clippy 默认 lint 逐项收紧（Win32::new → connect 以避免 new_without_default、
    BITMAPINFO 显式构造以避免 field_reassign_with_default、回复通道提取类型别名），但**这是推断不是读数**
  - Linux 侧 cargo test/clippy/doc --workspace（离线 vendor 门禁）：本机无 Linux 目标 std，
    且 vendor 树在 Windows 检出时被 CRLF 改写导致校验和不匹配（thiserror 一例），无法在本机复现
  - cargo metadata --filter-platform x86_64-unknown-linux-gnu：需要 linux-raw-sys，代理掉线后取不到
  - 非 100% 缩放、真实鼠标点击穿透、多显示器/混合 DPI、运行中 DPI 变更、HDR、
    独占全屏/UAC 安全桌面/远程会话（见实测记录「未测量」表）
findings: 见 9.3 与 9.5；另实测修正了两处初始假设（非分层窗口不满足穿透、PrintWindow 不可用），
  并证伪了一条机制假设（穿透不来自 WM_NCHITTEST 的 HTTRANSPARENT）。全量审查三问另剪掉两处：
  替换投射集合时无谓的第二次 DwmFlush、以及 destroy 失败后丢失句柄（后者威胁"无残留窗口"）。
next_step: 需要人工审查决定是否合并；合并前建议在 Linux 上跑一次完整离线门禁
  （cargo test/clippy/doc --workspace + scripts/ci_fidus_test.sh），以及在缩放 ≠ 100% 的
  Windows 机器上重跑 fidus-windows-probe
```

### 9.5 全量审查三问（AGENTS 禁忌 6）

**问一：是否仍有更优雅替代？**

* 平台层用 `windows-sys` 官方绑定而不是手写 `extern`：它已在 `vendor/` 与 `Cargo.lock` 里,
  因此不触发 `cargo vendor` 重建（那是 96 MB 仓库里的派生输入），也省掉 ABI 自证。
* 同步方式三选一（`DwmFlush` / 仅 `CAPTUREBLT` / 仅分层提交）实测都能到 20/20；保留
  `DwmFlush` **加** `CAPTUREBLT` 是一对冗余，理由是"已显示"是契约里最脆的一条（无同步实测
  8/20）。**若评审认为该剪，剪任意一个都只少一处成本，但保证从双点变单点**——这是有意的、
  明码标价的冗余，不是顺手多写的一行。
* 线程模型：owner 线程 vs 手工 `unsafe impl Send`；后者在跨线程 `Drop` 时会静默泄漏窗口
  （原则五），已被否决。

**问二：是否仍有可继续剪掉的不合理设计？** 本轮审查剪掉两处：

1. 替换投射集合时**多一次无谓的 `DwmFlush`**——旧集合的移除与新标记的显示被同一次合成覆盖，
   中间态无人观测。已合并为一次（回归测试断言命令序列里只有一个 `sync`）。
2. `destroy_marker` 失败时**句柄被丢弃**（`drain` 之后不再跟踪），使"拒绝死的窗口"永远无法重试，
   直接威胁"无残留窗口"。已改为把失败句柄放回跟踪表，并补回归测试
   `a_window_that_refused_to_die_is_retried_rather_than_forgotten`。

另有意识地**未**加入：截屏缓冲复用（见 9.3 的成本对账）、`CompositorKind::Windows` 之外的
core 改动、任何新的第三方依赖。

**问三：是否仍有逻辑问题？** 逐项复核后**未发现**新的逻辑问题。复核范围：
落点量化只有一处取整（`plan_marker`）且与校准器的 `round` 一致；`RawFrame` 校验失败一律 `Err`，
不存在"黑帧当成功"的路径；`clear_marker`/`destroy_projector` 幂等，且在空集合上不产生任何平台调用；
`Drop` 路径 join owner 线程后窗口必已销毁；`sync_presentation` 对 `DWM_E_COMPOSITIONDISABLED`
的处理有明确理由（没有合成就没有可错过的呈现）；能力映射里 L9 因缺 layer shell 被
`MissingProtocol` **提前**拒绝，不会误导调用方去申请一个帮不上忙的截屏权限。

---

## 10. 审查记录

```text
review_level: quick
reviewer: （待人工）
input_revision: 提案初稿 + tools/winprobe + docs/measurements/windows-backend-primitives.md
decision: （待定）
skipped_checks: 非 100% 缩放实测、真实鼠标点击、多显示器、实现与测试（提案阶段未实现）
findings: （待填）
next_step: 提案 → 自审裁枝 → 草案 → 方案
```

**提案可以得出"不做"的结论，这正是它的价值**（`docs/proposals/README.md`）：
若否决策略是"跨平台语义统一"，请直接否决——被否决的代价是这份提案与一个探针，
放它进实现的代价是一个后端加一条永久维护线。
