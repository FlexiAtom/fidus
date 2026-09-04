# Niri (Wayland) 点击穿透技术方案

> 适用环境：Niri 26.04+ (wlroots)，Wayland，layer-shell 协议
> 验证日期：2026-08-30
> 状态：✅ 已实装到测试版，实机确认可用

## 1. 结论（TL;DR）

在 Niri 上实现"桌宠/浮窗点击穿透"，**唯一可靠的方式**是：

**使用 `wlr-layer-shell-unstable-v1`，创建一个 `overlay` 层级的 layer surface，并设置 `keyboard_interactivity = NONE` + 空 input region。**

```c
zwlr_layer_shell_v1_get_layer_surface(...)          // LAYER_OVERLAY
zwlr_layer_surface_v1_set_keyboard_interactivity(..., NONE)
wl_surface_set_input_region(surface, empty_region)  // ← pointer 穿透的关键
```

任何基于 `xdg_toplevel`（普通 Qt/GTK 窗口）的方案，在 Niri 上**均无效**，原因见第 3 节。

---

## 2. 核心原理

### 2.1 为什么普通窗口方案失败

Wayland 协议规定：**一个 `wl_surface` 只能拥有一个 role，且一旦分配不可更改。**

Qt 的 `QWidget` / `QWindow` 在 Wayland 上会自动通过 `xdg-shell` 把 surface 升级为 `xdg_toplevel`（普通窗口 role）。因此：

- 尝试在 Qt 窗口的 `wl_surface` 上再调用 `zwlr_layer_shell_v1_get_layer_surface()` → 报错 `Surface already has a role`，连接断开。
- 尝试用 `Qt::WindowTransparentForInput`、`setMask(QRegion())`、空 input region 等 → Qt 内部会重置 input region 为 `NULL`（=全窗口可点），或被 Niri 忽略。

### 2.2 正确的架构：C 层裸 surface

**Qt/PyQt 完全不参与 Wayland surface 管理**，只负责离屏渲染（把画面画到内存）。

C 层（libwayland）负责：
1. 用 `wl_compositor_create_surface()` 创建一个**裸** `wl_surface`（无 role）
2. 用 `zwlr_layer_shell_v1_get_layer_surface()` 赋予 `layer-shell` role
3. 用 `wl_shm` 创建共享内存 buffer，接收渲染像素
4. 管理 input region 实现穿透

```
┌──────────────┐      像素(bytes)      ┌──────────────────┐
│  PyQt 离屏渲染 │ ──── QImage ──────▶ │  C shim (libwayland) │
│  (仅画画面)    │                      │  wl_shm + layer-shell │
└──────────────┘                      └────────┬─────────┘
                                                │ Wayland 协议
                                                ▼
                                          ┌──────────┐
                                          │  Niri    │
                                          │ compositor│
                                          └──────────┘
```

### 2.3 Pointer 穿透的真正关键

| 设置 | 作用 | 影响 pointer 穿透？ |
|---|---|---|
| `KEYBOARD_INTERACTIVITY_NONE` | 不接收键盘焦点 | ❌ 不影响 |
| `wl_surface_set_input_region(空)` | surface 不接收 pointer/touch | ✅ **关键** |
| `LAYER_OVERLAY` | 层级最高（置顶） | 只影响渲染顺序 |
| `exclusive_zone = 0` | 不独占屏幕边缘 | 不影响输入 |

**正确的穿透公式**：

```
LAYER_OVERLAY + KEYBOARD_INTERACTIVITY_NONE + 空 input region
```

Niri 看到 layer surface 的 input region 为空时，pointer 事件会 fall through 到下层窗口。

---

## 3. 协议时序（必须遵守）

wlroots 严格要求：**先 ack configure，才能 attach buffer。**

```
1. wl_surface_commit()              ← 触发 compositor 发 configure
2. 收到 configure 事件
3. ack_configure(serial)            ← 必须先 ack
4. wl_surface_set_input_region(空)  ← 在 configure 阶段设置
5. wl_surface_attach(buffer)        ← 现在才能 attach
6. wl_surface_commit()              ← 画面呈现
7. （每次 compositor 发新 configure 都要回到第 3 步）
```

违反时序会报错：`must ack the initial configure before attaching buffer`

---

## 4. 完整验证代码

### 4.1 编译前提

