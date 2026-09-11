# fidus · 零信任坐标定位引擎

> *当窗口系统拒绝开口，你就自己把坐标系画在墙上。*
>
> 规格文档：[`fidus_positioning_engine_v0.5.1.md`](./fidus_positioning_engine_v0.5.1.md)（架构冻结候选）
> · 方法论：[v0.4 草案](./fidus%20定位引擎（草案%20v0.4）.md)
> · Niri 实机验证：[点击穿透实现路径](./有关メア桌宠的Niri上的点击穿透实现路径.md)

fidus 是一个纯 Rust 定位库：在平台窗口坐标 API 不可信的环境下，它**不封装任何原生坐标 API**，而是用最基础的合成器原语（`wl_surface` / `wl_shm` / layer-shell、通用截屏）投射自己的标记、截取自己的屏幕、解算自己的坐标系。**定位什么由调用方决定；fidus 提供地图。**

零信任**不是一句口号，也不是主动挑选的清高姿态——它是被平台 API 活活坑出来的、被迫且唯一的选择**。下一节是实机取证。

## 为什么零信任是被迫的：实机取证

以下全部可在本机复现（Niri 26.04 / wayland-1 / eDP-1 1366×768）。结论先行：**在 Wayland 下，"我的窗口在屏幕哪里"这个问题在协议层面根本没有提问的入口**——不是 API 返回了错值，是压根没有这个 API。

### 取证一 · 扫遍全系统 42 份协议 XML，没有一个接口回答"我在哪"

```bash
find /usr/share/{wayland,wayland-protocols,wlr-protocols} -name '*.xml' | wc -l   # 42
```

把 42 份 XML 里所有带 `x`/`y` 参数的 request/event 全部列出来，逐条归类后只剩三类，**没有第四类**：

| 类别 | 例子 | 为什么没用 |
|---|---|---|
| **我们告诉合成器**（request） | `wl_surface.damage` / `wl_region.add` / `xdg_positioner.set_offset` / `zwlr_layer_surface_v1.set_margin` | 方向反了。这是我们**说**，不是它**答** |
| **相对某个 surface 的坐标**（event） | `wl_pointer.motion`、`wl_touch.motion` | 协议原文：*"the location relative to the focused surface"*——相对量，没有原点 |
| **输出（显示器）的位置**（event） | `wl_output.geometry` 的 `x`/`y` | 协议原文：*"x position within the global compositor space"*——是**显示器**在全局空间的位置，不是**窗口**的 |

### 取证二 · `xdg_toplevel.configure` 只给尺寸，不给位置

普通窗口（`xdg-shell`）拿到的全部事件：

```
xdg_surface.configure           args = [serial]
xdg_toplevel.configure          args = [width, height, states]     ← 无 x/y
xdg_toplevel.close              args = []
xdg_toplevel.configure_bounds   args = [width, height]             ← 还是无 x/y
xdg_toplevel.wm_capabilities    args = [capabilities]
```

这就是 Qt 的 `geometry()` / `win.xChanged` 在 Wayland 下恒返回 `(0,0)` 的**协议级根因**：Qt 无处可取，只能填 0。**这不是 Qt 的 bug，是它诚实地表达了"无可奉告"**——而调用方看到的是一个看起来完全合法的 `(0, 0)`，`success` 为真。这正是原则 1 要严防死守的那个"看起来合理的值"。

### 取证三 · layer-shell 同样只给尺寸——连我们自己放的 surface 都读不回位置

```
zwlr_layer_surface_v1 requests (我们能说): set_size, set_anchor, set_exclusive_zone,
                                           set_margin, set_keyboard_interactivity, ...
zwlr_layer_surface_v1 events   (它回答我们): configure(serial, width, height), closed()
```

我们能用 `set_margin` 把 surface 放到指定位置，却**没有任何事件告诉我们它最终被放在了哪**——usable-area 被面板/独占区吃掉多少、分数缩放怎么取整，全部不可见。这就是 fidus 必须"投射 + 截屏 + 解仿射"的直接原因：**唯一能读回位置的通道是眼睛**。

（对应规格 §4.4 的 Q9 教训：Wayland 客户端无法查询自身全局坐标，所以坐标必须在 `destroy` 之前从视觉测量中读出并缓存。）

### 取证四 · 合成器自己完全知道，就是不通过协议告诉你

```bash
niri msg windows
```

```
Window ID 2:
  Title: "localsend"
  Layout:
    Tile size: 659 x 736
    Window size: 659 x 736
    Window offset in tile: 0 x 0
```

