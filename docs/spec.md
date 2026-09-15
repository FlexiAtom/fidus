# fidus 定位引擎 · 草案 v0.5.1

> **零信任坐标 · 当窗口系统拒绝开口，你就自己把坐标系画在墙上**
>
> **范围定义：fidus 只定位调用方自身窗口。** 调用方提供自身窗口的离屏渲染模板，fidus 通过投射、截屏和视觉匹配返回该窗口可见内容的位置；不枚举、识别或定位任意第三方系统窗口，不提供窗口实例身份，也不承诺在目标完全不可见时恢复其几何位置。
>
> 状态：架构冻结候选（Architecture Freeze Candidate）
> 更新日期：2026-09-04
> 继承：v0.4（方法论）→ v0.5（校准器重构）→ **v0.5.1（精神内核 + 概率池纯净性）**

---

## 0. TL;DR · 五个决定性转变

v0.5.1 相对 v0.4 的全部修改，浓缩为一张表：

| # | v0.4（旧） | v0.5.1（新） | 来源 |
|---|---|---|---|
| 1 | L9/L10 作为持续追踪器常驻 | **初始化 + 定期校准，校准后立刻销毁 overlay** | 性能/视觉痛点化解 |
| 2 | Locator 单层枚举（A/B/C 混在一起） | **三层分离，A 层真值不进滤波器** | 数学正确性 |
| 3 | 宣称"全平台通用，L9/L10 终局" | **仅在 layer-shell 可用时启用 L9/L10，覆盖范围诚实化** | Niri 实机文档 |
| 4 | A 层原生 API 作为 P0 核心 | **fidus 不封装任何原生 API**（详见 §1） | 概率池纯净性 |
| 5 | 业务项混在核心 | **L2.5 删除；L3/L5 移出核心（归规划中的 `fidus-extras`）** | 职责清晰 |

---

## 1. 核心精神：Zero-Trust Coordinate（零信任坐标）

> **fidus 的立身之本，一句话：**
> *独立自主，自力更生*

### 1.1 五条不可动摇的原则

fidus 的独立性**不是可选的风格偏好，而是被平台 API 坑出来的「被迫且唯一」的选择**——§2.1 的实测矩阵就是证据：不存在一个可信的窗口坐标 API 供我们「优先尝试」。

> **编号即附录 A**。本节是附录 A 五条的展开，全项目（含 README、AGENTS.md、代码注释）引用的「原则 N」一律指这套编号。
> *历史注记*：v0.5.1 早期版本此处另立过一套「三条铁律」，与附录 A 编号冲突（如「原则三」在两套里含义不同），已于 P2-g 统一。

**原则一 · Trust no platform API（零信任）**
> 不信任任何平台 API 返回的坐标值。即使 API 返回 `success`，即使它看起来合理——fidus 不以任何外部坐标作为自身状态的一部分（严防死守某些看起来合理的值，如 `0,0`）。

**原则二 · Build your own map（自建地图）**
> fidus 自带测绘工具，从零建立坐标系：投射自己的标记 → 截自己的图 → 解自己的仿射变换。坐标系的每一个系数都可追溯到 fidus 自己截图中检出的标记质心。

**原则三 · Bootstrap from primitives（自举于原语）**
> 全部坐标能力仅依赖**全平台最基础、最普适的合成器原语**（`wl_surface` / `wl_shm` / layer-shell、X11 基本绘制、Windows GDI、通用截屏）。不依赖任何窗口管理器的"配合"或"恩赐"。后端只适配「投射 + 截屏」两件事。

**原则四 · Keep the pool pure（概率池纯净）**
> 概率池只接收视觉与交互测量。任何平台 API 返回值**在类型层面**无法进入（§6.2）。"纯视觉"是必要条件而非充分条件——不可定位的测量与伪造无异（§6.1、§12.1）。

**原则五 · Calibrate, then get out（校准完即撤）**
> 校准完成立即销毁 overlay，不常驻。fidus 是测绘队，不是房客（§4.4）。

> **自举性推论**：在完全没有原生坐标 API 的环境中，fidus 精度依然可与有 API 的环境媲美——因为它建立的是**锚定在屏幕物理边缘的坐标系**，而非依赖 compositor 施舍的"逻辑窗口树坐标"。

### 1.2 一个干净的类比

可以把平台 API 想象成一张**别人掌握开关的地图**：有时给你、有时给你错的（GNOME 下 `win.xChanged` 永远返回 `(0,0)`）、有时干脆不触发。fidus 的选择不是"反复恳求对方给地图"，而是——**自己带测绘工具，从零丈量出整片土地**。

这个类比只服务于一个技术论点：**为什么 fidus 不封装原生 API**。它不延伸、不外推，项目中不出现任何政治化表述。fidus 的精神内核是**技术自主**，用架构原则和代码来保证，而不是口号。

### 1.3 这条精神如何指导每一个决策

| 决策 | 由哪条原则推导 |
|---|---|
| fidus 不封装 `GetWindowRect` / `CGWindowList` / `XQueryTree` | 原则一（零信任）+ 原则三（不做 API 门面） |
| 概率池只接收视觉/交互测量 | 原则一 + 原则四 |
| L9 仅依赖 layer-shell 基础能力；后端只适配「投射 + 截屏」 | 原则三 |
| 校准器建立物理坐标系而非逻辑坐标 | 原则二 |
| `fidus-core` 公开 API 不含任何坐标类 native 输入（`SolvedMap` 封印） | 原则四（类型层面强制，§6.2） |
| 注册期拒绝不可定位的模板（`localizability`） | 原则四（不可定位的测量与伪造无异，§12.1） |
| 校准完成立即 teardown，不常驻 | 原则五 |

---

## 2. 为什么 fidus 必须"不封装原生 API"

### 2.1 实测依据：原生 API 根本不是"真值"

以下来自 Niri/Wayland 环境的实测矩阵（各 compositor × 窗口事件/几何 API 可靠性）：

| API / 事件 | X11 / Windows | GNOME / KDE | Niri | Sway / Hyprland | Weston |
|---|---|---|---|---|---|
| `moveEvent` | ✅ 触发 | ⚠️ 部分 | ⚠️ 受限 | ⚠️ 依赖协议 | — |
| `win.xChanged` | ✅ 真实坐标 | ❌ 永远 `(0,0)` | ⚠️ | ⚠️ | — |
| `self.pos()` | ✅ | ❌ 异常 | ⚠️ | ⚠️ | — |
| `startSystemMove()` | ✅ | ⚠️ 受限 | ⚠️ 依赖协议 | ⚠️ | — |
| `frameGeometry()` | ✅ | ❌ 不触发 | ⚠️ | ⚠️ | — |
| `requestActivate()` | ✅ | ⚠️ | ⚠️ | ⚠️ | — |

**这张表推翻了一个关键假设**：原生 API 并非"100% 可信的真值"。在 Wayland 下，`success` 实测恒为 `(0,0)`，事件可能永不触发，行为随 compositor 碎片化。

### 2.2 "先正道、失败再邪修"的二分法不成立

若 fidus 封装原生 API，内部逻辑会退化为：

```
query():
  if api.available():
    r = api.frameGeometry()   # 可能返回 (0,0) 但 success=true
    if r.confidence 看起来高:
      return r                # ← 把 (0,0) 当高精度真值
    else:
      goto 邪修池             # ← 与视觉估计混算
```

这导致**概率池污染**（详见 §6）：布尔型的"成功/失败"与概率型的"视觉置信度"量纲不同，无法公平融合；且每个 compositor 都要维护一份"这个 API 在此环境是否可信"的黑名单——无底洞，且与 fidus 定位冲突。

### 2.3 结论

**fidus 是纯邪修、零信任坐标系。** 明确不封装任何"正道" API。调用方在 fidus 之外自行决定何时实例化 fidus（详见 §3）。

---

## 3. 架构总览 · A/B/C 三层分离

```
┌──────────────────────────────────────────────────────────────┐
│                     fidus-core（本仓库）                     │
│                                                              │
│  ┌────────────────────┐    ┌──────────────────────────────┐  │
│  │  B · Calibrator    │    │  C · Estimator（概率池）     │  │
│  │                    │    │                              │  │
│  │  L9 Crosshair  ✅  │    │  L1 Fingerprint ─┐           │  │
│  │  L0 Anchor     ✅  │    │  L8 EdgeSync    ─┼▶ L7 融合  │  │
│  │  L10 GradField ❌  │    │  L4 相对位移    ─┘     │     │  │
│  │   （已否决 §4.2）  │    │                        ▼     │  │
│  │                    │    │                 常速度 Kalman│  │
│  └─────────┬──────────┘    └───────────────┬──────────────┘  │
│            │                               │                 │
│            ▼                               ▼                 │
│     CoordinateFrame              ProbabilisticPosition       │
│                                                              │
│  ┌────────────────────────────────────────────────────────┐  │
│  │  Gate · 能力探测（§5）：哪些情况不能用                  │  │
│  └────────────────────────────────────────────────────────┘  │
└──────────────────────────────────────────────────────────────┘

  ★ 明确排除：任何原生 API 门面（Windows / macOS / X11 / Portal）
     → 由调用方在 fidus 之外自行管理（见 §3.4）
```

### 3.1 B 层 · Calibrator（校准器）

**职责**：在初始化 / 定期校准时，**一次性**建立 `CoordinateFrame`。
**特性**：校准完成后**立即销毁 overlay surface**，不常驻。