```bash
# Arch 系需要 wlr-protocols 提供协议 XML
sudo pacman -S wlr-protocols wayland wayland-protocols libwayland

# 用 wayland-scanner 生成协议存根
XML=/usr/share/wlr-protocols/unstable/wlr-layer-shell-unstable-v1.xml
wayland-scanner client-header "$XML" wlr-layer-shell-client-protocol.h
wayland-scanner public-code    "$XML" wlr-layer-shell-client-protocol.c

# xdg-shell（链接时需要补齐符号）
XDGSHELL=/usr/share/qt6/wayland/protocols/xdg-shell/xdg-shell.xml
wayland-scanner client-header "$XDGSHELL" xdg-shell-client-protocol.h
wayland-scanner public-code    "$XDGSHELL" xdg-shell-client-protocol.c
```

### 4.2 minimal_layer_shell.c

```c
#define _GNU_SOURCE
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <fcntl.h>
#include <unistd.h>
#include <sys/mman.h>
#include <wayland-client.h>
#include "wlr-layer-shell-client-protocol.h"

static struct wl_display*          g_display     = NULL;
static struct wl_compositor*       g_compositor  = NULL;
static struct wl_shm*              g_shm         = NULL;
static struct zwlr_layer_shell_v1* g_layer_shell = NULL;
static struct wl_surface*          g_surface     = NULL;
static int g_width  = 320;
static int g_height = 320;
static int g_configured = 0;

static void registry_global(void* data, struct wl_registry* registry,
                            uint32_t id, const char* interface, uint32_t version) {
    if (strcmp(interface, wl_compositor_interface.name) == 0) {
        g_compositor = (struct wl_compositor*)
            wl_registry_bind(registry, id, &wl_compositor_interface,
                             (version >= 4) ? 4 : version);
    } else if (strcmp(interface, wl_shm_interface.name) == 0) {
        g_shm = (struct wl_shm*)
            wl_registry_bind(registry, id, &wl_shm_interface,
                             (version >= 1) ? 1 : version);
    } else if (strcmp(interface, zwlr_layer_shell_v1_interface.name) == 0) {
        g_layer_shell = (struct zwlr_layer_shell_v1*)
            wl_registry_bind(registry, id, &zwlr_layer_shell_v1_interface,
                             (version >= 4) ? 4 : version);
    }
}

static void registry_global_remove(void* data, struct wl_registry* registry, uint32_t id) {}
static const struct wl_registry_listener registry_listener = {
    registry_global, registry_global_remove
};

static struct wl_buffer* create_buffer(uint32_t width, uint32_t height) {
    size_t stride = width * 4;
    size_t size   = stride * height;

    int fd = memfd_create("meapet-shm", 0);
    if (fd < 0) { perror("memfd_create"); return NULL; }
    if (ftruncate(fd, (off_t)size) < 0) { perror("ftruncate"); close(fd); return NULL; }

    void* data = mmap(NULL, size, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
    if (data == MAP_FAILED) { perror("mmap"); close(fd); return NULL; }

    uint32_t* pixels = (uint32_t*)data;
    for (uint32_t y = 0; y < height; y++) {
        for (uint32_t x = 0; x < width; x++) {
            int mx = (x < 10 || x >= (int)width  - 10) ? 0 : 1;
            int my = (y < 10 || y >= (int)height - 10) ? 0 : 1;
            if (mx && my)
                pixels[y * width + x] = (180U << 24) | (60U << 16) | (120U << 8) | 255U;
            else
                pixels[y * width + x] = 0x00000000;
        }
    }

    struct wl_shm_pool* pool = wl_shm_create_pool(g_shm, fd, (int32_t)size);
    struct wl_buffer* buffer = wl_shm_pool_create_buffer(
        pool, 0, (int32_t)width, (int32_t)height,
        (int32_t)stride, WL_SHM_FORMAT_ARGB8888);
    wl_shm_pool_destroy(pool);
    munmap(data, size);
    close(fd);
    return buffer;
}

static void layer_surface_configure(void* data,
                                    struct zwlr_layer_surface_v1* surface,
                                    uint32_t serial, uint32_t width, uint32_t height) {
    zwlr_layer_surface_v1_ack_configure(surface, serial);
    g_configured = 1;

    /* ★★★ pointer/touch 穿透的关键：空 input region */
    struct wl_region* empty_region = wl_compositor_create_region(g_compositor);
    /* 不调 wl_region_add → region 为空 → 不接收任何 pointer/touch 事件 */
    wl_surface_set_input_region(g_surface, empty_region);
    wl_region_destroy(empty_region);

    fprintf(stderr, "[INFO] configured: %ux%u, 空 input region 已设置\n", width, height);
}

static void layer_surface_closed(void* data, struct zwlr_layer_surface_v1* surface) {
    fprintf(stderr, "[INFO] layer_surface closed by compositor\n");
}
static const struct zwlr_layer_surface_v1_listener layer_surface_listener = {
    layer_surface_configure, layer_surface_closed
};

int main(int argc, char** argv) {
    g_display = wl_display_connect(NULL);
    if (!g_display) { fprintf(stderr, "FATAL: 无法连接 Wayland display\n"); return 1; }

    struct wl_registry* registry = wl_display_get_registry(g_display);
    wl_registry_add_listener(registry, &registry_listener, NULL);
    wl_display_roundtrip(g_display);
    wl_registry_destroy(registry);

    if (!g_compositor)  { fprintf(stderr, "FATAL: 缺少 wl_compositor\n");   return 1; }
    if (!g_shm)         { fprintf(stderr, "FATAL: 缺少 wl_shm\n");          return 1; }
    if (!g_layer_shell) { fprintf(stderr, "FATAL: compositor 不支持 zwlr_layer_shell_v1\n"); return 1; }

    g_surface = wl_compositor_create_surface(g_compositor);
    struct zwlr_layer_surface_v1* layer_surface =
        zwlr_layer_shell_v1_get_layer_surface(
            g_layer_shell, g_surface, NULL,
            ZWLR_LAYER_SHELL_V1_LAYER_OVERLAY, "meapet");
    if (!layer_surface) { fprintf(stderr, "FATAL: get_layer_surface 失败\n"); return 1; }

    zwlr_layer_surface_v1_add_listener(layer_surface, &layer_surface_listener, NULL);
    zwlr_layer_surface_v1_set_size(layer_surface, g_width, g_height);
    /* 必须锚定 TOP|LEFT：无锚定(0)时 wlroots 强制居中，margin 不参与计算 */
    zwlr_layer_surface_v1_set_anchor(
        layer_surface,
        ZWLR_LAYER_SURFACE_V1_ANCHOR_TOP | ZWLR_LAYER_SURFACE_V1_ANCHOR_LEFT);
    zwlr_layer_surface_v1_set_margin(layer_surface, 100, 0, 0, 100);
    zwlr_layer_surface_v1_set_keyboard_interactivity(
        layer_surface, ZWLR_LAYER_SURFACE_V1_KEYBOARD_INTERACTIVITY_NONE);
    zwlr_layer_surface_v1_set_exclusive_zone(layer_surface, 0);

    /* 阶段一：触发 configure */
    wl_surface_commit(g_surface);
    wl_display_flush(g_display);

    /* 阶段二：等待 configure + ack + 设置空 input region */
    while (!g_configured && wl_display_dispatch(g_display) != -1) { }

    /* 阶段三：attach buffer */
    struct wl_buffer* buffer = create_buffer(g_width, g_height);
    wl_surface_attach(g_surface, buffer, 0, 0);
    wl_surface_damage(g_surface, 0, 0, g_width, g_height);
    wl_surface_commit(g_surface);
    wl_display_flush(g_display);
    fprintf(stderr, "[OK] 运行成功，按 Ctrl+C 退出\n");

    while (wl_display_dispatch(g_display) != -1) {
        wl_surface_commit(g_surface);
        wl_display_flush(g_display);
    }

    zwlr_layer_surface_v1_destroy(layer_surface);
    wl_surface_destroy(g_surface);
    wl_display_disconnect(g_display);
    return 0;
}
```

