> # ⚠️ 历史存档 · 请勿据此实现
>
> **本文是整个项目的最初草案，站在「メア桌宠」视角写成，早于 fidus 独立成库。**
> 当前规范是 [`docs/spec.md`](../spec.md)。
>
> **整体过时**：技术栈为 Qt-Python + OpenCV + numpy（fidus 是纯 Rust）；依赖 `globalPos()`（**违反零信任**）；含桌宠专用 UX 与 `meapet/` 目录结构；无零信任、无概率池纯净性、无 A/B/C 三层概念。
>
> **仍有独特价值的部分**（本文是唯一记载）：
> - **Trilateration 最小二乘解法**：两锚点消歧 + 3 锚点 lstsq 的完整代数——若 L3 Beacon 将来进 `fidus-extras`，这是唯一实现参考。
> - **五平台能力矩阵**：含**鸿蒙**，其它文档均未提及。
>
> 本文对 Windows DPI / macOS 沙盒问题的记录，已被 `README.md` 的实机取证节以更强形式覆盖。
>
> *归档于 P2-g（2026-09）。*

---

メア桌宠全平台定位统一兜底方案（草案）

核心理念：放弃与 Wayland / 多桌面环境（WM）的窗口坐标 API 死磕，转而构建一套完全不依赖平台私有接口、纯视觉 + 用户交互 + 数学求解的通用定位底座。
任何平台（Windows / X11 / Wayland / macOS / 鸿蒙）均可运行，精度随用户使用时间单调递增。

一、问题背景

场景 痛点
Wayland (niri/sway 等) 无全局窗口坐标 API，穿透模式（
"WA_TransparentForMouseEvents"）下 Qt 窗口 
"geometry()" 返回虚假值
X11 可用 
"xdotool" / Xlib，但 Wayland 下被彻底封锁
Windows 
"GetWindowRect" 可用，但多屏 DPI 缩放会偏
macOS 沙盒限制，私有 API 随时可能失效
多屏 / HiDPI 截屏坐标 ≠ 屏幕坐标，DPI 缩放导致像素映射混乱

结论：不存在一个全平台通用的"直接读取窗口绝对坐标"的 API。必须另辟蹊径。

二、核心设计哲学

┌──────────────────────────────────┐
│                    邪修定位系统总纲                                │
│                                                                    │
│  1. 不信任平台 API → 用视觉和交互自己测                            │
│  2. 不依赖单一手段 → 多层冗余，任何一层挂了自动降级                │
│  3. 把校准伪装成娱乐 → 用户以为在玩，实际在建坐标系                │
│  4. 渐进式精度 → 约束越多，系统越准，越用越聪明                    │
│  5. 纯数学求解 → 零平台依赖，numpy + OpenCV 即可               │
└──────────────────────────────────┘

三、方案合集

Level 0：四角哨兵锚点（屏幕坐标系校准）

灵感：计算机视觉中的 Fiducial Marker（ARtag / AprilTag）

原理：在屏幕四角各创建一个 1×1 像素、永久置顶、穿透点击 的独立 Qt Widget，颜色分别为红/绿/蓝/黄。截屏后通过 OpenCV 找色定位这四个锚点，反推截屏坐标系与屏幕坐标系的映射关系（仿射变换）。

class CornerSentinel(QWidget):
    """屏幕四角 1px 哨兵窗口"""
    def __init__(self, pos: str, color: tuple):
        super().__init__()
        self._color = QColor(*color)
        self.setWindowFlags(Qt.FramelessWindowHint | Qt.WindowStaysOnTopHint | Qt.Tool)
        self.setAttribute(Qt.WA_TransparentForMouseEvents)
        self.resize(1, 1)
        # 移动到屏幕四角之一
        ...
    
    def flash(self, duration_ms=200):
        """切换动作时闪白 → 截屏中更容易找色"""
        self._color = QColor(255, 255, 255)
        self.repaint()
        QTimer.singleShot(duration_ms, self._restore)

优势 说明
永远可见 独立 surface，不依赖桌宠窗口是否渲染
坐标已知 创建时即知道理论屏幕坐标
解决映射 截屏中锚点位置 ↔ 理论位置 → 算出缩放/偏移
跨平台 1px 窗口在所有平台均可创建

Level 1：视觉定位（方案 C — MarkerOverlay + grim + OpenCV）

原理：在桌宠窗口上叠加一个透明 
"MarkerOverlay"，绘制 8px 纯色圆点标记。切换动作时 
"show()" 标记 → 闪现 1 秒 → 
"hide()" 前 grim 截全屏 → OpenCV 找色 + 三角形几何校验 → 推算桌宠屏幕坐标。

class MarkerOverlay(QWidget):
    """桌宠上的视觉标记层"""
    def __init__(self, parent):
        super().__init__(parent)
        self.setAttribute(Qt.WA_TransparentForMouseEvents)
        self.setWindowFlags(Qt.FramelessWindowHint | Qt.Tool)
    
    def paintEvent(self, event):
        painter = QPainter(self)
        # 在桌宠的 4 个角画纯色标记
        colors = [(255,0,0), (0,255,0), (0,0,255), (255,255,0)]
        for i, (r,g,b) in enumerate(colors):
            painter.setBrush(QColor(r,g,b))
            painter.drawEllipse(i*20, i*20, 8, 8)