- **L9 Crosshair**（旗舰）：基于 layer-shell 投射已知位置标记，视觉反推映射
- ~~**L10 GradientField**（实验性）：频域编码全场坐标~~ — **已否决**，原理不成立，见 §4.2
- **L0 Anchor**（通用兜底）：四角哨兵方块（默认 8×8），**基线差分 ∧ 颜色**检测；只要后端能"同时投射多个标记 + 截屏"即可用

### 3.2 C 层 · Estimator（估计器）

**职责**：校准成功后，在稳态下做**增量追踪**，输出带置信度的 `ProbabilisticPosition`。
**输入**：仅视觉与交互测量（见 §6 白名单）。

- **L1 Fingerprint**（模板 NCC 匹配，粗到细 + 亚像素峰拟合）；模板须通过注册期可定位性检查（§6.1.2）
- **L8 EdgeSync**（低频边缘差分，经 MotionGate 门控，§6.1.1）
- **L4 相对位移**（拖动的运动模型）
- **L7 融合**：把 L1 与 L8 的测量按置信度加权融合（互相印证时有加成），送入**常速度 Kalman**（L4 运动模型为其状态转移）。首次定位与重捕获时**硬重置**（陈旧速度不得抹开跳变）；双盲帧按速度**滑行**并标记置信度 0——明确表示"这是信念不是测量"（§4.5）。

> **关于 L6**：v0.4 曾把 "Visual" 与 "Fingerprint" 分为 L1/L6 两层。实现中二者合一，**L6 已并入 L1**，全文与代码统一只用 `L1 Fingerprint`。

### 3.3 Gate · 能力探测

**职责**：回答"当前环境能不能用 fidus / 该走哪条路径"，作为一等公民接口（详见 §5）。

### 3.4 调用方的职责（fidus 之外）

fidus **不是兜底路径，它就是路径**。类型名从 `FallbackEngine` 改为 `FidusEngine`（P2-h）正是为此：旧名字来自"桌宠在正经 API 失败后的最后手段"那个时代，而 §2.1 已经证明**没有那个可退回的正经 API**。

调用方**可以**在自己那侧保留一条原生快速通道，但那是**调用方的选择与调用方的风险**，与 fidus 无关：

```
启动:
  // 可选：调用方自己调 native API，fidus 完全不参与、也不想知道结果
  if 调用方自行判定平台坐标够用:
      用调用方自己的路径              // 风险自负，见下方警告
  else:
      let engine = FidusEngine::new(parts);  // parts 只含能力，不含坐标
      engine.calibrate();
      engine.register_target(...);
      loop { use engine.estimate(); }
```

> **两条路径之间的结果不可混算。** 原生 API 给的是布尔型的"成功/失败"，fidus 给的是概率型的置信度——量纲不同，加权平均没有意义（§2.2、§6.1）。调用方要么用这条，要么用那条，**不要把两者融合**。
>
> **"通常可信"是个陷阱。** 即使在 X11/Windows 上：Windows 多屏混合 DPI 下 `GetWindowRect` 会偏；XWayland 里 X11 路径整条断掉（§2.1 取证五）。调用方若走快速通道，验证责任在调用方自己。

**关键：fidus 的概率池从头到尾只见过视觉/交互信号，从未见过 native 坐标值。污染的入口在类型层面封死。**

---

## 4. B 层详解 · 校准器（初始化 + 定期校准）

### 4.1 L9 Crosshair · 旗舰校准器

**适用**：layer-shell 可用的环境（wlroots 系：Niri / Sway / Hyprland；KDE Plasma；部分 GNOME≥49）。

**原理**：创建一个 `LAYER_OVERLAY` 的 layer surface，通过 `anchor=TOP|LEFT + margin` 精确放置已知偏移的标记，用视觉反馈反推坐标系映射。

**必须遵循的约束**（来自 Niri 实机验证，已确认可用）：

1. **裸 surface**：必须用 `wl_compositor_create_surface()` 自建，**绝不复用 Qt/GTK 的 surface**（否则 `Surface already has role`）。
2. **协议时序**：先 `commit` → 等 `configure` → `ack_configure` → **再** `attach buffer`。违反即 `must ack the initial configure before attaching buffer`。
3. **穿透公式**：`LAYER_OVERLAY + KEYBOARD_INTERACTIVITY_NONE + 空 input_region`。仅设键盘交互性**不会**穿透，空 input region 才是关键。
4. **锚定陷阱**：`anchor=0` 时 wlroots **强制居中**，margin 不参与计算。必须 `ANCHOR_TOP | ANCHOR_LEFT`。
5. **生命周期**：校准完成 → 计算 `CoordinateFrame` → **立即销毁 layer surface**（见 §4.4）。

> 完整协议时序、穿透公式与故障排查见 [`layer-shell-primer.md`](layer-shell-primer.md)，此处不再重复。

**实现细化（P1，本仓库的落地流程）**：

1. **可用区探针**：overlay surface 以"四边锚定 + 尺寸 0"创建，configure 事件宣告 usable-area 尺寸——**仅作标记布点的提示，永不进入概率池**。提示错了的后果只是标记出屏、检测不到、自动重试，**能自我纠正**；这正是"提示"与"测量"的分界。
2. **基线帧**：标记隐藏时截屏。
3. **标记序列**：两轮独立 pass，各采样 4 个抖动、乱序的 usable-area 角点（margin 预先量化到整数逻辑像素，与 backend 的 round 严格一致）。
4. **检测**：基线差分 **∧ 颜色** + 连通域 + **多帧互证**。三者缺一不可，见下方"检测的三重判据"。对应点取标记 bbox 的**几何中心**（排他边界），对光标穿透标记造成的空洞不敏感，也无像素索引质心的 −0.5px 约定偏差。
5. **求解**：≥4 组对应点做 2×3 仿射最小二乘（支持旋转输出）；残差与线性尺度 sanity 门控。
6. **验证**：3 个全新位置先预测后检测，误差 > 1.5px 即拒绝。
7. **一致性**：两轮 pass 的映射在 5 个探针点上偏差 > 1.5px 即拒绝。
8. **Teardown**：`destroy_projector` 在成功、失败、Drop **三条路径**上全部执行（§4.4）。

**检测的三重判据（P3，实测定案）**

差分检测隐含一个**从未被写下来的前提**：标记是两帧之间**唯一**的变化。这个前提在干净的测试桌面上成立，**在有人正在使用的真实桌面上不成立**——实测不投任何标记连采 30 帧，29 对相邻帧里有 1 对整屏变化 144386 像素（一个终端窗口重绘）。单次校准约采 28 帧，撞上的概率 `1−(1−1/29)^28 = 63%`，与实测失败率（4/12 ~ 7/15）吻合。

失败形态是 `68 plausible regions found`——**不是标记判错，是背景碎成了 68 块**；更危险的一次产出 287px 残差，说明背景碎块被当作合法对应点喂进了最小二乘（被残差门控拦下，**这正是它存在的理由**）。

因此检测必须同时满足三条：

| 判据 | 作用 | 单独为何不够 |
|---|---|---|
| **差分** | 隔离 fidus 自己的投射；静态壁纸（哪怕同色）两帧相同被抵消 | 别的窗口重绘同样是"变化" |
| **颜色** | 把标记从其它变化中分出来 | 干扰物可能恰好同色 |
| **多帧互证** | 标记是**持续存在的物体**，重绘是**时间上的事件**——再看一次，标记还在原处，重绘已经走了 | 同色且持续的干扰需前两条兜底 |

**颜色必须自适应选取**：固定色会与桌面内容撞色——默认品红与本机终端配色的色距仅 **33**，±40 容差下误匹配 1831 个背景像素。改为从**本来就要采的基线帧**中选当前屏幕上最罕见的候选色，实测余量 **117–128**（约为容差的 3 倍），成本为零。

> **为什么不是"重试时等一会儿"**：重绘是时间上的事件，立即重试大概率落在同一次事件内。但"等待"需要给后端加第三个原语，而规格明确只允许"投射 + 截屏"两个（§3.4）。**多帧互证用本来就要采的帧换来了同样的独立性**，不动架构约束——这与概率池"用更多证据代替更强假设"是同一个思路。
>
> 原实现的重试注释声称"burn a tick 让背景少发散"，实际那行 `rng.next_u64()` 耗时约 1 纳秒，**什么都没等**（§12 同型缺陷）。

**实测效果**（同一台机器、屏幕有真实活动，噪声底 9/29）：

| 配置 | scale=1 | scale=1.25 |
|---|---|---|
| 修复前 | — | **12/12 失败** |
| 修复后 | **0/15** | **0/15** |

**实测环境边界**：当前实机数字来自 Arch Linux + Niri + 接近默认的 kitty（kitty 配置只有 fish shell；Niri 使用默认动画/焦点环，并运行 waybar、mako、swww）。这足以验证“真实使用中的窗口会重绘”，但不外推到透明/模糊主题、复杂终端配色、视频/网页动画或其他合成器；这些场景必须单独实测。

**已知精度边界（实机实测，2026-09）**：完整数据见 [`measurements/l9-fractional-scaling.md`](measurements/l9-fractional-scaling.md)。

| 条件 | rms 残差 | 说明 |
|---|---|---|
| scale = 1 或 2（整数） | **0.000 px** | 完美 |
| scale = 1.25 / 1.5 / 1.75 | 0.025–**0.308 px** | ⚠️ 系统性退化 |
| transform 90 / 180 / 270 | **0.000 px** | 旋转被仿射完全吸收，**不是问题** |