合成器的内部状态里，每个窗口的位置一清二楚。它通过自己的 IPC 讲得明明白白，**但 Wayland 协议这一侧一个字都不说**。而 fidus 不能去读 `niri msg`——那是绑定单一合成器的私有 IPC，违反原则 3（只依赖最普适的原语），且换个合成器立刻失效。

连**特权**协议也一样守口如瓶。`zwlr_foreign_toplevel_management_v1` 能列出系统里所有窗口，它给的事件是：

```
zwlr_foreign_toplevel_handle_v1 events:
  title, app_id, output_enter, output_leave, state, done, closed, parent
```

标题、app_id、在哪个输出上、最大化没有——**唯独没有几何**。这是设计意图，不是遗漏：Wayland 把窗口位置定义为合成器的私有事务。

### 取证五 · X11 回退路径在 XWayland 下同样断掉

"那退回 X11 API 不行吗"——在 Wayland 会话里，XWayland 是 rootless 的：

```bash
$ xwininfo -root -tree
  0x3fe (the root window)  1366x768
     7 children:
     0x800001 "xsettingsd"           1x1+-1+-1
     0x600003 "Fcitx5 Input Window"  30x33+266+186
     0x600001 (has no name)          1x1+0+0
     ...
$ xdotool search --name "." | wc -l
2
```

屏幕上明明开着若干 Wayland 原生窗口，**X11 的窗口树里一个都看不到**——只有 7 个 1×1 的输入法/设置辅助窗口。`xdotool` 数得出 2 个。X11 那套 `XQueryTree` / `xdotool getwindowgeometry` 在这里不是"返回错值"，而是**目标根本不在它的世界里**。

更进一步，rootless XWayland 连根窗口的画面都没有：

```bash
$ FIDUS_BACKEND=x11 cargo run -p fidus --bin fidus-calibrate
[stage 1] backend init failed (X11): environment lacks required primitives:
  root window is not capturable (rootless XWayland?): X11Error { error_kind: Match,
  major_opcode: 73, request_name: Some("GetImage") }
```

`GetImage(root)` → `BadMatch`。fidus 在**连接时**就探到并诚实拒绝（`screen_capture_permission = Revoked`），而不是校准到一半才失败。

### 这五条取证推出的唯一结论

平台 API 不是"有时不准，兜底一下就好"——**它在这个环境里连提问的入口都不存在，且在 Qt 那一层被伪装成了一个 `success` 的 `(0,0)`**。于是：

- "先走正道、失败再邪修"的二分法**不成立**（规格 §2.2）。判断"这次的 `(0,0)` 是真在原点还是无可奉告"本身就需要一个可信坐标源——循环论证。
- 布尔型的"API 成功"和概率型的"视觉置信度"**量纲不可比**，融合即污染（原则 4）。
- 所以零信任是**唯一**架构，不是降级路径。相应地，**能用类型封死的，绝不靠约定和注释封**——见 [`AGENTS.md`](./AGENTS.md) §0。

## 状态矩阵

| 里程碑 | 内容 | 状态 |
|---|---|---|
| **P0** | 架构地基：类型系统、三层 trait、Gate、概率池纯净性 | ✅ 完成 |
| **P1** | L9 Crosshair 校准器 + teardown 生命周期 | ✅ 完成，Niri 实机验证通过 |
| **P2** | C 层增量追踪：L1 Fingerprint + 运行时 target 注册 ✅（P2-a）→ L8 EdgeSync + MotionGate ✅（P2-b）→ L4/L7 融合（常速度 KF）✅（P2-c）→ L0 Anchor 通用兜底校准器 + X11 backend ✅（P2-d） | ✅ 完成 |
| **P3** | L10 GradientField（feature-gated 实验项） | ⬜ 研究项，未实现 |

当前可在 Niri / Sway / Hyprland / KDE Plasma 等支持 `zwlr_layer_shell_v1` + `zwlr_screencopy_manager_v1` 的 Wayland 合成器上以 L9 校准，在真 X server 上以 L0 校准（后端由 `FidusBuilder` 自动选择：layer-shell 优先，其次 X11），并可注册调用方自己的渲染模板做稳态追踪：L1 模板匹配 + L8 门控差分融合进常速度 Kalman 跟踪器（L7，P2-c）——拖拽跟随、动画期滑行（置信度 0 的标注信念而非伪造测量）、丢失后重捕获。Windows、macOS、GNOME（portal 路径）的 backend 尚未实现。