关键：标记在 
"self.hide()" 前 1 秒闪现即可被 grim 截到，无需进入 layer-shell 渲染管线。

Level 2：用户校准（方案 D — 点击 / 拖拽记录）

原理：用户每次点击或拖拽桌宠，
"mousePressEvent" / 
"mouseReleaseEvent" 中记录 
"globalPos()" → 直接获得屏幕绝对坐标。这是精度最高、延迟最低的定位手段。

def mousePressEvent(self, event):
    self._drag_start_global = event.globalPos()

def mouseReleaseEvent(self, event):
    self._last_known_pos = event.globalPos()
    self._save_pet_position(event.globalPos().x(), event.globalPos().y())

天然校准窗口期：用户点击切换动作时，正是记录坐标的最佳时机——用户以为在操作，实际在为你提供高精度定位数据。

Level 3：摆件系统（用户部署的参考站网络）

灵感：无线电追踪中研究人员手持接收器三角定位野生动物

原理：用户可在桌面摆放多个"摆件"小部件（花盆、小船、植物等），每个摆件是一个独立 Qt Widget，位置由方案 D 记录。摆件 = 已知全局坐标的参考站，桌宠 = 待定位目标。

class BeaconWidget(QWidget):
    """用户可拖拽的摆件——桌面上的参考站"""
    def __init__(self, beacon_type: str):
        ...
        self._global_pos = QPoint(0, 0)
    
    def mouseMoveEvent(self, event):
        new_pos = event.globalPos() - self._drag_offset
        self.move(new_pos)
        self._global_pos = new_pos  # ★ 方案D 记录参考站坐标 ★

多边定位（Trilateration）求解：

class TrilaterationSolver:
    @staticmethod
    def solve(anchors: list, distances: list) -> tuple | None:
        """
        anchors: [(x1,y1), (x2,y2), (x3,y3), ...] 摆件全局坐标
        distances: [r1, r2, r3, ...] 到桌宠的距离
        """
        if len(anchors) < 2:
            return None  # 少于2个 → 回退
        if len(anchors) == 2:
            return _solve_two_anchors(anchors, distances)  # 两圆相交 + 消歧
        # 3+ 个：最小二乘
        A, b = [], []
        x1, y1 = anchors[0]
        for i in range(1, len(anchors)):
            xi, yi = anchors[i]
            A.append([2*(xi-x1), 2*(yi-y1)])
            b.append(distances[i]**2 - distances[0]**2 - (xi**2+yi**2) + (x1**2+y1**2))
        x = np.linalg.lstsq(np.array(A), np.array(b), rcond=None)[0]
        return (float(x[0]), float(x[1]))

摆件数量 定位能力 精度
0-1 回退到其他方案 —
2 基线锚点 + 两圆相交（需消歧） 中
3+ 三边/多边定位（最小二乘） 高
分散摆放 夹角 60-90° 最优 最高

用户体验包装：首次启动时提示"给我找几个小伙伴吧 🪴"，用户以为在装饰桌面，实际在部署参考站网络。

Level 4：闭环控制系统（相对坐标系 + 约束求解）

灵感：SLAM（同步定位与建图）+ 粒子滤波 + 图优化

原理：初始化时桌宠出现在屏幕中心（已知绝对位置 → 设为原点 
"(0,0)"）。所有后续位置均为相对偏移。每次用户交互（点击、拖拽、摆件移动、视觉测距）产生一个约束，系统从所有约束中迭代求解最优坐标。