**成因是投射端的取整，不是测量端的分辨力**：上文第 3 条要求 margin 量化到整数逻辑像素（为与 backend 的 round 一致），合成器随后落到 `round(L × scale)`，与理想值差最多 0.5 物理像素。整数 scale 下 `round(L×s) ≡ L×s`，残差必然为 0——实测完全吻合（可精确落点比例 100%/25%/50%/25%/100%，对应 rms 0.000/0.089/0.141/0.208/0.000）。

> **真正需要关注的是余量而非残差**：分数缩放下 `consistency` 实测最差 1.114px，门限 1.5px，**仅剩 26% 余量**；约 30 次校准中出现 2 次 `AreaMismatch` 失败。失败是**诚实的**（报错退出，不产出错误坐标，符合原则四），但可用性受损。面积先验（第 4 条）用"面积"当"是不是同一标记"的代理，正是 §12.2 记录的同型缺陷。
>
> **对症方向在投射端**（允许非整数逻辑坐标，或把标记的实际取整落点作为已知量代入求解），与频域精修类方案（L10）无关——**精确地测量一个落错位置的标记，只会得到精确的错误值。**

### 4.2 L10 GradientField · **已否决**（P3 提案，2026-09）

> **状态：不实现。** 依据见[提案 P3-L10](proposals/P3-L10-gradient-field.md)，数字可由该提案附录脚本复现。
> 本节记录**为什么否决**，以免后人重新发明同一个不成立的方案。

**原设想**（源自 v0.4）：投射对数螺旋渐变网格 → 对截屏做 2D FFT → 锁定主频与相位 → 主频得缩放、相位得偏移、倾斜得旋转。

**否决理由：方案的两半互斥。**

| 图案 | FFT 主频锐度 | 相位精度 | 整周期歧义 |
|---|---|---|---|
| 规则光栅 | **250.9**（尖锐） | **0.034 px** | ❌ 34 个等价解 |
| 对数螺旋 k=10…80 | **3.4–6.8**（无峰） | 不适用 | ✅ 无歧义 |

- **规则光栅**有可锁的主频，但相位是 `mod 2π` 的量——只能确定 `offset mod period`。1366px 宽屏、40px 周期下有 **34 个等价解**，整周期位移对相位**完全不可见**（实测差值 1e-16 rad，即双精度噪声）。
- **对数螺旋**确实消除了歧义（NCC 随位移单调下降且不回升，全局唯一），**但它消除歧义的方式正是让局部频率随位置变化——这与"存在主频"是同一枚硬币的两面**。实测 k 从 10 到 80 锐度始终只有 3.4–6.8（对照：规则光栅 250.9），且 **k 越大能量越弥散**（前 100 强频点占比从 13.4% 降到 1.6%）。这是图案的固有性质，不是参数没调好。

> **一句话**：**要主频就得规则，要规则就有歧义；消歧就得不规则，不规则就没主频。**

**结论**：L10 **不能作为独立校准器**——它可以把已知的粗位置精修到 0.03px，却无法回答"在哪个周期里"。消歧需要一个已知位置的参照物，而那正是 L9/L0 已经在做的事。

**一条被写反的风险**（更正）：原文把"动态壁纸/视频背景的频谱污染"列为主要风险。**实测不成立**——窄带相位估计对宽带噪声天然免疫：壁纸能量摊在所有频率上，而估计只在**一个已知频点**取值。强纹理背景下误差仍为 **0.034 px**。

> **教训**（已并入 §12.5 与 AGENTS §10）：**担心错了对象，比不担心更危险**——它把注意力从真正的障碍（歧义）上引开了两年。

**仍然保留的构想**（未被本次否决触及，若将来另起炉灶可参考）：

1. **频域滤波抹除器**（× L1）：网格与目标空间频率不同，滤除网格频率后目标轮廓如剪影般清晰。
2. **涟漪 odometry**（× L4）：拖拽时的水波纹反馈，其涟漪中心轨迹可替代全局鼠标坐标——与原则一相容。
3. **相位零点免费校准**（× L4）：周期性动画的相位归零帧可作绝对基准。

> 这三条依赖的是"相位精度"而非"独立定位"，因此**不受本次否决影响**。但按 AGENTS §10，复活其中任何一条都需先回答：**它解决谁的问题？**

**代码现状**：`CalibrationMethod::GradientField` 保留为枚举变体，Gate 诚实返回 `FeatureDisabled`，builder 返回 `None`。**没有任何代码依赖它**——这正是 P0"先立架构约束"换来的回旋余地：推翻一个方案的成本是零。

### 4.3 L0 Anchor · 通用兜底校准器

创建 4 个纯色哨兵方块（颜色取高饱和冷门色；尺寸 2×2 为配置下限，**默认 8×8**），锚定虚拟屏幕四角。每次校准随机打乱"颜色→角点"的映射并立即销毁重建，至少通过 2 轮独立采样验证映射一致性，并结合几何矩形约束确认，方可输出 CoordinateFrame。

> **注意**：单纯"找色"不足以检测哨兵——那是 P2-d 修掉的缺陷。正确做法是**基线差分 ∧ 颜色**，见下方实现细化第一条。

> **实现细化（P2-d，2026-09-09）**：
> - **检测 = 基线差分 ∧ 颜色**，而非单纯找色。L0 没有 L9 的"置顶 overlay"保证，纯找色会被壁纸里的同色块欺骗；先截"标记隐藏"基线，再要求哨兵像素既*变化*又*带哨兵色*，静态同色块在差分中抵消。只有在两帧之间移动的诱饵能干扰，而那以歧义/未检出进入重试——与 L9 的原则一致：干扰只能让单次测量失效，无法伪造。
> - **角点按"整条边"抖动**：四角位置每轮随机，但仍是轴对齐矩形，矩形约束（等对角线平行四边形）继续成立。
> - **验证轮**：与 L9 一致，解出映射后先预测再检测全新内部位置。
> - **标记尺寸**：2×2 作为配置下限，默认 8×8（2px 低于任何真实截屏的噪声底）。
> - **依赖原语**：只需"投射 + 截屏"，外加"同时投射多个标记"的能力（§5.2 `multi_marker_projection`）。Gate **按能力而非平台身份**判定 L0 可用性——它是全平台兜底，不是"X11 专属"。
> - **已知不可用环境**：rootless XWayland（`GetImage(root)` → `BadMatch`）；X11 backend 连接时探测一次截屏并诚实报告。

### 4.4 校准生命周期（teardown 是关键）

```
calibrate():
  surface = create_layer_surface()
  loop:
      commit → configure → ack → set_input_region(空) → attach → present
      采集视觉反馈 → 求解 CoordinateFrame
  ★ destroy_layer_surface(surface)    // 必须显式销毁，不留残留
  return CoordinateFrame              // 交给 C 层做增量追踪
```

**Q9 教训**：Wayland 客户端无法查询自身全局坐标，`hide()` 之后位置信息失效。因此**坐标必须在 `destroy` 之前读取并缓存**于 `CoordinateFrame`。

### 4.5 定期校准触发条件

校准器不在稳态运行，仅在以下事件时重新触发：

| 触发条件 | 落地状态（P2-g） |
|---|---|
| 调用方显式请求 | ✅ `invalidate_frame()` |
| 显示器热插拔 / 分辨率变化 | ⬜ 由调用方判断后调用 |
| DPI 缩放因子变化 | ⬜ 由调用方判断后调用 |
| 置信度 `< 0.4` 持续一段时间 | ⬜ 由调用方判断后调用 |
| 系统空闲超过阈值（如 30 分钟） | ⬜ 由调用方判断后调用 |

> **当前边界**：库**不内建**计时器、热插拔监听或自动重校准——那需要常驻后台活动，与原则五（校准完即撤）相抵触。fidus 只提供 `invalidate_frame()`，何时调用由调用方决定。自动触发列为 P3 候选；若将来实现，也必须是"调用方可关闭"的显式行为。

---

## 5. Gate · "哪些情况不能用"的可编程接口

能力探测是 fidus 对外 API 的核心部分。调用方**必须先查询，再决定要不要校准**。

### 5.1 `CalibrationStatus`

```rust
pub enum CalibrationStatus {
    /// 可用，精度无退化
    Available { method: CalibrationMethod },
    /// 平台/环境不支持（如 L9 需要 layer-shell，但跑在 GNOME<49）
    NotSupported { method: CalibrationMethod, reason: UnsupportedReason },
    /// 需要权限，可引导用户开通（如屏幕录制权限）
    PermissionRequired { method: CalibrationMethod, permission: PermissionType },
    /// 可用但受环境限制（如视觉方案遇动态壁纸，置信度下降）
    Degraded { method: CalibrationMethod, estimated_confidence: f32 },
}
```

### 5.2 `EnvironmentContext`（探测输入）

```rust
pub struct EnvironmentContext {
    pub has_layer_shell: bool,
    /// `None` = 未知。调用方可直接告知；也可由 `ScreenClassifier`（P2-e）
    /// 自测——两次截屏 + 帧差分，纯视觉，不问任何平台 API。
    pub is_dynamic_wallpaper: Option<bool>,
    pub multi_monitor_count: usize,
    pub compositor_type: CompositorKind,   // Niri / Sway / KWin / Mutter / ...
    pub screen_capture_permission: PermissionState,
    pub wayland_input_region_supported: bool,
    /// P2-d 新增：后端能否同时投射多个标记（每标记一窗 → true；单 layer surface → false）。
    /// L0 Anchor 的能力前提。这是能力描述，不是平台身份。
    pub multi_marker_projection: bool,
}
```

### 5.3 `PermissionState` 状态机（一等公民）

```
Unknown → Requesting → Granted → Revoked → RequiresRestart
```

