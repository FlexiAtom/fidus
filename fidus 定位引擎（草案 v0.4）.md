fidus 定位引擎（草案 v0.4）

桌宠 MeaPet 底层定位引擎 · 纯 Rust 核心 · 全平台通用
定位策略：视觉 / 交互 / 数学求解 / 主动投射

修订记录

版本 日期 变更
v0.1 2026-08-30 初稿：7 个定位 Level（Anchor/Visual/Interaction/Beacon/Relative/Fingerprint/Fusion）
v0.2 2026-09-02 加入 PointerProbe（L2.5）、EdgeSync（L8，动态壁纸检查+像素点闪烁+鼠标点击联动）、RelativeLocator 增加 EdgeClamp 约束
v0.3 2026-09-03 加入 CrosshairLocator（L9 十字线主动投射）；基于 Niri layer-shell 验证文档确立 C shim 架构
v0.4 2026-09-03 加入 GradientFieldLocator（L10 渐变网格场），全屏空间频率编码；融合分析确立"主动投射坐标系"终局形态

一、问题背景

平台 传统定位 API 状态
Wayland (niri/sway/hyprland) geometry() / globalPos() / moveEvent ❌ 全线红灯
X11 xdotool / Xlib ⚠️ 仅限 X11
Windows GetWindowRect ⚠️ DPI 缩放偏移
macOS 沙盒限制 / 私有 API 失效 ⚠️ 待验证
鸿蒙 Next 暂无统一方案 ⚠️ 待验证

核心结论：不存在跨平台可直接读取窗口绝对坐标的 API。fidus 必须构建一套与平台无关的"自定位系统"——像人一样用眼睛看、用手点、靠边缘判断，并且有朝一日自己把坐标轴画在墙上。

Niri 验证成果（2026-08-30 实机确认）：Wlroots 系 compositor 上，通过 
"zwlr_layer_shell_v1" + 
"overlay" 层 + 空 input region，可创建一个位置精确可控、输入完全穿透、永远置顶的 layer surface。这为"主动投射定位标记"提供了可靠底座。

二、核心设计哲学

1. 不信任平台 API     → 用视觉和交互自己测算
2. 不依赖单一手段     → 多层冗余，任意层失败降级
3. 把校准伪装成娱乐   → 用户以为在玩，实际建坐标系
4. 渐进式精度         → 约束越多系统越准
5. 纯数学求解         → 零平台依赖，全部经典轮子
6. 人是平行估计器     → 人和算法面对同一份观测，各自独立求解，结果融合
7. 边缘是唯一绝对零点 → 屏幕边框永远存在，零部署的全局坐标源
8. 信号必须带节拍     → 动态壁纸能骗过单帧找色，骗不过时序同步
9. 从被动观测到主动投射 → 不再"找灯"，而是自己把灯点亮
10. 从离散锚点到连续场 → 点给精度，场给冗余度

三、总体架构

┌─────────────────────────────────────────────────────────┐
│                     业务层（特化包）                       │
│  ┌──────────┐ ┌──────────────┐ ┌────────────────────┐   │
│  │ fidus-pet│ │ fidus-overlay │ │ fidus-xxx (社区)   │   │
│  └──────────┘ └──────────────┘ └────────────────────┘   │
├─────────────────────────────────────────────────────────┤
│                     核心层 fidus                         │
│  ┌───────────────────────────────────────────────────┐  │
│  │                  Locator trait                     │  │
│  │  L0  Anchor        L1  Visual                      │  │
│  │  L2  Interaction   L2.5 PointerProbe               │  │
│  │  L3  Beacon        L4  Relative (+EdgeClamp)       │  │
│  │  L5  TUIScan       L6  Fingerprint                 │  │
│  │  L7  Fusion        L8  EdgeSync                    │  │
│  │  L9  Crosshair     L10 GradientField               │  │
│  ├───────────────────────────────────────────────────┤  │
│  │  ScreenClassifier     (动态壁纸全局检查)             │  │
│  │  ScreenState          (屏幕/截屏/缩放状态)           │  │
│  │  MarkerManager        (layer-shell 标记管理器)      │  │
│  ├───────────────────────────────────────────────────┤  │
│  │  FFI (C/Python/Node.js) / 平台适配层                │  │
│  └───────────────────────────────────────────────────┘  │
└─────────────────────────────────────────────────────────┘