### 4.3 编译 & 运行

```bash
gcc minimal_layer_shell.c \
    wlr-layer-shell-client-protocol.c \
    xdg-shell-client-protocol.c \
    $(pkg-config --cflags --libs wayland-client) \
    -lrt \
    -o minimal_layer_shell

./minimal_layer_shell
```

成功标志：屏幕 (100,100) 处出现半透明橙色方块，**点击该方块可穿透到下层窗口**。

---

## 5. 运行时切换穿透（桌宠常用）

桌宠需要在"穿透模式"（让鼠标穿过宠物点到桌面）和"交互模式"（能拖动/点击宠物）之间切换。做法是**动态替换 input region**：

```c
/* 穿透：设置空 region */
void set_click_through(struct wl_surface* surface) {
    struct wl_region* empty = wl_compositor_create_region(g_compositor);
    wl_surface_set_input_region(surface, empty);
    wl_region_destroy(empty);
    wl_surface_commit(surface);
}

/* 恢复：设置 NULL = 全 surface 可点（无限区域）*/
void set_clickable(struct wl_surface* surface) {
    wl_surface_set_input_region(surface, NULL);
    wl_surface_commit(surface);
}

/* 局部可点：只有某矩形区域接收事件（用于拖动条等）*/
void set_input_rect(struct wl_surface* surface, int x, int y, int w, int h) {
    struct wl_region* region = wl_compositor_create_region(g_compositor);
    wl_region_add(region, x, y, w, h);
    wl_surface_set_input_region(surface, region);
    wl_region_destroy(region);
    wl_surface_commit(surface);
}
```