> macOS 权限变更后**进程需重启**才能生效；Linux Portal 每次需用户点确认。库必须把此状态暴露给上层做 UX。

### 5.4 典型用法

```rust
let availability = engine.query_calibrator_availability(CalibrationMethod::Crosshair);
match availability {
    CalibrationStatus::Available { .. } => engine.start_calibration(...),
    CalibrationStatus::NotSupported { reason, .. } => /* 透传 UI / 降级 */,
    CalibrationStatus::PermissionRequired { permission, .. } => /* 触发授权流程 */,
    CalibrationStatus::Degraded { .. } => /* 仍可校准，但对结果施加约束 */,
}
```

---

## 5.5 坐标语义（规范定义）

校准产出 `CoordinateFrame`：一条**逻辑空间 ↔ 物理空间**的仿射映射。这两个术语全项目通用，定义如下：

- **逻辑空间（`LogicalPoint`）**：被校准输出的 **layer-shell usable-area 坐标**——与 layer-shell margin 同一坐标系。调用方拿到后可直接用于自己的窗口摆放。
- **物理空间（`PhysicalPoint`）**：**fidus 自己截屏里的像素坐标**（Y 反转已由 backend 归一化）。

**两侧都是 fidus 自产量**：逻辑侧锚定在 fidus 自己选的标记 margin 上，物理侧锚定在 fidus 自己截到的像素上。仿射求解**天然吸收**合成器藏起来的一切——usable-area 偏移（面板/独占区）、分数缩放、输出旋转——全程没有读取过一个平台坐标。这是原则二"自建地图"的具体含义。

> **为什么不叫"屏幕坐标"**：那个词暗示存在一个可被查询的全局真值。§2.1 的取证表明它不存在。fidus 只承认自己测出来的两个空间，以及它们之间那条自己解出来的映射。

---

## 6. 概率池纯净性（架构铁律）

### 6.1 输入白名单

> **自身窗口范围**：本节的目标是调用方自身窗口。调用方提供该窗口的离屏渲染模板，L1/L8/L7 只负责在截屏中定位其可见内容；fidus 不枚举、识别或定位任意第三方系统窗口，也不提供窗口实例身份。目标完全不可见时，系统必须报告丢失或低置信度，不得声称恢复了窗口几何。

| 输入源 | 进池？ | 理由 |
|---|---|---|
| L9 Crosshair 视觉测量 | ✅ | 纯视觉，噪声可建模 |
| ~~L10 GradientField~~ | — | **已否决**（§4.2），从不产生测量 |
| L1 Fingerprint | ✅ 需先通过可定位性检查（§6.1.2） | 纯视觉，但模板须可定位 |
| L7 融合输出 | ✅ | 仅由上列测量加权而成，不引入新信息源 |
| L8 EdgeSync BBox | ✅ | 纯视觉 |
| L4 拖动位移 | ✅ | 运动模型 |
| **任何 native API 返回值** | ❌ **类型层面禁止** | 来源不可信、量纲不可比、污染池 |

> **"纯视觉"是必要条件，不是充分条件。** 不可定位的测量与伪造无异：线性渐变模板会让 NCC 在任意位置给出满分（§12.1）。因此 L1 的模板必须通过**注册期可定位性检查**（`localizability()`）才算合格输入——见下文 6.1.2。

#### 6.1.1 L8 EdgeSync 输入门控（MotionGate）

> **本节是门控行为的唯一规范。** §12.3 只记录"当初为什么改"的踩坑叙事。
> 早期版本的条文（仅"R 超阈即弃用"）已被 P2-f 推翻——照那样实现会**复现已修掉的吸收态缺陷**，详见下方失效模式。

在每次将 L8 的 BBox 测量送入 Kalman 之前，必须通过 MotionGate 检查：以 **500ms 为窗口**（按真实时间，不按调用次数）执行帧间差分，计算目标区域像素变化率 `R ∈ [0,1]`。

**判决阈值**：

```
threshold = min( max(baseline × 3, 15%), 50% )
若 R > threshold → 判定 DYNAMIC
```

- **上限 50% 不可省略**。超过半数像素变化在任何定义下都是动画，不允许学习到的 baseline 反驳它。*失效模式*：缺此上限时 baseline 可把阈值推过 1.0，而 `R ∈ [0,1]` 永远无法跨过 → 门控永久失效、无回落路径（P2-b 实际踩过）。
- **baseline 只由通过门控的（静态）观测学习**；DYNAMIC 帧**绝不参与学习**。*失效模式*：让被判动态的帧抬高判它自己的门槛，是正反馈回路——约 22 次观测后真动画即被放行（P2-b 实际踩过）。
- 学习采用**非对称 EMA**（慢升快降），避免稳态噪声把门限顶死。

**两条有据放行例外（P2-f）**：R 超阈时，以下情况测量**仍然放行**——

1. blob **在位移处通过模板验证**：R 的尖峰由"目标离场"解释，而到达处的内容经过了模板验证，这比统计门是更强的证据；
2. blob **尺寸超出模板足迹**（`DEPARTURE_EXCESS_PX`）：重绘不可能超出它所重绘的足迹，超出部分即"内容离开过原位"的几何证据。

*为什么必须有这两条*：缺了它们，慢速拖拽（每窗口 2px）会让 R≈0.92 恒不下降 → 门控永不 re-arm → 测量被永久丢弃，**尽管模板已验证通过**。这是典型吸收态，L8 对最常见的慢速拖拽场景永久静默。推导见 §12.3。

仅当 R 超阈**且无任何有据解释**时才弃用测量、进入等待；**连续 2 个窗口通过后**才恢复输出。冷启动**不算**动态结束，首个安静窗口即可输出（2 帧规则只约束"动态结束后的恢复"）。

连续 6 个窗口判定动态时，`is_persistently_dynamic()` 如实上报"此区域长期动态"，供调用方决策——这是对"把动画吸收进 baseline"的诚实替代。

此 Gate 不属于原生 API 门面，纯粹基于帧差分的视觉计算，与零信任原则严格一致。

#### 6.1.2 L1 目标外观准入（P2-g）

`register_target` 先跑 `localizability()`：判据是**自相关随平移距离的衰减**，不是方差（方差与可定位性**反相关**，§12.1）。

- 自相似度 ≥ **0.98** → 直接拒绝；
- 仅对起步（r=2）> 0.9 的模板，额外要求 r=2→r=16 衰减 ≥ **0.1**（低起步模板豁免此项，故 0.822 起步的纯色块+细边框不受影响）。

**逃生门 `UntrackablePolicy`**（公开 API 契约）：默认 `Refuse`；调用方可**显式**选 `TrackWithReducedConfidence`。它**不是"跳过检查"开关**——检查照常执行、歧义度照常实测，但置信度上限压到 `1 - 自相似度`（下限 0.05），L7 与 Kalman 自动降权。池子里没有伪造的位置，只有**被如实标注为"弱"**的位置。下限不取 0，因为"弱测量"与"无测量"（§4.5 丢失后报告先验）是两种不同陈述，必须可区分。


### 6.2 类型层面强制

`fidus-core` 的公开 API **不含任何坐标类 native 输入**：

```rust
pub struct FidusEngine { /* calibrator / estimator / gate / io_factory / frame */ }

impl FidusEngine {
    // EngineParts 由平台胶水层装配；其中不含任何坐标值
    pub fn new(parts: EngineParts) -> Self;

    pub fn gate(&self) -> &dyn Gate;                                        // §5
    pub fn calibrate(&mut self) -> Result<&CoordinateFrame, CalibrationError>;
    pub fn register_target(&mut self, t: TargetDescription) -> Result<(), EstimateError>;
    pub fn estimate(&mut self) -> Result<ProbabilisticPosition, EstimateError>;
    pub fn invalidate_frame(&mut self);                                     // §4.5
}
```

> 注：`estimate()` 返回 `Result`——目标丢失且无先验时必须**报错**而非编造位置（§4.5）。

#### 6.2.1 `SolvedMap`：把"编译器应阻止"变成事实（P2-f）

上面那句"编译器应阻止"在 P2-f 之前是**空头支票**：`AffineTransform` 的六个系数是 `pub`，`CoordinateFrame::new` 接受任意变换——一个平台矩形可以直接写成 `AffineTransform { c: x, f: y, .. }` 冒充校准结果。这正是 §6.1 白名单要禁的污染，却能随手做到。

现在的封印：

1. **系数私有**，只读访问经由 `coefficients()`；
2. **`SolvedMap` 是唯一凭证**——只有 `from_correspondences()` 能铸造它，无公开构造函数；
3. **`CoordinateFrame::new` 只接受 `SolvedMap`**。

于是坐标系**只能靠测量挣来，无法声称**。连 `AffineTransform::IDENTITY` 都不行——它在 1× 单显示器上看起来完全正常，却会让 fidus 把截图像素当逻辑坐标上报，是最隐蔽的一种污染。两个 `compile_fail` doctest 把这两个洞钉死。

**诚实的边界**：调用方若伪造喂给 `from_correspondences` 的 `PhysicalPoint`，仍得到伪造的地图。这个洞**无法用类型消除**（检测器必须能构造测量点），只能靠"检测器自己从截图里检出质心"这一构造方式封闭。封印消除的是**随手注入**那条路径——也正是实际发生过的那条。

---

## 7. crate 拆分

> ✅ = 已实现（P2-g 时点）　🚧 = 规划中，尚未创建