class RelativeCoordinateSystem:
    def __init__(self):
        screen = QApplication.primaryScreen().geometry()
        self._origin_abs = (screen.width() // 2, screen.height() // 2)
        self._pet_rel = (0, 0)
        self._beacons_rel = {}
        self._constraints = []  # 观测约束列表
    
    def add_observation(self, obs_type: str, **kwargs):
        """添加约束：distance / pet_move / beacon_move / visual"""
        ...
    
    def solve(self) -> dict:
        """Gauss-Newton 迭代优化，最小化所有约束残差"""
        for _ in range(50):
            for constraint in self._constraints:
                # 距离约束 → 沿连线梯度下降
                # 位移约束 → 直接修正
                ...
        return {k: (v[0], v[1]) for k, v in nodes.items()}

约束类型：

约束 来源 精度
距离约束 方案 C 视觉测距 / 哨兵像素距离 高
位移约束 方案 D 点击/拖拽 高
基线约束 摆件相对位置 中
全局锚点 四角哨兵 高

优势：

- 零绝对坐标依赖（从原点出发）
- 渐进式精度（约束越多越准）
- 容错性强（单约束噪声被其他约束拉回）
- 误差不累积（每次都是绝对视觉测量）

Level 5：TUI 扫描动画（过程伪装）

原理：将 Level 0-4 的计算过程伪装成"切换动作时的加载动画"。用户看到的是 TUI 风格的扫描特效，实际后台在闪哨兵、截屏、找色、求解。

class TUIScanOverlay(QWidget):
    """伪装成切换加载动画的扫描层"""
    def start_scan(self):
        self.show()
        # 闪哨兵 + 截屏 + 定位 → 全部在动画期间完成
        for s in self._sentinels:
            s.flash(300)
        QTimer.singleShot(100, self._do_locate)
        QTimer.singleShot(800, self.hide)

效果：用户感知 = "切换动作有酷炫扫描动画"；系统实际 = "趁动画期间完成全平台定位校准"。

Level 6：桌面像素指纹（环境特征匹配）

灵感：地磁定位 / WiFi 指纹

原理：桌宠截屏当前周围像素 → 与离线建立的"桌面指纹库"匹配 → 直接输出屏幕坐标。零交互、零锚点、纯被动。

离线阶段：桌宠在屏幕网格采样点截屏 → 记录周围像素特征 → 建立指纹库
在线阶段：截屏当前周围像素 → 匹配指纹库 → 输出位置

Level 7：多源贝叶斯融合（终极形态）

灵感：GNSS + IMU + UWB 多源融合

架构：

┌──────────────────────────────────────────────────────────────┐
│                    贝叶斯融合中心                               │
│                                                              │
│  视觉层 (方案C)  ──┐                                         │
│  几何层 (摆件+哨兵) ┼──→ EKF / 粒子滤波 ──→ 桌宠坐标信念分布   │
│  交互层 (方案D)  ──┘       (自适应粒子数, KLD-sampling)        │
│                                                              │
│  闭环检测 → 回到已知区域时全局优化（消除累积误差）               │
└──────────────────────────────────────────────────────────────┘

四、回退链（Fail-Safe Hierarchy）

摆件 ≥ 3 个 → 多边定位（Level 3）
      ↓ 失败
摆件 = 2 个 → 基线 + 两圆相交消歧（Level 3）
      ↓ 失败
哨兵 + 方案D 点击 → 仿射变换 + 点击坐标（Level 0 + 2）
      ↓ 失败
哨兵 + 方案C 视觉 → 截屏找色 + 几何校验（Level 0 + 1）
      ↓ 失败
闭环控制系统 → 相对坐标 + 约束求解（Level 4）
      ↓ 失败
桌面指纹 → 环境特征匹配（Level 6）
      ↓ 失败
最后手段 → 回到屏幕中心 + 提示用户"点我一下"

任何一层成功即可定位，所有层的结果输入贝叶斯融合中心。

五、跨平台适配

平台 哨兵窗口 截屏 找色 摆件 Widget 方案D
Wayland 
"Qt.Tool" + 
"WindowStaysOnTopHint" grim OpenCV ✓ 独立窗口 ✓ 
"globalPos()" ✓
X11 同上 scrot/import OpenCV ✓ 同上 ✓ 同上 ✓
Windows 
"WS_EX_TOOLWINDOW" BitBlt OpenCV ✓ 同上 ✓ 同上 ✓
macOS 
"NSPanel" screencapture OpenCV ✓ 同上 ✓ 同上 ✓
鸿蒙 Qt 窗口 Qt 截屏 API OpenCV ✓ 同上 ✓ 同上 ✓

零平台私有 API 依赖。核心逻辑仅用 Qt + numpy + OpenCV。

六、首次启动向导（PositionPreview）

┌─────────────────────────┐
│  给我找几个小伙伴吧！🪴                          │
│                                                   │
│  把下面小摆件拖到桌面不同位置：                   │
│  🌸 🚢 🪴 💡 🐱                                │
│                                                   │
│  · 至少 2 个（越多越准）                          │
│  · 分散放效果最好                                │
│  · 放好后双击空白处开始                          │
│                                                  │
│  [我放好了！]                                     │
└─────────────────────────┘

用户操作 → 方案D 记录摆件坐标 → 相对坐标系初始化
         → 进入闭环控制 → 正常进入穿透模式

七、文件结构

meapet/
├── main.py                  # 入口
├── pet_window.py            # 桌宠主窗口
├── live2d_widget.py         # Live2D 渲染
├── locator/
│   ├── __init__.py
│   ├── screen_anchor.py     # Level 0: 四角哨兵
│   ├── visual_locate.py     # Level 1: 方案C 视觉定位
│   ├── beacon_locator.py    # Level 3: 摆件系统 + 多边定位
│   ├── relative_coords.py   # Level 4: 闭环控制系统
│   ├── fusion_locator.py    # Level 7: 贝叶斯融合
│   └── tui_overlay.py       # Level 5: TUI 扫描伪装
├── fingerprint/
│   └── desktop_fingerprint.py  # Level 6: 桌面像素指纹
└── config/
    └── position.json        # 持久化坐标 + 摆件布局

八、一句话总结

用 1px 哨兵窗口当卫星，用花盆摆件当基站，用用户点击当 GPS 信号，用 OpenCV 当接收器，用贝叶斯滤波当大脑——在桌面上实现了一套微型全球定位系统。不依赖任何平台私有 API，纯视觉 + 交互 + 数学，跨平台通用，越用越准。

草案版本 0.1 | 2026-09-03 | 邪修之路，永无止境