四、Level 全集

Level 0：四角哨兵锚点 | 
"AnchorLocator"

灵感：视觉 Fiducial Marker。

屏幕四角各放一个 1×1 像素、置顶、穿透的窗口（红/绿/蓝/黄）。截屏找色 + 仿射变换 → 反推截屏与屏幕映射。

v0.4 状态：保留作为 X11/Windows 快速校准手段；在支持 layer-shell 的 Wayland 平台，逐渐被 L9/L10 取代（标记层投影更优）。

Level 1：视觉定位 | 
"VisualLocator"

灵感：人眼定位。

目标上叠加 MarkerOverlay（8px 圆点），动作切换时闪现 → 截屏 → 找色 + 三角形校验。

v0.4 升级：

- 支持闪烁节拍模式：T0 红，T1 白，T2 红。帧差分掩码 + 互相关识别。动态壁纸下的粒子无法同步节拍，标记锁定从"颜色唯一"升级为"时序唯一"。
- 支持频域滤波辅助：与 L10 融合时，通过网格频段滤除背景，只保留桌宠轮廓。

Level 2：用户校准 | 
"HumanInteractionLocator"

点击/拖拽时通过 
"globalPos()"（若可用）记录坐标，作为最高精度锚点。人即传感器：每一次点击都是一次校准观测。

v0.4 升级：与 L10 涟漪模式融合——拖拽时网格产生视觉波纹反馈，涟漪中心的变化轨迹作为 odometry 输入，不依赖鼠标全局坐标。

Level 2.5：鼠标探针 | 
"PointerProbeLocator"

1px 小窗跟随鼠标，当鼠标悬停探针窗口上时：鼠标全局坐标 = 探针窗口绝对坐标。仅在平台支持鼠标全局坐标时启用，否则自动禁用。

v0.4 升级：与 L9 十字线融合——让十字交点移动到当前鼠标位置，通过检测十字线的实际像素位置与预期位置差，建立鼠标全局坐标的可信度评估。

Level 3：摆件系统 | 
"BeaconLocator"

用户摆花花盆/小船/植物，每个摆件是已知坐标参考站。截屏测距 → 多边定位（最小二乘）。摆件 ≥3 精度高。

v0.4 升级：L9 十字线辅助部署——系统在预设位置显示十字线，用户将摆件拖到十字线上即完成高精度注册，创建绝对锚点。用户以为在玩放置游戏，实际在部署 GPS 卫星。

Level 4：闭环控制 | 
"RelativeLocator"

以屏幕中心为原点，所有后续位置为相对偏移。约束优化（Gauss-Newton）求解。

约束类型：

enum Constraint {
    Distance { from: String, to: String, dist: f64 },       // 圆约束
    Displacement { node: String, delta: (f64, f64) },      // 向量约束
    Absolute { node: String, coord: (f64, f64) },          // 绝对点锚定
    EdgeClamp { node: String, edge: ScreenEdge, width: f64 }, // 边界钳制
    FrequencyPhase { node: String, phase: f64 },           // L10 相位硬同步
}

贴边刷新（Edge Snap Refresh）：用户久不拖桌宠 → 系统自动吸附到屏幕底边 → 记录绝对坐标 → 优雅归位。每次贴边刷新免费完成一次全局重定位。

Level 5：TUI 扫描动画 | 
"TUIScanOverlay"

定位计算过程伪装成"切换动作时的加载动画"。v0.4 升级：动画开始强制触发 L9/L10 校准，让"每次切换动作"都自动完成定位校准——用户看到的是酷炫动画，系统看到的是校准时机。

Level 6：桌面像素指纹 | 
"FingerprintLocator"