> **fidus 是全平台统一兜底库，不是 Wayland 专用工具。**
>
> 上一节的取证说明零信任是被 Wayland **逼**出来的；但"起因是 Wayland"不等于"只服务 Wayland"。Wayland 是**最极端的一个实例**——极端到连提问的入口都没有——而不是唯一的一个：
>
> | 平台 | 坐标 API 的实际状态 |
> |---|---|
> | Wayland | 协议层面不存在窗口位置事件；Qt 侧被伪装成 `success` + `(0,0)` |
> | X11 | 本身可信，但 XWayland 下窗口树里根本没有 Wayland 原生窗口（取证五） |
> | Windows | `GetWindowRect` 可用，但多屏混合 DPI 下返回值与实际像素不一致 |
> | macOS | 沙盒限制，私有 API 随系统更新失效；屏幕录制权限变更后需重启进程 |
>
> 一个"各平台各写一套 + 维护每个合成器可信度黑名单"的方案是无底洞。fidus 的选择是**一套机制覆盖所有平台**：投射自己的标记、截自己的屏、解自己的仿射——这套机制在 Windows 和 macOS 上同样成立，因为它只需要"能画一个方块"和"能截一张图"。
>
> L0 Anchor 因此被设计为只依赖"投射 + 截屏"两个原语的**通用**校准器：Gate 按**能力**（能否同时投射多个标记）而非平台身份决定它是否可用。Windows / macOS 缺的只是 backend（各约 300 行原语适配），校准器与估计器一行不用改。

## 五条设计原则（附录 A，一切功能的优先级高于功能本身）

1. **Trust no platform API.** 不信任任何平台坐标 API——公开 API 在类型层面不存在"坐标输入"这一入口。
2. **Build your own map.** 自带测绘工具：投射已知标记 → 截屏检测 → 解算映射。
3. **Bootstrap from primitives.** 只依赖最基础、最普适的合成器原语。
4. **Keep the pool pure.** 概率池只接收视觉与交互测量；布尔型的"API 成功"与概率型的"视觉置信度"量纲不同，永不混算。
5. **Calibrate, then get out.** 校准完成立即销毁 overlay surface，不常驻（所有退出路径均强制 teardown）。

## workspace 结构（规格 §7）

```
crates/
├── fidus-core/                  类型系统 + 三层 trait + 概率池规则 + 坐标空间
│                                  coord.rs      LogicalPoint/PhysicalPoint/AffineTransform
│                                  frame.rs      CoordinateFrame（校准产物，C 层的输入）
│                                  gate.rs       Gate trait + ProbeGate（§5 能力探测）
│                                  io.rs         CalibrationIo 会话（仅有的两个原语）
│                                  engine.rs     Calibrator/Estimator trait + FallbackEngine
├── fidus-backend-wayland-layer/ 仅适配基础原语：layer-shell 投影 + wlr-screencopy 截屏
├── fidus-backend-x11/           仅适配基础原语：override-redirect 窗口投影（每标记一窗）+ GetImage(root) 截屏
├── fidus-calibrate/             L9 Crosshair + L0 Anchor 校准器；共享 detect.rs 差分检测器（L10 占位）
├── fidus-estimate/              C 层估计器：L1 Fingerprint + L8 EdgeSync/MotionGate + L4/L7 融合（常速度 KF）+ ScreenClassifier
└── fidus/                       伞 crate：FidusBuilder 后端选择（Auto/WaylandLayer/X11）+ 冒烟测试二进制
```

`fidus-backend-*` 只适配"画标记、截屏幕"的原语，**永不**绑定任何报告窗口几何的协议——这是零信任在 backend 层的落地。X11 backend 的 override-redirect 窗口是 fidus **自己**创建、放在**自己**选的坐标上的，与 layer-shell margin 同一性质。

## 坐标语义

校准成功后产出 `CoordinateFrame`：一条**逻辑空间 ↔ 物理空间**的仿射映射。

- **逻辑空间**：被校准输出的 layer-shell usable-area 坐标——与 layer-shell margin 同一坐标系，桌宠拿到后可直接喂给 `overlay_set_position()`。
- **物理空间**：fidus 自己截屏里的像素坐标（Y 反转已由 backend 归一化）。

两个侧面都是 fidus 自产量：逻辑侧锚定在 fidus 自己选的标记 margin 上，物理侧锚定在 fidus 自己截到的像素上。仿射求解天然吸收合成器藏起来的一切——usable-area 偏移（面板/独占区）、分数缩放、输出旋转变换——全程没有读取过一个平台坐标。

## L9 校准协议（本仓库的实现细化）