```
fidus-core       ✅ // 类型系统 + 三层 trait + 概率池 + 坐标空间（SolvedMap 封印）
fidus            ✅ // 顶层 facade：FidusBuilder + fidus-live-calibrate CLI
fidus-backend-*     // （仅适配最基础合成器原语，非 API 门面）
                   ├─ wayland-layer  ✅ (wlroots/KDE/Niri)
                   ├─ x11            ✅
                   ├─ wayland-portal 🚧 (GNOME)
                   ├─ windows-gdi    🚧
                   └─ macos          🚧
fidus-calibrate  ✅ // L0 Anchor / L9 Crosshair（L10 已否决，仅留 feature 占位）
fidus-estimate   ✅ // L1/L8 + L7 融合 + 常速度 Kalman (L4)
fidus-extras     🚧 // 业务特定项：L3 Beacon / L5 TUIScan——移出核心，尚未实现
```

> 注：`fidus-backend-*` 适配的是**基础绘制/截屏原语**，不是"读取窗口坐标"的 API——后者一律不封装。

---

## 8. P4 测试子项目方案：`fidus-test`

> 来源：[`docs/proposals/P4-fidus-test-subproject.md`](proposals/P4-fidus-test-subproject.md) 与 [`docs/drafts/P4-fidus-test.md`](drafts/P4-fidus-test.md)。
> 状态：**P4 历史快照：本机实现与候选 Release 资产已完成；当前正式发布未收口**。CI 协议核心、live-host 协议桥接、本机 live-container 实测、Debian 12 runtime 归档和自动验证脚本在该快照中已完成；跨机器复现与当前 clean source binding 仍未完成。本节仍不把容器宣称为显示隔离。

### 8.1 目标与边界

`fidus-test` 是跨环境测试工具与报告层，不是生产 backend、校准器或估计器。它只通过 fidus 正式公开 API 调用被测系统，不能复制 `detect.rs`、校准器、仿射求解或 backend 实现，也不能读取平台窗口几何作为真值。

crate 内现有仿真测试继续留在生产 crate 中；`fidus-test` 负责真实环境编排、诊断、协议解析、生命周期和报告。

旧的 `fidus-live-calibrate` 入口在第一阶段保留，新入口不得复制生产逻辑；旧入口只允许是共享实现的薄兼容包装。

### 8.2 三种执行模式

| 模式 | 作用 | 显示会话 | 输出修改权 |
|---|---|---|---|
| `ci` | 固定构建、workspace 检查、协议/报告单测 | 无 | 无 |
| `live-host` | 用户正在使用的真实桌面测试 | 宿主会话 | 仅显式授权 |
| `live-container` | 验证容器进入宿主显示会话 | 显式挂载 | 容器无权修改，由宿主 runner 独占 |

三种模式必须分开报告和统计。容器不等于显示隔离；挂载 Wayland/X11 socket 后，宿主 compositor 和真实桌面仍是信任边界。

`ci` 默认不挂载 `$XDG_RUNTIME_DIR`、Wayland/X11 socket、整个 HOME 或 `/dev/dri`。`live-container` 只允许最小显式挂载，默认非 root，不授予 privileged，不默认挂 `/dev/dri`。

实施顺序固定为：`ci → live-host → live-container`。`live-container` 首轮只验证入口兼容性，不能替代 `live-host` 的真实性能结论。

### 8.3 Output mutation 与恢复

任何会修改 scale/transform 的命令都必须由调用方主动传入 `--allow-output-mutation`。没有该门，不得修改用户当前 compositor 配置。

只有宿主 runner 拥有 output mutation authority。恢复状态机必须记录不可伪造的原值快照：`output_id`、原始 `scale`、原始 `transform` 和读取时间；同一 output 在一次 run 内不得改变身份。宿主 runner 必须用锁拒绝并发 mutation runner，不能让两个进程互相覆盖恢复值。

状态顺序固定为：`ReadOriginal → AppliedAndReadBack → Running → RestoreRequested → RestoredAndReadBack`。任何子进程启动失败、测试失败或正常退出都先进入 `RestoreRequested`；恢复失败覆盖原始测试结果，最终为 `RecoveryUnverified`。只有读回值逐字段等于原值才可报告恢复成功。

容器 runner 不执行 compositor 控制命令，也不拥有第二个恢复器。正常退出、SIGINT、超时和子进程异常必须测试恢复路径；SIGKILL、宿主崩溃和 compositor 重启后的状态不可证明，必须报告 `RecoveryUnverified`，不得声称恢复成功。

### 8.4 机器可读结果协议（v1，冻结）

在人读日志之外，测试 binary 输出版本化行式 key-value。**一行是一个独立记录；同一运行通过 `run_id` 关联。**

公共字段（每一行都必须有）：

```text
FIDUS_RESULT version=1 kind=<kind> run_id=<token> status=<status> execution_mode=<mode>
```

`kind` 只有以下四种：

| kind | 必需字段 | 允许 status | 语义 |
|---|---|---|---|
| `environment` | `backend compositor output scale transform` | `ready`, `unavailable` | 测试前环境事实；不是坐标真值 |
| `calibration` | `backend method`；成功时另有 `rms_residual_px verification_max_err_px consistency_max_err_px` | `ok`, `failed` | fidus 本次校准结果 |
| `lifecycle` | `teardown`、`recovery` | `ok`, `failed`, `unverified` | overlay teardown 与宿主恢复证据 |
| `summary` | `records_total records_ok records_failed` | `ok`, `failed`, `harness_error` | 一次 run 的唯一终态 |

固定示例：

```text
FIDUS_RESULT version=1 kind=environment run_id=r01 status=ready execution_mode=live-host backend=wayland compositor=niri output=eDP-1 scale=1.25 transform=normal
FIDUS_RESULT version=1 kind=calibration run_id=r01 status=ok execution_mode=live-host backend=wayland method=crosshair rms_residual_px=0.203 verification_max_err_px=0.731 consistency_max_err_px=0.856
FIDUS_RESULT version=1 kind=lifecycle run_id=r01 status=ok execution_mode=live-host teardown=confirmed recovery=not_requested
FIDUS_RESULT version=1 kind=summary run_id=r01 status=ok execution_mode=live-host records_total=3 records_ok=3 records_failed=0
```

协议规则：

- 行首固定为 `FIDUS_RESULT `；公共字段唯一且必需；
- `run_id` 是非空 ASCII token，同一 run 的所有记录必须一致；
- key 只允许 `[a-z0-9_]+`，value 第一版只允许无空格 ASCII token；
- 版本未知、kind 未知、status 不在该 kind 的允许集合、重复 key、缺字段、非法数值、run_id 不一致或缺少唯一 `summary`，均为 `HarnessError`；
- `summary` 是 run 的唯一终态；`records_total/ok/failed` 只统计 summary 之前的业务记录，不把 summary 自身计入；没有 summary 不能把 run 算作通过或失败；
- `environment=unavailable`、`lifecycle recovery=unverified` 是测试工具/环境结果，不计入 fidus 校准失败率，但会使场景验收失败；
- 普通人读日志可以保留，但不能被当成协议记录；第一版不引入 serde/JSON 依赖。

`teardown=confirmed` 只表示 fidus 自己的 `CalibrationIo::destroy_projector` 返回成功并完成内部 teardown 路径；它**不是**通过 `niri msg layers`、`xwininfo` 或其他窗口几何查询证明的。外部平台检查只能作为人工诊断日志，不能进入协议真值。


### 8.5 实施选择

实现载体已按职责分层审查，脚本、专用 crate 与容器**共存而非三选一**：shell 负责宿主编排、compositor 控制、容器启动和恢复；`fidus-test` 负责协议、解析、报告、退出语义和场景模型；容器负责固定 CI toolchain/系统包，或显式承载 live binary。真正被否决的是单层包办一切、继续把工具堆入 `fidus` 伞 crate，以及高权限万能容器。因此采用 `fidus-test + ci/live-host/live-container`，三种模式分开报告与统计。

### 8.6 实施阶段与剩余门槛

CI 协议核心（独立 crate、feature forwarding、严格 parser、run summary 校验）与 live-host 协议桥接已实现并通过 workspace test/clippy/doc；live-container 已用本机临时实验镜像真实访问当前 Wayland 会话并完成校准；正式镜像现在统一为 Debian 12 slim 的 pinned multi-stage 构建，并提供 `fidus-live-debian12.tar.zst` Release 归档及 SHA-256/image ID 校验文件，协议记录中的 `execution_mode=live-container` 已由宿主 parser 验证。不同 UID 会在 backend 初始化阶段得到 `EnvironmentUnavailable`，缺 socket 得到 `EnvironmentUnavailable`，非法 runtime 得到 `HarnessError`；宿主参数契约也有自动测试。正式 Debian runtime 镜像已导出为 `fidus-live-debian12.tar.zst`；固定 digest、image ID、归档 SHA-256 和离线协议 smoke 均有对应 Release 资产。CI pinned 镜像已完成 build/run：build 阶段使用约五次网络重试预热依赖，run 阶段使用 `network=none`；CI 的 target/cache 保持可写以执行 Cargo build script，live-container 则使用只读 rootfs 和 noexec tmpfs。

P4 已完成的门槛包括 feature 构建、协议单测、双入口兼容、Debian 镜像 build/run、Release 归档校验和 live-container 权限实验。output mutation authority 与完整恢复状态机仍是后续独立工作，不能因 P4 完成而宣称已经实现。

所有未通过的环境仍应报告 `EnvironmentUnavailable`，不能伪装成通过。

### 8.7 P5 output mutation/recovery 方案