截屏周围像素与指纹库匹配输出坐标。v0.4 升级：指纹只描边不填充——匹配"目标 BBox 边缘 10px 宽度局部特征"。动态壁纸下中央变化剧烈，屏幕最外缘特征相对稳定。

Level 7：多源贝叶斯融合 | 
"FusionEngine"

EKF / 粒子滤波，输入所有可用 Locator 的输出：

Anchor (L0)       ──┐
Visual (L1)       ──┼──→ EKF / 粒子滤波 ──→ Location
Interaction (L2)  ──┤
Beacon (L3)       ──┤
PointerProbe      ──┤
EdgeSync (L8)     ──┤
Crosshair (L9)    ──┤
GradientField(L10)──┘

Level 8：边缘协同定位 | 
"EdgeSyncLocator"

灵魂：把屏幕边缘当绝对零点，把闪烁标记当信号暗号，把鼠标点击当触发扳机。

ScreenClassifier：动态壁纸检查

截两张相隔 200ms 全屏图做时域差分（裁剪中心 84% 排除任务栏），差分比例 > 15% → 动态背景。结果注入全局状态，所有 Locator 据此调整策略：

- 
"bg_dynamic = false" → 边缘 BBox 可独立定位，置信度 0.8~0.99。
- 
"bg_dynamic = true" → 强制联动模式（闪烁节拍 + 鼠标点击），置信度 0.9~0.95。

联动一：边缘BBox + 像素点闪烁

标记以固定节拍闪烁（红→白→红），帧差分掩码确认标记；只有符合节拍的像素簇才是标记。动态壁纸粒子无法伪造节奏。

联动二：边缘BBox + 鼠标点击

鼠标按下即触发截屏，找到标记 BBox → 直接得到绝对坐标。用户的物理点击与系统视觉锁定形成双向确认。

Level 9：十字线主动投射 | 
"CrosshairLocator"

核心思想：不再"找灯"，而是自己把灯点亮。

通过 layer-shell 创建一个 overlay 层十字线：

- 两条 1px 宽、贯穿全屏的横线/竖线（推荐细长双 surface，避免全屏大 buffer）。
- 空 input region → 完全穿透。
- 锚定 TOP|LEFT + margin 精确控制交点位置。

定位公式：

已知：十字交点 margin 值 (mx, my)（相对 usable area）
检测：截屏中十字交点像素位置 (x_hat, y_hat)（直接用行列扫描找同色连续像素链）
推导：可用区域 → 全屏偏移量 = (x_hat - mx, y_hat - my)
实际应用：桌宠中心与十字交点重合 → 桌宠绝对坐标 = (x_hat, y_hat)

意义：

- 十字线是系统主动投射的已知几何参照物，视觉检测变成"验证自己放的标记在哪"，而非"在未知世界里找线索"。
- 抗动态壁纸：贯穿全屏的连续直线是无法被随机粒子伪造的强几何特征。
- 十字线作为 L4 的"绝对重置按钮"——漂移过大时自动触发一次校准。

Level 10：渐变网格场 | 
"GradientFieldLocator"

核心思想：整张屏幕画满坐标系。

通过 layer-shell 投射一个渐变网格 overlay：

- 横向线条透明度从左 100% 渐变到右 0%。
- 纵向间距呈对数螺旋扩张（中心密、边缘疏）。
- 以恒定速度整体平移，形成动画。

数学暴力：对截屏做 2D FFT，网格在频域中形成明确的峰值。锁定主频与相位后：

主频 f_main → 求解缩放因子（DPI 补偿）
相位角 θ    → 求解偏移量（截屏 ↔ 屏幕映射）
倾斜角      → 求解旋转量（锚定纠正）

三大融合诡计：

1. 频域滤波抹除器（× L1 Visual）网格与桌宠具有完全不同的空间频率特征。截屏 → FFT → 滤除网格频率成分 → 剩余图像中桌宠轮廓如黑白剪影般清晰。动态壁纸粒子在频域滤波前灰飞烟灭，置信度 0.99。
2. 涟漪引力场（× L2 Interaction）拖拽桌宠时网格产生水波纹反馈。用户看到的是酷炫 UX；系统看到的是涟漪中心的连续位移轨迹（odometry），替代鼠标全局坐标输入。
3. 相位同步计时器（× L4 Relative）网格以 1Hz 周期完整滚动。每完成一个周期，相位归零帧必然与初始帧逐像素一致。系统可利用"相位零"帧作为绝对基准，强制重定位全部相对坐标。每次动画循环 = 一次免费 GPS 校准。

