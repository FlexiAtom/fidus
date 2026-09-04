# fidus 定位引擎 · 草案 v0.5.1

> **零信任坐标 · 当窗口系统拒绝开口，你就自己把坐标系画在墙上**
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
| 2 | Locator 单层枚举（A/B/C 混在一起） | **三层分离，A 层真值不进 EKF** | 数学正确性 |
| 3 | 宣称"全平台通用，L9/L10 终局" | **仅在 layer-shell 可用时启用 L9/L10，覆盖范围诚实化** | Niri 实机文档 |
| 4 | A 层原生 API 作为 P0 核心 | **fidus 不封装任何原生 API**（详见 §1） | 概率池纯净性 |
| 5 | 业务项混在核心 | **L2.5 删除；L3/L5 移入 `fidus-extras`** | 职责清晰 |

---

## 1. 核心精神：Zero-Trust Coordinate（零信任坐标）

> **fidus 的立身之本，一句话：**
> *独立自主，自力更生*

### 1.1 三条不可动摇的原则

fidus 的独立性不是"兜底时的无奈"，而是**主动选择的设计立场**。它体现为三条架构铁律：

**原则一 · 零信任（Zero-Trust）**
> 不信任任何平台 API 返回的坐标值。即使 API 返回 `success`，即使它看起来合理——fidus 不以任何外部坐标作为自身状态的一部分（严防死守某些看起来合理的值，如0,0）。

**原则二 · 自给（Self-Sufficient）**
> fidus 的全部坐标能力，仅依赖**全平台最基础、最普适的合成器原语**（`wl_surface` / `wl_shm` / layer-shell、X11 基本绘制、Windows GDI、通用截屏）。不依赖任何窗口管理器的"配合"或"恩赐"。

**原则三 · 自举（Bootstrappable）**
> 在完全没有原生坐标 API 的环境中，fidus 精度依然可与有 API 的环境媲美。因为它建立的是**锚定在屏幕物理边缘的坐标系**，而非依赖 compositor 施舍的"逻辑窗口树坐标"。

### 1.2 一个干净的类比

可以把平台 API 想象成一张**别人掌握开关的地图**：有时给你、有时给你错的（GNOME 下 `win.xChanged` 永远返回 `(0,0)`）、有时干脆不触发。fidus 的选择不是"反复恳求对方给地图"，而是——**自己带测绘工具，从零丈量出整片土地**。

这个类比只服务于一个技术论点：**为什么 fidus 不封装原生 API**。它不延伸、不外推，项目中不出现任何政治化表述。fidus 的精神内核是**技术自主**，用架构原则和代码来保证，而不是口号。

### 1.3 这条精神如何指导每一个决策

| 决策 | 由哪条原则推导 |
|---|---|
| fidus 不封装 `GetWindowRect` / `CGWindowList` / `XQueryTree` | 原则一（零信任）+ 原则三（避免成为 API 门面） |
| 概率池只接收视觉/交互测量 | 原则一 |
| L9 仅依赖 layer-shell 基础能力 | 原则二 |
| 校准器建立物理坐标系而非逻辑坐标 | 原则三 |
| `fidus-core` 公开 API 不含任何坐标类 native 输入 | 原则一（类型层面强制） |

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
┌─────────────────────────────────┐
│                    fidus-core（本仓库）                        │
│                                                              │
│ ────────────┐ ┌───────────────┐    │
│  │  B · Calibrator    │    │  C · Estimator (概率池)   │    │
│  │  L9 Crosshair      │    │  L1 Fingerprint           │   │
│  │  L10 GradientField │───▶│  L8 EdgeSync         │    │
│  │  L0 Anchor         │    │  L4 相对位移 (运动模型)  │    │
│  └───────────┘└───────────────┘    │
│           │                        │                       │
│           ▼                        ▼                       │
│      CoordinateFrame      ProbabilisticPosition                 │
│                                                             │
│  ┌───────────────────────────┐     │
│  │  Gate · 能力探测（§5）：哪些情况不能用              │     │
│  └───────────────────────────┘     │
└────────────────────────────────┘

  ★ 明确排除：任何原生 API 门面（Windows / macOS / X11 / Portal）
     → 由调用方在 fidus 之外自行管理（见 §3.4）