P5 方案已根据提案、草案全审和实际实验冻结设计边界；**本节是方案，不代表真实 mutation runner 已实现**。详细接口、伪代码和测试矩阵见 [`docs/drafts/P5-output-mutation-recovery.md`](drafts/P5-output-mutation-recovery.md)，实测证据见 [`docs/measurements/p5-output-mutation-recovery.md`](measurements/p5-output-mutation-recovery.md)。

#### 8.7.1 权限与职责

- 只有 `live-host` 宿主 runner 可以执行 output mutation、保存快照、恢复和 read-back；
- 必须由调用方显式传入 `--allow-output-mutation`；默认不得修改用户当前 compositor 配置；
- `live-container` 永远拒绝 mutation；容器内 binary 不调用 compositor 控制命令，也不拥有第二个恢复器；
- mutation 只属于测试控制，不属于 fidus 坐标真值，不能进入概率池或 `CoordinateFrame`；
- output 名称只能是命令 selector，不能单独证明稳定硬件身份。

#### 8.7.2 快照、应用与恢复

宿主 runner 必须在 mutation lock 内保存不可变原始快照：

```text
output selector
identity evidence
original scale
original transform
wall-clock time
monotonic time
```

状态顺序固定为：

```text
LockAcquired → ReadOriginal → AppliedAndReadBack → Running
→ RestoreRequested → RestoredAndReadBack → Finished
```

apply 和 restore 都不能只信命令退出码，必须读回并逐字段比较。恢复每个字段都必须 best-effort 尝试；一个字段失败不能跳过其它字段。恢复前还必须检查本 run 最后一次 applied 状态，发现用户或其它工具外部修改时默认拒绝覆盖。

当前 Niri 实测发现：未连接 output 的控制命令可能返回 `rc=0` 并表示“连接后生效”，所以 `rc=0` 不是 apply 证据；transform `90` 的读回是 `90° counter-clockwise`，比较前必须 canonicalize。

#### 8.7.3 恢复、信号与锁失败

恢复结果优先级高于 child/calibration 结果。恢复失败、身份无法证明、锁释放无法证明、compositor 重启、宿主崩溃或 SIGKILL 后，均不得报告恢复成功；协议 recovery 值冻结为：

```text
not_requested | confirmed | unverified
```

其中当前 Niri 的 NameOnly 身份只能产生 `unverified`，不能产生 `confirmed`。宿主 supervisor 必须使用实际 PGID，对 child 进程组执行 `TERM → bounded wait → KILL → wait/reap`；恢复阶段重复信号只记录，不重入恢复。锁必须使用 canonical session lock 的内核持有语义，PID 元数据只是诊断，stale lock 不自动删除。

P5 当前保守退出码真值表如下；恢复未被稳定身份或 read-back 证明时，优先级最高并统一返回 `4`：

| 条件 | lifecycle recovery | runner exit |
|---|---|---:|
| 未授权、参数、symlink lock 或锁竞争错误（尚未取得 snapshot） | `not_requested` | `3` |
| 环境不可用或 snapshot 读取失败（尚未取得 snapshot） | `not_requested` | `2` |
| snapshot 后任意 apply/child/信号路径，且恢复无法证明 | `unverified` | `4` |
| 当前 Niri 字段已恢复但只有 NameOnly identity | `unverified` | `4` |
| child 返回非零但恢复仍只能是 NameOnly | `unverified` | `4` |
| 未来具备稳定 identity 且所有字段 read-back 成功 | `confirmed` | child 原始结果（0 或业务失败） |

真实桌面错误处理的单点验证已冻结：本项目不再要求或默认执行该真实异常实验，不能用本机 fake 回归替代真实 compositor 证据，也不能把 TTY/独立终端当作隔离。跨机器发布验证另行按社区互助流程处理。

**意外恢复失败的人工处置**：真实 mutation 若经明确授权执行，操作者必须在开始前把 `output`、原始 `scale`、原始 `transform` 和读取时间记录到屏幕外的持久日志；若 runner 返回 `recovery=unverified`、异常退出、会话崩溃或被 SIGKILL，必须先停止其它 output mutation，再从同一会话的 `niri msg outputs` 确认目标 selector，手动执行 `niri msg output <OUTPUT> scale <ORIGINAL_SCALE>` 与 `niri msg output <OUTPUT> transform <ORIGINAL_TRANSFORM>`，随后再次读取并逐字段核对。若 selector/原值/当前状态无法确认，禁止猜测 `scale=1` 或 `transform=normal`，应保持 `recovery=unverified` 并由人工恢复。该人工流程只能降低遗留风险，不能证明自动恢复可靠；意外路径下恢复本身明确不可靠。

本人工流程不进入默认 CI 或生产路径，也不把手动恢复结果升级为 `recovery=confirmed`。

#### 8.7.4 人工恢复工具方案

`P5-manual-output-recovery.md` 已完成提案阶段的核心假设核验和自审裁枝，现转入本方案并实现为 `scripts/manual_restore_output.sh`。该工具只接受显式 `--allow-output-mutation`、output selector、原始 scale 和原始 transform；执行前解析 `niri msg outputs`，要求目标 output 唯一且已连接；随后分别设置两个字段、逐字段读回并写入屏外日志。selector 不唯一、参数错误、环境不可用、setter 失败或读回不一致均不得猜测默认值，并返回保守非零结果。工具只提供人工补救，不自动接管崩溃、不证明稳定 identity、不把结果升级为 `recovery=confirmed`，也不进入默认 CI 或生产路径。无显示 fake 回归必须覆盖正常恢复、selector 拒绝、部分 setter 失败、读回不一致和日志记录。

#### 8.7.5 实现状态

已完成：提案批准、草案全审、协议 recovery parser 约束、Niri 只读/正常路径实验、fake 模型实验、锁竞争和进程组实验、`fidus-test` 纯测试 fixture、仅供显式手工调用的最小 `scripts/output_mutation_runner.sh`，以及仅供显式人工调用的 `scripts/manual_restore_output.sh`（含 selector/字段 read-back、日志和 fake 正负回归）（fake Niri 回归通过）。方案 A 审计还修复了 snapshot 后早退恢复、recovery 优先、实际 PGID 信号转发、symlink lock、ASCII 控制字符、malformed/deferred parser 和并发 flock 边界。尚未完成：真实 Niri 异常路径与跨 compositor 身份证明；该脚本仍不是默认测试入口，当前 NameOnly 即使字段恢复也只能返回 `recovery=unverified`/退出码 4，不能报告 confirmed。因此不得把 P5 方案写成完整 P5 已实现。

---

## 8.8 核心库完全体收口与边界硬化方案

> 本节是由 `docs/drafts/complete-state-hardening.md` 转入的方案约束。它定义收口目标和验收边界，不表示下列代码工作已经完成；实现必须按工作包逐项提交证据，真实桌面 mutation、破坏性实验、跨机器验证和发布仍需单独授权。

### 8.8.1 范围裁决

核心收口只覆盖现有 L0 Anchor、L1 Fingerprint、L4 Relative、L7 Fusion、L8 EdgeSync、L9 Crosshair，以及支撑它们的 Wayland/P5 测试边界。L6 继续并入 L1；L2 不恢复为独立全局指针层；L3 Beacon、L5 TUIScan 继续移出核心；L10 GradientField 维持否决；L2.5 PointerProbe 维持删除。

“完全体”在本项目中的含义是：已支持的投射 + 截屏抽象上，不存在已知由公开输入触发的 panic、NaN 传播或 Critical/High 级静默错误；这不等于所有 compositor、所有平台或所有异常桌面证据已经完成。

### 8.8.2 工作包与强制约束

1. **Fused 置信度边界**：L1 的 raw quality 与可接受 confidence ceiling 必须分离表示；L7/Kalman 只能接收应用 ceiling 后的值。默认不可定位模板必须拒绝，opt-in 只能降权。只有周期/渐变回归仍能穿透时，才另立 innovation/motion gate 方案。
2. **输入与数值边界**：所有公开 affine/correspondence、Frame、stride/data、尺寸乘法、BoundingBox、ROI、图像尺寸、MotionGate 和 Anchor 配置必须拒绝非有限、溢出、空/不足数据、负区间和 `passes=0` 等退化输入；不得用 epsilon、clamp 或默认值伪造合法测量。
3. **Anchor 几何与多轮**：验证一般 affine 的真实不变量，不以对角线等长作为通用条件；所有 pass 必须参与一致性判决；map、quality、验证点必须来自同一有效结果或明确聚合；验证点不足必须失败。
4. **Wayland buffer 生命周期**：screencopy 与 marker buffer 只有在收到 compositor `release` 后才可复用；`ready` 只表示内容可读；session 必须区分 ready、failed、released、destroyed，并覆盖延迟 release 和快速复用。
5. **P5 runner 安全**：无法证明 escaped session 清理时不得报告 confirmed；partial apply 必须记录已成功 setter；restore 前后外部变化和 TOCTOU 默认保守拒绝覆盖；lock 清理失败必须升级结果；runtime mount 使用 allowlist/结构化参数；各阶段信号进入不可重入恢复；wrapper 输出失败不得静默吞掉。
6. **协议与 wrapper**：summary status、业务状态和 counts 必须一致；child 为 0 但 summary 为 failed/harness_error 时必须非零；malformed public record 必须返回错误；broken pipe 必须有明确 harness error；wrapper 必须有真实 integration test。
7. **文档与发布**：历史段落必须标注快照；F42 必须区分 fixture/protocol pass 与真实观察；审查记录绑定明确 revision；release manifest、SBOM、provenance、签名必须来自同一 clean source revision，并保持 fail-closed。