五、回退链（v0.4 完整版）

                    ┌──────────────────────────┐
                    │     动态壁纸检查           │
                    └────────────┬─────────────┘
             动态                 │               静态
    ┌────────────────────────────┴────────────────────────────┐
    │                                                         │
┌───▼────────────────────┐              ┌───────────────────▼──────┐
│ EdgeSync 联动模式       │              │ EdgeSync 直接模式          │
│ (闪烁节拍 + 鼠标点击)    │              │ (边缘BBox + 鼠标点击)       │
│ 置信度 0.9~0.95        │              │ 置信度 0.8~0.99           │
└────────────────────────┘              └───────────────────────────┘
                    ↓ 失败                          ↓ 失败
┌───────────────────────────────────────────────────────────────────┐
│ Crosshair (L9) / GradientField (L10) —— 主动投射校准                │
│ layer-shell 就绪时为首选；失败则跳过                                  │
└───────────────────────────────────────────────────────────────────┘
                    ↓ 失败
┌───────────────────────────────────────────────────────────────────┐
│ Level 0-3 经典回退链                                               │
│ Beacon(≥3) → Beacon(=2) → Anchor+Interaction → Anchor+Visual      │
└───────────────────────────────────────────────────────────────────┘
                    ↓ 失败
┌───────────────────────────────────────────────────────────────────┐
│ RelativeLocator + EdgeClamp + 贴边刷新                              │
└───────────────────────────────────────────────────────────────────┘
                    ↓ 失败
┌───────────────────────────────────────────────────────────────────┐
│ FingerprintLocator → 回到屏幕中心 + 提示用户"点我一下"                │
└───────────────────────────────────────────────────────────────────┘

六、文件结构（v0.4）

fidus/
├── README.md                  # 攻略总目录（Diátaxis 结构，外链 docs/）
├── docs/
│   ├── tutorials/             # 教程
│   │   ├── getting-started.md
│   │   ├── calibration-wizard.md
│   │   └── action-animation.md
│   ├── how-to-guides/         # 指南
│   │   ├── add-new-locator.md
│   │   ├── debug-locator.md
│   │   └── cross-platform.md
│   ├── reference/             # 参考
│   │   ├── locator-trait.md
│   │   ├── levels.md
│   │   └── config.md
│   └── explanation/           # 概念
│       ├── philosophy.md
│       └── fusion-strategy.md
├── src/
│   ├── lib.rs
│   ├── locator.rs             # Locator trait
│   ├── state.rs               # ScreenState / 全局配置
│   ├── estimator/
│   │   ├── mod.rs
│   │   ├── anchor.rs          # L0
│   │   ├── visual.rs          # L1（闪烁节拍）
│   │   ├── interaction.rs     # L2
│   │   ├── pointer_probe.rs   # L2.5
│   │   ├── beacon.rs          # L3
│   │   ├── relative.rs        # L4（EdgeClamp/FrequencyPhase）
│   │   ├── tui_scan.rs        # L5
│   │   ├── fingerprint.rs     # L6（边缘指纹）
│   │   ├── edge_sync.rs       # L8 + ScreenClassifier
│   │   ├── crosshair.rs       # L9
│   │   ├── gradient_field.rs  # L10
│   │   └── fusion.rs          # L7
│   ├── marker/
│   │   ├── mod.rs
│   │   ├── crosshair.rs       # 十字线 overlay 逻辑
│   │   ├── gradient_field.rs  # 渐变网格 overlay 逻辑
│   │   └── layer_shell_shim.c # C shim（已验证代码）
│   ├── platform/
│   │   ├── mod.rs
│   │   ├── wayland.rs
│   │   ├── x11.rs
│   │   ├── windows.rs
│   │   └── macos.rs
│   ├── math/
│   │   ├── affine.rs
│   │   ├── trilateration.rs
│   │   ├── optimize.rs        # Gauss-Newton + 约束
│   │   ├── fft.rs             # 频域分析
│   │   └── frequency.rs       # 相位/主频提取
│   ├── io/
│   │   ├── screenshot.rs
│   │   └── config.rs
│   └── ffi/
└── tests/

