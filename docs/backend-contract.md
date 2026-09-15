# 后端契约（移植到新平台）

> **范围边界**：fidus 只定位调用方自身窗口；后端只为自身窗口的标记投射与屏幕截取提供原语，不枚举、识别或定位第三方系统窗口。
>
> **读者**：想把 fidus 带到 Windows / macOS / 某个新合成器的人。
> 规范见 [`spec.md`](spec.md)；Wayland 具体协议见 [`layer-shell-primer.md`](layer-shell-primer.md)。

## 0. 一句话

**后端只做两件事：把标记画到我指定的位置，把屏幕截给我。**

校准器（L9/L0）与估计器（L1/L8/L7）**一行都不用改**——它们只认 `CalibrationIo` / `CaptureIo` 两个 trait。现有后端各约 300 行。

## 1. 绝对禁止

> 这一节比第 2 节重要。写对原语只是能用，违反这一节是**架构性污染**。

**永不绑定任何报告窗口几何的协议或 API。**

具体地说，后端不得调用、封装或转发：

| 平台 | 禁止 |
|---|---|
| Windows | `GetWindowRect` / `GetClientRect` / `ClientToScreen` |
| macOS | `CGWindowListCopyWindowInfo` / `NSWindow.frame` |
| X11 | `XQueryTree` / `XGetGeometry` / `XTranslateCoordinates` |
| Wayland | `zwlr_foreign_toplevel` 的任何几何字段、合成器私有 IPC（如 `niri msg windows`） |

**哪怕它在你的平台上是准的。** 理由见 [`spec.md` §2.1](spec.md)：这些值在跨平台、跨缩放、跨合成器时不可比，而一旦它们进了概率池，产出的错误位置**与真实测量长得一模一样，永不自我暴露**。

> **唯一的例外不是例外**：`usable_size_hint()` 返回合成器宣告的可用区**尺寸**。它是**布点提示**，不是测量——不进 `CoordinateFrame`，不进概率池。判据很简单：**提示错了会怎样？** 标记落到屏幕外 → 检测不到 → 自动重试。能自我纠正的就是提示；会静默产出错误坐标的就是污染。

## 2. 必须实现的原语

### 2.1 `CaptureIo::capture() -> Frame`

把当前输出截成**自上而下**的像素帧。

- **Y 轴方向由后端归一化**。若平台给的是 bottom-up（OpenGL 系），后端负责翻转——上层假定 `(0,0)` 是左上角。
- `Frame` 需给出 `width` / `height` / `stride` / `format` / `data`。`stride` 可大于 `width * bpp`（行对齐），上层会正确处理。
- 截不到就 **`Err`**，不要返回黑帧或上一帧。黑帧会让检测器"什么都没检出"从而进入重试，看起来像环境问题，实际是你在编造数据。

### 2.2 `CalibrationIo::show_marker(pos, style)`

在**逻辑坐标 `pos`** 处画一个标记，`pos` 是标记的**左上角**（不是中心）。

- **返回前必须保证标记已经真的出现在屏幕上**。这是最容易错的一条：异步提交后立刻返回，校准器会截到还没画出来的帧，测量全部作废。Wayland 上要等 frame callback，不能只 `commit`。
- 位置必须**精确**。`margin` / 窗口位置在送出前**量化到整数逻辑像素**，且与上层的取整规则一致——半像素误差会直接进入仿射残差。
- 标记窗口**必须全程不截获输入**（[`spec.md` §11.4](spec.md)）。注意"不接收键盘"≠"鼠标能穿透"。

### 2.3 `CalibrationIo::clear_marker()` / `destroy_projector()`

- `clear_marker` 同样**必须等到标记真的从屏幕上消失**才返回——基线帧的正确性依赖它。
- `destroy_projector` **幂等**，且会在成功、失败、`Drop` 三条路径上被调用。校准完必须不留任何残留窗口（原则五）。

### 2.4 `show_markers(&[(pos, style)])`（可选）

同时投射多个标记。**L0 Anchor 的前提**；L9 不需要。

- 单 surface 的后端（layer-shell）**保留默认实现**（返回 `MultiMarkersUnsupported`），并在探测的 `EnvironmentContext` 里报 `multi_marker_projection: false`。
- 每标记一窗的后端（X11 override-redirect，以及将来的 Windows/macOS）覆写它。

> Gate **按能力而非平台身份**决定 L0 是否可用——L0 是全平台兜底，不是"X11 专属"。

## 3. 能力探测（`EnvironmentContext`）

后端如实填写自己**能做什么**，不描述自己**是谁**。

- `has_layer_shell`、`multi_marker_projection`、`wayland_input_region_supported`：能力布尔。
- `screen_capture_permission`：**连接时探测一次真实截屏**并如实上报。
- `compositor_type`：仅用于日志与已知缺陷绕行，**不得**作为能力判断依据。

> **诚实拒绝优于中途失败**。范例：rootless XWayland 下 `GetImage(root)` 返回 `BadMatch`，X11 后端在连接时就把 `screen_capture_permission` 报为 `Revoked`，于是 Gate 拒绝、`Auto` 落到下一个后端——而不是校准到一半才炸。

## 4. 自检清单

移植完成后，逐条确认：

- [ ] 没有调用任何第 1 节列出的 API（`grep` 一遍）
- [ ] `capture()` 返回自上而下的帧；失败时 `Err` 而非黑帧
- [ ] `show_marker` 返回时标记**确实可见**（截一帧验证，别信 API 的返回值）
- [ ] `clear_marker` 返回时标记**确实不可见**
- [ ] 标记位置量化到整数逻辑像素
- [ ] 标记窗口不截获鼠标（**实测**：在标记上点击，事件应落到下层窗口）
- [ ] `destroy_projector` 幂等，且 `Drop` 路径也会执行
- [ ] 校准后 `niri msg layers` / `xwininfo -root -tree` / 平台等价物中**无残留窗口**
- [ ] 权限不足时**连接期**就诚实上报
- [ ] 按 [`AGENTS.md` §6](../AGENTS.md)，踩到的坑补一个**不需要显示服务器**的仿真测试

## 5. 参考实现

| 后端 | 投射方式 | 多标记 |
|---|---|---|
| `fidus-backend-wayland-layer` | 单 layer surface + `anchor=TOP\|LEFT` + margin | ❌ 单 surface |
| `fidus-backend-x11` | 每标记一个 override-redirect 窗口 | ✅ |

X11 后端的 override-redirect 窗口是 fidus **自己**创建、放在**自己**选的坐标上的——与 layer-shell 的 margin 同一性质：**我们告诉系统标记在哪，而不是问系统标记在哪。** 这就是零信任在后端层的全部含义。