### 8.8.3 执行顺序与验收

```text
WP-A Fused 置信度
→ WP-B 输入/数值边界
→ WP-C Anchor 几何
→ WP-D Wayland 生命周期
→ WP-E P5 runner
→ WP-F wrapper 协议
→ WP-G 文档/发布
→ 全量审查
```

每个工作包完成时必须同时提交：代码入口和调用链、失效模式说明、无显示回归、失败/回滚边界、实际命令及结果。工作包之间不得用 fake 证据替代真实 compositor 或跨机器证据。

### 8.8.4 历史归档状态标注

历史归档不得通过修改原 archive、manifest、provenance 或签名来补齐新格式。允许增加与归档绑定的机器可读旁证文件，例如 `fidus-live-debian12.archive-status.json`，但它只解释历史状态，不改变发布校验结果。

标注文件必须包含归档 `filename`、`sha256`、`size_bytes`，并固定声明 `status=historical`、`current_release_eligible=false`。旧来源只能放在 `source_binding.status=legacy-unbound` 与 `historical_commit`/`historical_tree` 字段中，不能伪装成当前 `source.revision_binding`。标注文件作为普通受版本控制文件参与 source digest，不列入签名阶段可变 release outputs 的排除列表。

工具必须区分 `current-bound`、`historical-unbound`、`invalid`：历史标注只能使旧归档状态更可解释，不能绕过当前源码绑定、artifact/SBOM/provenance hash、签名或正式发布门禁。标注自身与归档 hash/size 不匹配、字段缺失、JSON 非法，或被篡改为当前可发布状态时必须 fail-closed。真实发布和跨机器验证仍由独立授权与证据门槛决定。

### 8.8.5 当前验收与授权边界

- 无显示矩阵仍按 F1–F42 单项统计；`real-pending` 不得改写为 `pass`。
- 真实 Niri 正常路径只能证明本机观察；当前 NameOnly 身份不能产生 `recovery=confirmed`。
- 真实 compositor 异常、SIGKILL、compositor restart、跨 compositor 和跨机器发布验证均为独立证据，不由本方案自动授权。
- push、正式发布、真实桌面 mutation 和破坏性实验仍需调用方明确请求。
- 方案完成定义是：工作包证据齐全、全量门禁通过、文档与 release 时间线一致；不是“所有候选功能都实现”。

---

## 9. 实施优先级（P0 → P5）

| 阶段 | 内容 | 说明 |
|---|---|---|
| **P0** | §1 精神内核 + §6 概率池纯净性 + §5 能力探测接口 | **架构地基，先于一切代码** |
| **P1** | L9 Crosshair（Niri 实机验证）+ teardown 生命周期 | 旗舰校准器 |
| **P2** | L0 Anchor（通用兜底）+ C 层 L1/L8/L7 增量追踪 | 覆盖无 layer-shell 环境。✅ 已完成（P2-a…P2-g） |
| **P3** | ~~L10 GradientField~~ **已否决**（§4.2） | 提案阶段实测推翻原设计 |
| **P4** | `fidus-test` 跨环境测试子项目方案（§8） | 先实现 `ci`，再 `live-host`，最后显式实验 `live-container`；脚本、专用 crate、容器按职责共存 |
| **P5** | 宿主 output mutation/recovery 方案（§8.7） | 最小显式 runner、无显示回归和保守 recovery 语义已实现；真实异常实验、稳定 identity 和跨机器验证仍有明确挂起，不得默认修改用户桌面 |

> **重要顺序**：先确立"零信任 + 概率池纯净"的架构约束，再写校准器代码。这与 v0.4 把 C shim 当 P3 的顺序**完全相反**。

---

## 10. 已删除 / 已降级项（相对 v0.4）

| 项 | 处理 | 理由 |
|---|---|---|
| A 层原生 API 封装 | **删除出核心** | §1 零信任原则 + §2 污染风险 |
| `OrthodoxOutcome` 接入协议（v0.5 提出） | **删除** | fidus 不应接收任何正道数据，含该协议 |
| L2.5 PointerProbe | **删除** | 逻辑自相矛盾（穿透 vs 接收输入） |
| L3 Beacon | 移出核心 → 规划中的 `fidus-extras`（尚未创建） | 场景过于特殊（摆花盆当参考站） |
| L5 TUIScan | 移出核心 → 规划中的 `fidus-extras`（尚未创建） | 纯 UX 包装，非通用库职责 |
| ~~L10 GradientField~~ | **已否决** | §4.2：原理自相矛盾，非风险未消 |

---

## 11. 待实机验证（开放问题）

以下问题**显式标注为未验证**，避免 v0.4 式的过度承诺：

1. ~~L10 对数螺旋网格的频域主频是否可稳定锁定？~~ — **已解答：不能**。实测锐度 3.4–6.8（规则光栅 250.9），k 越大越弥散。且这不是主要障碍——真正的障碍是整周期歧义。见 §4.2。
2. GNOME Mutter 的 layer-shell 支持度（版本门槛、行为差异）？ — **未验证**，缺 portal 后端
3. 校准后坐标系的**漂移速率**（决定定期校准的频率）？ — **未测**；正因未知，§4.5 不内建自动重校准计时器
3b. ~~**分数缩放下的稳健性**~~ — **已解决，且成因与缩放无关**：真正的成因是真实桌面上别的窗口自发重绘，破坏了差分检测"标记是唯一变化"的隐含前提。已由 §4.1「检测的三重判据」修复，实机 A/B：修复前 12/12 失败，修复后 0/30。分数缩放本身只带来 0.2–0.3px 的无害残差。
4. 多显示器/混合 DPI 下 `CoordinateFrame` 的表示与热插拔处理？ — **部分有策略**：Gate 在 `multi_monitor_count > 1` 时返回 `Degraded`（置信度 0.75/0.65），属"诚实降级"而非解决；表示与热插拔仍未定
5. 权限状态机在各平台的精确行为（尤其 macOS 重启需求）？ — **状态机已落地**（`PermissionState` 五态已参与 Gate 判决，含 `RequiresRestart`），但 **macOS 实机行为仍未验证**
6. P5 真实 host adapter、Niri parser adapter、signal supervisor 和异常恢复？ — **部分实现**；当前已有受控 runner、parser、signal/进程组监督、锁与恢复的无显示 fake 回归及本机正常路径实验，但真实 compositor 异常注入、稳定 identity 和跨机器验证仍未完成；NameOnly 身份不得报告 `recovery=confirmed`

> 第 2、3 条是真空白；3b 已定位成因待修；第 4、5、6 条已有诚实的降级/占位，但都**不等于已解决**。

---

## 12. 反直觉陷阱备忘（写代码前必读）

> 本节全部来自**实际踩坑或审查发现**，每条都附可复现的实测数字。
> 与 §4/§6 不同，这里记的不是"该怎么做"，而是**"看起来该这么做，但错了"**。
> 共同点：出事的量在直觉上是对的代理指标，实测却不是。

### 11.1 方差 ≠ 可定位性（P2-f，2026-09）

想当然的做法是用"模板够不够花哨"（方差 / 标准差）判断它能不能被追踪。**实测推翻了这个直觉**。

> **口径**（可复现，脚本见 `template.rs` 的 `localizability` 同款采样）：60×40 模板，自相似度 = 模板与自身平移 `(dx,dy)` 后在重叠区的 NCC；每个半径 `r` 取 `(r,0) (0,r) (r,r) (r,-r)` 四个方向的**最大值**。

| 模板 | 标准差 | r=2 | r=4 | r=8 | r=16 | 实际可定位性 |
|---|---|---|---|---|---|---|
| **线性渐变** | **73.6** | 1.000 | 1.000 | 1.000 | 1.000 | ❌ **完全无法定位** |
| 光栅 (fx=1,fy=2) | 60.0 | 0.980 | 0.924 | 0.738 | 0.195 | ✅ 周期但会衰减，可用 |
| 纯色块+细边框 | 97.5 | 0.822 | 0.711 | 0.692 | 0.648 | ✅ 弱纹理但可定位 |
| 哈希纹理 | 74.8 | 0.016 | 0.009 | 0.025 | 0.017 | ✅ 最佳 |

**渐变与哈希纹理的标准差几乎相同（73.6 vs 74.8），可定位性却是天壤之别（1.000 vs 0.025）**——方差对这件事**毫无分辨力**。而标准差最高的色块（97.5）也只是"可用"而非最佳。任何方差阈值不是没用就是反着的。

原因：NCC 分母大不代表峰**尖**。渐变沿其轴向平移不变，自相关恒为 1.000，于是 NCC 在一整条线上都给高分，报告的位置**是任意的**。而它下游长得和真实测量一模一样：满置信度进融合，把 Kalman 拽到虚构的位置，且**永不自我暴露**。

**正确判据是自相似度的绝对水平 + 高起步时的衰减**，代码里是两道独立关卡：

1. 任一半径的自相似度 **≥ 0.98** → 直接拒绝（渐变恒为 1.000，必被拦下）；
2. **仅当起步（r=2）> 0.9** 时，才额外要求 r=2→r=16 衰减 ≥ 0.1（光栅 0.980→0.195 轻松通过）。

第 2 条的 `> 0.9` 前提是**关键**：正因为有它，起步仅 0.822、衰减只有 0.174 的色块才**不被误杀**——低起步模板豁免衰减要求，因为它的绝对水平已经离 1.0 足够远，峰本身无歧义。