```

### 3.1 B 层 · Calibrator（校准器）

**职责**：在初始化 / 定期校准时，**一次性**建立 `CoordinateFrame`。
**特性**：校准完成后**立即销毁 overlay surface**，不常驻。

- **L9 Crosshair**（旗舰）：基于 layer-shell 投射已知位置标记，视觉反推映射
- **L10 GradientField**（实验性，feature-gated）：频域编码全场坐标
- **L0 Anchor**（通用兜底）：1×1 像素找色，X11/Windows 可用

### 3.2 C 层 · Estimator（估计器）

**职责**：校准成功后，在稳态下做**增量追踪**，输出带置信度的 `ProbabilisticPosition`。
**输入**：仅视觉与交互测量（见 §6 白名单）。

- L1 Visual / L6 Fingerprint
- L8 EdgeSync（低频边缘差分）
- L4 相对位移（拖动的运动模型）

### 3.3 Gate · 能力探测

**职责**：回答"当前环境能不能用 fidus / 该走哪条路径"，作为一等公民接口（详见 §5）。

### 3.4 调用方的职责（fidus 之外）

```
启动:
  rect = 调用方自己调 native API        // fidus 不参与
  if rect 可信:                          // X11/Windows 上通常可信
      return rect                        // 快速通道，不经过 fidus
  else:
      // Wayland 下 API 返回 (0,0)/不触发/不可信 → 才实例化 fidus
      let engine = FallbackEngine::new(env);   // env 只描述环境，不含坐标
      engine.calibrate();
      loop { use engine.estimate(); }
```

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

> 完整验证代码与 PyQt 集成方案见附件《Niri 点击穿透实现路径.md》，此处不再重复。

### 4.2 L10 GradientField · 实验性（feature-gated）

**定位**：仅在启动时运行一次的校准手段，**绝不常驻**。

**已知风险**（需实机验证，未承诺可用）：
- 对数螺旋网格的频域峰值展宽，FFT 主频难锁定
- 动态壁纸/视频背景的频谱污染
- 持续全屏重绘的 CPU/电量代价（因只在校准时运行，已大幅缓解）
- 用户可见的动画网格干扰（一次性，可接受）

**结论**：默认 `cfg(feature = "gradient-field")` 关闭，仅作为研究项。

### 4.3 L0 Anchor · 通用兜底校准器

创建 4 个 2×2 像素纯色方块（颜色取高饱和冷门色），锚定虚拟屏幕四角。每次校准随机打乱“颜色→角点”的映射并立即销毁重建，至少通过 2 轮独立采样验证映射一致性，并结合几何矩形约束确认，方可输出 CoordinateFrame。

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

- 显示器热插拔 / 分辨率变化
- DPI 缩放因子变化
- 置信度 `< 0.4` 持续一段时间
- 系统空闲超过阈值（如 30 分钟）
- 调用方显式请求

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
    pub is_dynamic_wallpaper: Option<bool>,
    pub multi_monitor_count: usize,
    pub compositor_type: CompositorKind,   // Niri / Sway / KWin / Mutter / ...
    pub screen_capture_permission: PermissionState,
    pub wayland_input_region_supported: bool,
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

## 6. 概率池纯净性（架构铁律）

### 6.1 输入白名单

| 输入源 | 进池？ | 理由 |
|---|---|---|
| L9 Crosshair 视觉测量 | ✅ | 纯视觉，噪声可建模 |
| L10 GradientField | ✅ | 纯视觉 |
| L1 Fingerprint / L6 | ✅ | 纯视觉 |
| L8 EdgeSync BBox | ✅ | 纯视觉 |
| L4 拖动位移 | ✅ | 运动模型 |
| **任何 native API 返回值** | ❌ **类型层面禁止** | 来源不可信、量纲不可比、污染池 |

L8 EdgeSync 输入门控（L8 Gate）
在每次将 L8 的 BBox 测量送入 EKF 之前，必须通过 MotionGate 检查：
以 500ms 为窗口执行帧间差分，计算目标区域像素变化率 R。
若 R > max(baseline * 3, 15%)，判定目标内容为 DYNAMIC，该帧测量弃用，L8 进入等待状态。
连续 2 帧通过门控后，L8 才重新输出测量值。
此 Gate 不属于原生 API 门面，纯粹基于帧差分的视觉计算，与零信任原则严格一致。


### 6.2 类型层面强制

`fidus-core` 的公开 API **不含任何坐标类 native 输入**：

```rust
pub struct FallbackEngine {
    calibrator: Box<dyn Calibrator>,   // L9/L10/L0
    estimator: Box<dyn Estimator>,      // L1/L8 + EKF
}