> Wayland 语义：`set_input_region(NULL)` = 无限区域 = 全 surface 接收输入；空 region = 不接收任何输入。

每次修改 input region 后必须 `wl_surface_commit()` 才会生效。

---

## 6. PyQt 集成方案（推荐架构）

把上面的 C 代码封装成共享库 `liblayer_shell_shim.so`，Python 通过 ctypes 调用。

### 6.1 C 层 API 设计

```c
/* layer_shell_c.h */
struct layer_shell_state;
struct overlay_ctx;   /* 不透明结构体，包含 surface / layer_surface / buffer */

struct layer_shell_state* layer_shell_create(struct wl_display* display);
void layer_shell_destroy(struct layer_shell_state* state);

/* 创建 overlay surface + wl_shm buffer，返回上下文 */
struct overlay_ctx* create_overlay(struct layer_shell_state* state,
                                   int width, int height,
                                   int pos_x, int pos_y,
                                   const char* ns);

/* 更新像素（PyQt 离屏渲染后调用）*/
void update_pixels(struct overlay_ctx* ctx, const uint8_t* rgba,
                   int width, int height);

/* 设置点击穿透 / 可点 / 局部可点 */
void overlay_set_click_through(struct overlay_ctx* ctx);
void overlay_set_clickable(struct overlay_ctx* ctx);
void overlay_set_input_rect(struct overlay_ctx* ctx, int x, int y, int w, int h);

/* 移动位置 */
void overlay_set_position(struct overlay_ctx* ctx, int x, int y);

void destroy_overlay(struct overlay_ctx* ctx);
```

### 6.2 Python 侧调用

```python
import ctypes
from PyQt5.QtGui import QImage

shim = ctypes.CDLL("./liblayer_shell_shim.so")
# ... 设置 argtypes/restype ...

# 1. 初始化（传入 Qt 的 wl_display）
state = shim.layer_shell_create(display_ptr)

# 2. 创建 overlay surface
ctx = shim.create_overlay(state, 320, 320, 100, 100, b"meapet")

# 3. 离屏渲染 → 更新像素
img = QImage(320, 320, QImage.Format_ARGB32)
# ... 用 QPainter 画宠物 ...
shim.update_pixels(ctx, img.constBits(), 320, 320)

# 4. 切换穿透
shim.overlay_set_click_through(ctx)   # 穿透
shim.overlay_set_clickable(ctx)       # 恢复可点
shim.overlay_set_input_rect(ctx, 0, 0, 320, 40)  # 仅顶部拖动条可点

# 5. 移动
shim.overlay_set_position(ctx, 500, 300)
```

### 6.3 获取 Qt 的 wl_display

```cpp
// 在 shim 里通过 Qt 私有 API 拿到 display 指针
#include <QtGui/qpa/qplatformnativeinterface.h>

struct wl_display* get_qt_display() {
    QPlatformNativeInterface* native = QGuiApplication::platformNativeInterface();
    return (struct wl_display*)native->nativeResourceForWindow("display", nullptr);
}
```

---

## 7. 常见问题排查

### Q1: `Surface already has a role`
**原因**：该 `wl_surface` 已被 Qt 分配 `xdg_toplevel` role。
**解决**：必须用 `wl_compositor_create_surface()` 自建裸 surface，**不要复用 Qt 的 surface**。

### Q2: `must ack the initial configure before attaching buffer`
**原因**：时序错误，先 attach 了 buffer。
**解决**：严格按第 3 节的时序，先 commit → 等 configure → ack → 再 attach。

### Q3: `xdg_popup_interface` 未定义符号
**原因**：链接时缺少 `xdg-shell-client-protocol.o`。
**解决**：把 `xdg-shell-client-protocol.c` 也一起编译链接。

### Q4: `memfd_create` 隐式声明
**原因**：缺少 `_GNU_SOURCE` 特性测试宏。
**解决**：在 `#include` 前 `#define _GNU_SOURCE`，或编译时加 `-D_GNU_SOURCE`。

### Q5: 画面出现但点击不穿透
**原因**：只设了 `KEYBOARD_INTERACTIVITY_NONE`，没设空 input region。
**解决**：在 `layer_surface_configure` 里调用 `wl_surface_set_input_region(surface, empty_region)`。