七、核心数据流

用户操作 / 系统触发
        │
        ▼
┌──────────────┐  截屏(多帧/单帧)  ┌──────────────┐
│  MarkerManager│ ────────────────▶ │  ScreenState  │
│ layer-shell  │                   │  (原始像素)    │
└──────────────┘                   └──────┬───────┘
                                          │
                    ┌─────────────────────┼─────────────────────┐
                    ▼                     ▼                     ▼
           ┌──────────────┐    ┌─────────────────┐    ┌──────────────────┐
           │ ScreenClassifier│  │ FFT / 行列扫描  │    │ 找色/差分掩码     │
           │ 动态壁纸判断    │  │ 主频/相位提取   │    │ 连通域/边缘BBox    │
           └──────────────┘    └─────────────────┘    └──────────────────┘
                    │                     │                     │
                    ▼                     ▼                     ▼
           ┌─────────────────────────────────────────────────────────┐
           │              各 Locator 独立求解                         │
           │  Crosshair  → 绝对坐标 (高置信)                          │
           │  Gradient   → 变换矩阵 + 过滤后的桌宠BBox                │
           │  EdgeSync   → 边缘距离 + 标记确认                        │
           │  ...                                                     │
           └───────────────────────────┬─────────────────────────────┘
                                       ▼
                              ┌─────────────────┐
                              │  Fusion (L7)    │
                              │  EKF/粒子滤波    │
                              └────────┬────────┘
                                       ▼
                              ┌─────────────────┐
                              │  最终 Location   │
                              │ (x, y, conf)     │
                              └─────────────────┘

八、跨平台适配

能力 X11 Wayland (wlroots) Windows macOS
layer-shell ❌ ✅ ❌ ❌
全局鼠标坐标 ✅ ⚠️ 受限 ✅ ✅
截屏 ✅ ✅ ✅ ⚠️ 屏录权限
窗口绝对坐标 API ✅ ❌ ⚠️ DPI ❌
推荐路径 Classic (L0-4) L9/L10 优先 Classic + PointerProbe Classic + Visual 强化

Layer-shell 覆盖检测：运行时通过 
"WAYLAND_DISPLAY" 判定 Wayland → 尝试绑定 
"zwlr_layer_shell_v1" → 成功则启用 L9/L10 全家桶；失败或 X11 回退到传统方案。

九、里程碑 P0 - P7（v0.4 修订）

里程碑 内容 依赖
P0 Rust 工程骨架 + Locator trait + 截屏抽象 无
P1 Anchor/Visual/Interaction 三板斧，静态壁纸下可定位 P0
P2 Level 4 闭环控制系统（约束求解 + EdgeClamp） P1
P3 C shim layer-shell + MarkerManager（交叉影线） P0
P4 CrosshairLocator + 动态壁纸检查 + 闪烁节拍 P3
P5 EdgeSyncLocator（鼠标点击联动 + 像素点闪烁） P4
P6 GradientFieldLocator（FFT 频域 + 相位同步 + 涟漪互动） P5
P7 FusionEngine 全部接入 + 跨平台矩阵完善 + 桌宠特化整合包 fidus-pet P6

十、v0.4 总结

1. L0-L6 是被动观测，不断逼近绝对坐标。
2. L8 利用天然存在的屏幕边缘，获得了零部署的绝对零点。
3. L9 首次实现主动投射，验证"自己放标记，然后读自己"的哲学转变。
4. L10 把"点"升级为"场"，把视觉检测升级为频域测量——动态壁纸、粒子特效、复杂背景全部在 FFT 面前溃败。

*邪修之路，永无止境*