1. **可用区探针**：overlay surface 以"四边锚定 + 尺寸 0"创建，configure 事件宣告 usable-area 尺寸——**仅作标记布点的提示**，永不进入概率池；提示错了的后果只是标记出屏、检测不到、自动重试，自我纠正。
2. **基线帧**：标记隐藏时截屏。
3. **标记序列**：两轮独立 pass，各采样 4 个抖动、乱序的 usable-area 角点（margin 预先量化到整数逻辑像素，与 backend 的 round 严格一致）。
4. **检测**：基线差分 + 连通域。overlay 永远置顶且由 fidus 控制，因此**差分把标记从任意背景中隔离**——动态壁纸最多让单次测量失效（歧义 → 重试），无法伪造。面积门控（先验 = 首个检出标记的面积）滤掉壁纸漂移与光标小块；对应点取标记 bbox 的**几何中心**（排他边界），对光标穿透标记造成的空洞不敏感，也无像素索引质心的 −0.5px 约定偏差。
5. **求解**：≥4 组对应点做 2×3 仿射最小二乘（支持旋转输出）；残差、线性尺度 sanity 门控。
6. **验证**：3 个全新位置先预测后检测，误差 > 1.5px 即拒绝。
7. **一致性**：两轮 pass 的映射在 5 个探针点上偏差 > 1.5px 即拒绝（对应 §4.3"至少 2 轮独立采样验证映射一致性"）。
8. **Teardown**：`destroy_projector` 在成功、失败、Drop 三条路径上全部执行（§4.4）。

## L0 校准协议（通用兜底，规格 §4.3 的加固版）

L9 有"置顶 layer surface"的保证，L0 只有"几个我们自己建的窗口"——检测器不能假设哨兵是屏幕上最大的变化。因此 L0 叠了四道彼此独立的防线：

1. **基线差分 ∧ 颜色**：哨兵 = *相对隐藏基线发生变化* **且** *带哨兵色* 的像素区域。壁纸里碰巧有哨兵色的色块在两帧中相同，被差分抵消；只有在基线与捕获**之间**变化的东西才可能干扰，而那会以歧义/未检出的形式进入重试。
2. **颜色→角点随机打乱**（每次尝试重洗）：移动的诱饵每次要伪装的角点都不同。
3. **矩形约束**：四个角的位置每轮按"整条边"抖动（仍是轴对齐矩形），仿射保持平行与中点，所以检出的四个中心必须构成等对角线的平行四边形——某一角被诱饵顶替会剧烈破坏它。
4. **验证轮 + 双 pass 一致性**：解出映射后先预测再检测 2 个全新内部位置（> 1.5px 拒绝），两轮独立 pass 在 5 个探针点偏差 > 1.5px 拒绝。

四个哨兵**同时**投射、单次捕获——真 X server 上整轮校准约 60ms。对应点同样取 bbox 几何中心。

**rootless XWayland 的诚实拒绝**：Wayland 合成器下的 XWayland 没有根窗口画面，`GetImage(root)` 返回 `BadMatch`。X11 backend 在连接时探测一次截屏；失败则把 `screen_capture_permission` 报为 `Revoked`，Gate 拒绝、`FidusBuilder::Auto` 落到下一个后端——不会校准到一半才失败。

## 快速开始

实机冒烟测试（Wayland 下屏幕上会闪现约两秒的品红色标记方块；X11 下四角闪现四色哨兵）：

```bash
cargo run --release -p fidus --bin fidus-calibrate
# 强制后端 / 偏好校准器（Gate 仍有最终决定权）：
FIDUS_BACKEND=x11 FIDUS_METHOD=anchor cargo run --release -p fidus --bin fidus-calibrate
```

四个阶段分别验证：后端连接 + 环境探测 → 所有方法的 Gate 回答 → 完整校准 → teardown（`niri msg layers | grep fidus` / `xwininfo -root -tree` 应无 fidus 窗口）。

作为库使用：

```rust
use fidus::prelude::*;

// 后端自动选择：layer-shell 优先，其次 X11（也可 build_with(BackendChoice::X11)）
let mut engine = FidusBuilder::new().build()?;

// 1. 先查询，再决定要不要校准（§5.4）；构建时已按 Gate 选好校准器（L9 → L0）
let usable = CalibrationMethod::ALL
    .iter()
    .any(|m| engine.gate().query_calibrator_availability(*m).is_usable());
if !usable { eprintln!("此环境无可用校准器"); return Ok(()); }

// 2. 校准一次；返回时 overlay / 标记窗口已销毁
let frame = engine.calibrate()?;
println!("scale {:.3}, rms {:.3}px",
    frame.map().linear_scale(), frame.quality().rms_residual_px);

// 3. 注册追踪目标：调用方自己的离屏渲染（纯像素，fidus 不收任何平台坐标）
let target = TargetDescription {
    template_logical: RgbaImage::from_raw(4, 4, vec![0; 4 * 4 * 4]),
    initial_center: None,
};
engine.register_target(target).expect("estimator accepts targets");

// 4. 稳态追踪（L7 融合：L1 模板匹配 + L8 门控差分 → 常速度 Kalman）
let _ = engine.estimate();
```