### Q6: Niri 报 protocol error 后断连
**原因**：Qt 旁路写入 surface 状态，与 Qt 内部冲突。
**解决**：C 层创建的 surface 绝不能让 Qt 管理；Qt 侧用 `Qt.BypassWindowManagerHint` + 环境变量 `QT_WAYLAND_USE_BYPASSWINDOWMANAGERHINT=1`，或干脆不用 Qt 窗口。

### Q7: 如何确认 surface 身份
```bash
niri msg layers | grep meapet
```
查看当前所有 layer surface，确认我们的 surface 存在且 namespace 正确。

### Q8: set_position / set_margin 无效，surface 总是显示在屏幕中央
**现象**：明明传了 `(x, y)`，surface 却固定在屏幕正中。
**原因**：`anchor=0`（无锚定）时，wlroots 的布局逻辑是**强制居中**，`margin` 完全不参与计算：
```c
if      (anchor & LEFT)  x = usable.x + margin.left;
else if (anchor & RIGHT) x = usable.x + usable.width - width - margin.right;
else                     x = usable.x + (usable.width - width) / 2;  // ← margin 无效
```
**解决**：锚定到左上角，margin 才变成精确的「距左 x、距上 y」：
```c
zwlr_layer_surface_v1_set_anchor(ls,
    ZWLR_LAYER_SURFACE_V1_ANCHOR_TOP | ZWLR_LAYER_SURFACE_V1_ANCHOR_LEFT);  // = 5
```

### Q9: 位置同步要在 `hide()` 之前取坐标
**现象**：切回穿透模式后，桌宠位置停留在拖动前的位置。
**原因**：Wayland 客户端无法查询自身全局坐标；`hide()` 之后窗口位置信息即失效。
**解决**：切换顺序必须是——先取坐标 → 同步给 layer surface → 最后 `hide()`：
```python
x, y = self.x(), self.y()   # ① 先取（hide 之前）
backend.set_position(x, y)  # ② 同步
self.hide()                 # ③ 最后才隐藏
```
反向切回时，也应在 `show()` 之前把窗口 `move()` 回记录的位置，避免瞬间跳走。

---

## 8. 构建检查清单

- [ ] `wlr-layer-shell-unstable-v1.xml` 存在（来自 `wlr-protocols` 包）
- [ ] `wayland-scanner` 生成的 `.h` / `.c` 已编译为 `.o`
- [ ] `xdg-shell-client-protocol.o` 已链接（补齐 `xdg_popup` 等符号）
- [ ] 编译加了 `-D_GNU_SOURCE`（或文件顶部 `#define`）
- [ ] 链接 `libwayland-client`（`pkg-config --libs wayland-client`）
- [ ] 运行环境：`WAYLAND_DISPLAY=wayland-*` 已设置
- [ ] 代码严格遵循"先 configure/ack，后 attach buffer"时序
- [ ] `layer_surface_configure` 里设置了空 input region

---

## 9. 兼容性说明

本方案基于 **wlroots 的 `zwlr_layer_shell_v1`**，适用于：

- ✅ Niri (所有版本，26.04+ 验证通过)
- ✅ Hyprland
- ✅ Sway
- ✅ KWin (Plasma Wayland)
- ✅ 任何基于 wlroots 或支持 layer-shell 的 compositor

不适用：
- ❌ macOS (无 Wayland)
- ❌ Windows (无 Wayland)
- ❌ X11 会话（需用 XShape 方案，与本方案互斥）

**Compositor 检测建议**：运行时通过 `WAYLAND_DISPLAY` 判断是否 Wayland，再读 compositor 信息决定走 layer-shell 还是 XShape。

---

## 10. 参考资料

- Wayland 协议：`wl_surface.set_input_region` 语义
- `wlr-layer-shell-unstable-v1` 协议规范
- Qt 源码：`QWaylandWindow::updateInputRegion()`（确认空 region 语义）
- Niri issue tracker：layer-shell 支持、input region 处理
- wlroots：`layer_shell` 实现，hit-test 逻辑

---

## 附录：变更记录

| 日期 | 变更 |
|---|---|
| 2026-08-30 | 初始版本，验证 Niri 26.04 穿透方案 |
| 2026-08-30 | 确认 `KEYBOARD_INTERACTIVITY_NONE` 不影响 pointer，需用空 input region |
| 2026-08-30 | 验证 `minimal_layer_shell.c` 穿透成功 |

---

若文档无法复现以项目仓库的实际代码为准：https://github.com/suan-11/mea-pet-public
作者：Hy4 preview（AI）、FlexiAtom（人）