落地：`template::localizability()`，在 `register_target` 处**拒绝**而非追踪时才发现（§6.1 概率池只收真实测量 → 宁可注册失败，不可长期产出似是而非的位置）。

**逃生门（`UntrackablePolicy`）**：默认 `Refuse`；调用方可**显式**选择 `TrackWithReducedConfidence`，换取尽力而为的追踪。注意它**不是"跳过检查"开关**——那等于把洞重新打开。它做的是：

- 检查照常执行，歧义度照常实测；
- 测量照常产出，但置信度**上限按实测歧义度压低**（`1 - 自相似度`，下限 0.05）；
- 于是 L7 融合与 Kalman 自动降权，调用方也能从 `confidence` 看见降级。

这条守住了 §6.1 的实质：**池子里没有伪造的位置，只有被如实标注为"弱"的位置**。下限不取 0，因为"弱测量"与"无测量"（§4.5 丢失后报告先验）是两种不同的陈述，必须可区分。

折扣是**连续**的：接受/拒绝边界上不设断崖，否则刚好压线通过的模板会和优秀模板显示同样的可信度——那正是本节要防的"看似合理的值"。

> **通用教训**：为"质量"设阈值前，先测一遍**该指标与真正在意的属性是否单调相关**。20 行脚本就能问出来，而这类错误一旦上线，症状是"偶尔飘"，几乎无法归因。

### 11.2 面积 ≠ 能区分"合并 blob"（P2-f，2026-09）

L8 用面积上界 `AREA_MAX_FACTOR` 过滤 blob，注释声称它能拒绝"离场+到达合并"的 blob。**实测它两个方向都错**（60×40 目标，合并 blob 面积 = `(60+dx)×(40+dy)`）：

| 位移 | 面积比 | 旧上界 2.5× 的行为 |
|---|---|---|
| 2px | 1.08× | **漏网**（注释声称会拒绝） |
| 24px | 2.24× | **漏网** |
| 30px | 2.62× | **误杀**（这是真实位移，该测量） |
| 60px | 5.00× | 误杀 |

小位移（追踪循环里的常态）全部漏网，中等位移全部误杀，**L8 在它最该工作的区间失明**。

根因：面积**原理上**就分不开这两种情况——2px 位移的合并 blob 面积几乎等于静止 blob。于是不再假装能分：上界放宽到 4.2×（两个相接的模板区域上限 `2w×2h=4×`）**放行所有合并 blob**，歧义交给模板验证裁决。

### 11.3 "离预测多远" ≠ "尖峰是否被解释"（P2-f，2026-09）

修好 11.2 后暴露的更深的洞。L8 原用"blob 离预测位置 >4px"作为"是否发生位移"的代理，进而决定要不要信任模板验证。**慢速漂移下这个代理失效**：

- 目标每窗口漂移 2px → 对角距离 2.83px，落在 `IN_PLACE_TOLERANCE_PX=4` 内 → 判为"原地"
- 但有纹理的目标移动 2px 会重绘自身区域 **~92%** 的像素 → `R≈0.92` → 门控判 DYNAMIC → 丢弃

**终局推演**：漂移持续则 R 永不下降 → 门控永不 re-arm → 测量被永久丢弃，**尽管模板已经验证通过**。典型吸收态，无回落路径，L8 对"慢速拖拽"这一最常见场景永久静默。

**正确判据是几何而非统计**——两种情况在变化率相同时**形状不同**：

| 场景 | 合并 blob 尺寸（模板 60×40） |
|---|---|
| 漂移 2px | **62×42**（离场 ∪ 到达） |
| 漂移 5px | **65×45** |
| 原地动画 | **60×40**（恰好是足迹） |

**重绘不可能超出它所重绘的足迹**，所以超出部分就是"内容离开过原位"的证据。落地：`DEPARTURE_EXCESS_PX`。

### 11.4 "不接收键盘" ≠ "鼠标能穿透"（Wayland 原语，P1）

投射标记时，标记窗口**必须全程不截获用户输入**，否则校准会把用户的点击吞掉。直觉做法是设 `keyboard_interactivity = NONE`——但它**对 pointer 毫无作用**：

| 设置 | 作用 | 影响 pointer 穿透？ |
|---|---|---|
| `KEYBOARD_INTERACTIVITY_NONE` | 不接收键盘焦点 | ❌ **不影响** |
| `wl_surface_set_input_region(空)` | surface 不接收 pointer/touch | ✅ **唯一关键** |
| `LAYER_OVERLAY` | 层级最高 | 只影响渲染顺序 |

`input region` 的三态语义是另一个陷阱：`NULL` = **全表面可点**（这是默认值！），**空** region = 全穿透，矩形 = 局部可点。**把"没设置"当成"穿透"，标记窗口会吃掉校准期间的所有点击。**

同类还有 **anchor=0 的强制居中陷阱**：`anchor=0` 时 wlroots 强制把 surface 居中且 `margin` **完全不参与计算**——必须锚定 `TOP|LEFT`，margin 才是精确的"距左 x、距上 y"。fidus 投射标记的位置精度直接依赖这一点。

完整推导、协议时序（commit→configure→ack→attach）与 Q1–Q9 故障排查见 [`layer-shell-primer.md`](layer-shell-primer.md)。

### 11.5 通用模式

以上各条是同一个错误的不同形态，写新代码时自查：

1. **我用来判决的量，真的是我关心的那个属性吗？** 方差之于可定位性、面积之于合并、距离之于位移——三个都是似是而非的代理。
2. **这个代理在边界附近的行为，我实测过吗？** 三条全都是"典型情况下对，边界情况下反着"。
3. **判错的后果是"报错"还是"静默产出看似合理的值"？** 后者必须当 Critical 处理——§6.1 的概率池纯净性防的正是这个。
4. **注释里声称的过滤效果，我代入真实数字验算过吗？** 11.2 的注释写了两年，从来没成立过。
5. **这个 API 的名字，和它实际管的事情是同一件吗？** `keyboard_interactivity` 听起来该管输入，实际不管 pointer；`NULL` input region 听起来像"没有区域"，实际是"全部可点"。**名字是文档，不是契约。**
6. **我列出的风险，是真风险吗？** §4.2 把"动态壁纸频谱污染"当作 L10 的主要障碍列了两年——实测窄带相位估计对宽带噪声天然免疫，那根本不是障碍；真正的障碍（整周期歧义）**一次都没被写下来过**。**担心错了对象比不担心更危险**：它让人以为风险已经盘过了，从而不再去找真的那个。
7. **这段逻辑依赖什么没写下来的前提？** 差分检测依赖"标记是两帧间唯一的变化"，这个前提从未写进注释，也从未被检验——它在干净测试环境下成立，在有人正在使用的真实桌面上不成立，代价是 63% 的校准失败率。**前提不写下来，就没人能发现它失效了。**
8. **这个方案的各个部分，互相拆台吗？** L10 要"对数螺旋"（为消歧）又要"锁定主频"（为求解），而前者的实现方式恰恰摧毁后者。**组合方案要逐对检查相容性，不能只验证单件。**

---

## 附录 A · 设计原则速查

> **Zero-Trust Coordinate — 零信任坐标**
>
> 1. *Trust no platform API.* 不信任任何平台坐标 API。
> 2. *Build your own map.* 自带测绘工具，从零建立坐标系。
> 3. *Bootstrap from primitives.* 仅依赖最基础、最普适的合成器原语。
> 4. *Keep the pool pure.* 概率池只接收视觉与交互测量。
> 5. *Calibrate, then get out.* 校准完成立即销毁 overlay，不常驻。

这五条是 v0.5.1 的全部灵魂。任何新增功能若与之一冲突，**优先修改功能，不修改原则**。
§1.1 是这五条的展开，全项目引用的"原则 N"一律指此编号。

---

## 附录 B · 四条方法论（各层设计的思想根）

> 自 v0.4「核心设计哲学」抢救。五条原则规定**不可做什么**，这四条解释**为什么各层长成现在这样**——缺了它们，后人会把具体设计误当成随意选择。

**B1 · 边缘是唯一绝对零点**
> 屏幕物理边框永远存在、无需部署、不受合成器策略影响。这是全系统唯一可靠的全局坐标源——**所有校准器都锚定于它**，而非 compositor 施舍的逻辑窗口树坐标。这也是原则二"自建地图"能成立的物理前提。

**B2 · 信号必须带节拍**
> 动态壁纸能骗过单帧找色，**骗不过时序**。因此 L0 用"基线差分 ∧ 颜色"而非纯找色（静态同色块在差分中抵消），L8 用 500ms 窗口而非单帧判决。
> *推论*：任何"单帧即可确认"的检测都是可被背景伪造的；要求信号在**时间维度**上自证，干扰就只能让单次测量失效，无法伪造出一个错误的位置。

**B3 · 从被动观测到主动投射**
> 不再"找灯"，而是自己把灯点亮。L9/L0 的根本转变在于：fidus 不去场景中寻找恰好存在的特征，而是**投射自己已知位置的标记**再回读。前者的精度取决于运气，后者取决于我们自己的设计。

**B4 · 从离散锚点到连续场**
> 点给精度，场给冗余度。L0/L9 是离散锚点（少量高精度对应点），L10 是连续场（全屏每点都携带坐标信息）。二者不是替代关系——离散点在少量标记下就能解出精确映射，连续场则在部分区域被遮挡时仍可求解。

> **与 v0.4 的取舍**：v0.4 另有"把校准伪装成娱乐""渐进式精度""人是平行估计器"三条。前者是桌宠产品的 UX 策略，不是定位库的职责；后两条依赖 L2/L3 等已删除层。故不予保留。