## C 层估计管线（P2）

1. **L1 Fingerprint**：调用方模板（逻辑像素渲染）按校准 scale 重采样 → NCC 粗到细扫描（预测位置播种）+ 亚像素峰拟合。
2. **L8 EdgeSync + MotionGate**：500ms 窗口帧差分；目标区域变化率 R > max(baseline×3, 15%) 即 DYNAMIC 丢弃、连续 2 帧通过才恢复（§6.1）。基线非对称学习（慢升快降）避免稳态噪声卡死门限。有据细化：**已模板验证的位移 blob 即使 R 尖峰也放行**（尖峰由离场解释，到达内容经模板验证——比统计门更强的检查）。
3. **L7 融合**：L1+L8 测量按置信度融合（互相印证加成）→ 常速度 Kalman（L4 运动模型）。首修/重捕获时**硬重置**（陈旧速度不得抹开跳变）；双盲帧**滑行**——按速度外推、置信度 0，明确标注是信念而非测量（§4.5 池内不进伪造值）。

## 测试

```bash
cargo test     # 88 个测试：数学、Gate、检测器、L9/L0 端到端仿真、L1/L8/L7 估计器
               #   含 2 个 compile_fail doctest，确保坐标注入入口在类型层面不存在
cargo clippy   # 零警告
```

其中 `fidus-calibrate` 的**仿真测试**在无显示服务器的环境下模拟整个合成器行为。`tests/sim.rs`（L9）：分数缩放 1.25 + 面板偏移的精确恢复、动态壁纸噪声下收敛、光标穿透标记后 bbox 中心不变、双标记歧义拒绝、盲投影/小屏幕失败路径的 teardown。`tests/anchor_sim.rs`（L0）：1.5× 缩放 + 原点偏移恢复、**静态哨兵色壁纸无害**（差分抵消）、**动画哨兵色诱饵必须拒绝而非吸收**、单 surface 后端干净拒绝。实机上踩过的每个坑都固化为回归测试。

## 已知限制（对应规格 §10 开放问题）

- **单输出**：多显示器时 Gate 返回 `Degraded`，只校准第一个输出（§10.4 未定案前的诚实降级）。
- 动态壁纸探测（P2-e 已落地）：`FidusBuilder::build` 默认跑一次 `ScreenClassifier`（3 对相隔 200ms 的截屏、中心 84% 区域、步长 4 采样，取**最差**一对；> 15% 判动态）自动填充 `is_dynamic_wallpaper`，调用方显式提供时跳过；Gate 据此把 L9 降为 `Degraded{0.85}`（差分检测对动态背景鲁棒，代价是重试更多而非精度更低）、L0 降为 `Degraded{0.6}`。实机读数：静态桌面 ~0.0001–0.0005，全屏动画 0.22–0.29，局部动画 0.08–0.17（正是"取最差一对"的理由）。它分不清"壁纸在动"和"窗口里在播视频"——但 Gate 问的是"基线与捕获之间背景会不会变"，两者答案相同，所以不区分。
- **L0 在 rootless XWayland 上不可用**（取证五：`GetImage(root)` → `BadMatch`，连接时即诚实拒绝）。
- **Windows / macOS 后端未实现**：这是**缺 backend，不是缺设计**。L0 Anchor 校准器、Gate 的能力判定、整条 C 层估计管线都与平台无关，需要的只是各约 300 行的"投射 + 截屏"原语适配（Windows: 分层窗口 + BitBlt；macOS: `NSPanel` + `CGWindowListCreateImage`，另需屏幕录制权限状态机 —— `PermissionState` 已预留 `RequiresRestart`，因为 macOS 授权后进程必须重启）。
- **L10 GradientField**：Gate 诚实返回 `FeatureDisabled`。
- **GNOME（无 layer-shell）**：需要 `fidus-backend-wayland-portal`，未实现。
- **Y_INVERT**：screencopy 的 Y 反转已处理并有单测，但仅在实机（Niri 不置位该标志）验证过非反转路径。

## 许可

MIT OR Apache-2.0