impl FallbackEngine {
    // env 只描述环境（§5.2），不含任何坐标值
    pub fn new(config: EnvironmentContext) -> Result<Self, InitError>;

    pub fn calibrate(&self) -> Result<CoordinateFrame, CalibrationError>;
    pub fn estimate(&self) -> ProbabilisticPosition;   // 池内仅视觉/交互
}
```

**若有人在内部试图混入 native 坐标，编译器应阻止**——这是零信任原则在类型系统上的落地。

---

## 7. crate 拆分

```
fidus-core          // 类型系统 + 三层 trait + 概率池 + 坐标空间
fidus-backend-*     // （仅适配最基础合成器原语，非 API 门面）
                      ├─ wayland-layer   (wlroots/KDE)
                      ├─ wayland-portal  (GNOME)
                      ├─ x11
                      └─ windows-gdi
fidus-calibrate     // L0 Anchor / L9 Crosshair / L10 Field (feature-gated)
fidus-estimate      // L1/L6/L8 + EKF
fidus-extras        // 业务特定项：L3 Beacon / L5 TUIScan（移出核心）
```

> 注：`fidus-backend-*` 适配的是**基础绘制/截屏原语**，不是"读取窗口坐标"的 API——后者一律不封装。

---

## 8. 实施优先级（P0 → P3）

| 阶段 | 内容 | 说明 |
|---|---|---|
| **P0** | §1 精神内核 + §6 概率池纯净性 + §5 能力探测接口 | **架构地基，先于一切代码** |
| **P1** | L9 Crosshair（Niri 实机验证）+ teardown 生命周期 | 旗舰校准器 |
| **P2** | L0 Anchor（通用兜底）+ C 层 L1/L8 增量追踪 | 覆盖无 layer-shell 环境 |
| **P3** | L10 GradientField（feature-gated，实验性） | 仅研究，不承诺默认启用 |

> **重要顺序**：先确立"零信任 + 概率池纯净"的架构约束，再写校准器代码。这与 v0.4 把 C shim 当 P3 的顺序**完全相反**。

---

## 9. 已删除 / 已降级项（相对 v0.4）

| 项 | 处理 | 理由 |
|---|---|---|
| A 层原生 API 封装 | **删除出核心** | §1 零信任原则 + §2 污染风险 |
| `OrthodoxOutcome` 接入协议（v0.5 提出） | **删除** | fidus 不应接收任何正道数据，含该协议 |
| L2.5 PointerProbe | **删除** | 逻辑自相矛盾（穿透 vs 接收输入） |
| L3 Beacon | 移入 `fidus-extras` | 场景过于特殊（摆花盆当参考站） |
| L5 TUIScan | 移入 `fidus-extras` | 纯 UX 包装，非通用库职责 |
| L10 GradientField | feature-gated 实验 | §4.2 风险未消 |

---

## 10. 待实机验证（开放问题）

以下问题**显式标注为未验证**，避免 v0.4 式的过度承诺：

1. L10 对数螺旋网格的频域主频是否可稳定锁定（动态壁纸下）？
2. GNOME Mutter 的 layer-shell 支持度（版本门槛、行为差异）？
3. 校准后坐标系的**漂移速率**（决定定期校准的频率）？
4. 多显示器/混合 DPI 下 `CoordinateFrame` 的表示与热插拔处理？
5. 权限状态机在各平台的精确行为（尤其 macOS 重启需求）？

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
