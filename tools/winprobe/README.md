# winprobe · Windows 原语测量工具

> **这是测量工具，不是产品后端。** 它存在的理由只有一个：为
> [`docs/proposals/windows-backend.md`](../../docs/proposals/windows-backend.md)
> 的每一条主张提供**可复现的实测数字**。
> 实测记录见 [`docs/measurements/windows-backend-primitives.md`](../../docs/measurements/windows-backend-primitives.md)
> 与原始输出 [`windows-backend-primitives.txt`](../../docs/measurements/windows-backend-primitives.txt)。

它通过 fidus **真实的** `CalibrationIo`/`CaptureIo` trait 与**真实的**
`AnchorCalibrator` + `detect_colored_change` 测量 Windows 投射与截屏原语，
因此测的不是"另一个仿制品"，而是后端将来要满足的那套接口。

**与常驻诊断的分工**（别把两者混起来）：

| | `tools/winprobe`（本目录） | `cargo run --no-default-features --features windows -p fidus --bin fidus-windows-probe` |
|---|---|---|
| 测什么 | **设计空间**：3 种截屏原语 × 2 种投射 × `DwmFlush` 有无，各 20 轮 | **已发货配置**：真实后端 + 真实 `FidusBuilder`/gate/L0，逐条打印契约 §4 清单 |
| 能重测被否决的备选吗 | ✅ 这是它存在的主要理由（`PrintWindow`、非分层窗口、无同步） | ❌ 只跑最终选定的路径 |
| 何时用 | 质疑"为什么是分层窗口 + `BitBlt`"时 | 换机器/换缩放/改后端代码后 |

## 它测量什么

| 组 | 问题 |
|---|---|
| 环境 | DPI 感知是否生效、工作区尺寸、DWM 合成是否开启、截屏是否并非冻结画面 |
| 可见性 | 自建窗口是否真的画上了（自读窗口 DC）、三种截屏原语各看到多少标记像素、落在哪个 bbox |
| 设计矩阵 | `GDI 窗口` vs `WS_EX_LAYERED` × `SRCCOPY` vs `SRCCOPY\|CAPTUREBLT` × `DwmFlush` 有无，各 20 轮 |
| 输入 | `WindowFromPoint` 是否命中标记窗口、`WM_NCHITTEST` 返回值、前台窗口是否被抢 |
| 残留 | `FindWindowW`：clear 之后 / Drop 之后是否还有该窗口类的窗口存活 |
| 端到端 | 真实 L0 Anchor 校准：解出的仿射、rms / verify / consistency 残差、采样数、捕获次数、耗时 |
| 耗时 | 一次 1920×1032 工作区截屏的 min/median/max |

## 运行

工作区**没有** Rust 工具链（Windows 上也没有 MSVC 环境变量），本工具是**独立
workspace**（自己的 `[workspace]` 表），不参与主 workspace 的 `cargo test --workspace`
与离线 vendor 门禁——它依赖 `windows-sys`，而主 workspace 的 `vendor/` 只为 Linux
目标生成。

```bash
# MSVC 工具链 + 仓库内本地 cargo（不改系统 PATH）
export CARGO_HOME="$PWD/.toolchain/cargo" RUSTUP_HOME="$PWD/.toolchain/rustup"
export PATH="$CARGO_HOME/bin:$PATH"
export TMP="$PWD/.toolchain/tmp" TEMP="$PWD/.toolchain/tmp"   # 链接器要能写临时目录

cargo build --release --manifest-path tools/winprobe/Cargo.toml
./tools/winprobe/target/release/winprobe.exe | tee docs/measurements/windows-backend-primitives.txt
```

注意：

* **不要在 Windows 上用 `--offline --config .cargo/config.ci.toml`**：`vendor/` 在
  Windows 检出时会被改写成 CRLF，`thiserror` 的 `.cargo-checksum.json` 因此不匹配
  （`the listed checksum of …/thiserror/tests/ui/duplicate-fmt.rs has changed`）。
  该门禁是给 Linux CI 的，Windows 上请走 registry。
* 运行时会在桌面上**真实**闪现标记窗口（每格 20 轮 × 4 组）。它不注入输入、
  不改显示设置、不动用户的鼠标；退出路径（含 `Drop`）都会销毁窗口。
* 同一进程内第二次注册同名窗口类会返回 `ERROR_CLASS_ALREADY_EXISTS (1410)`，
  这是成功而非失败（窗口类是**进程全局**的），工具按此处理。

## 已知未测量

* **非 100% 缩放**：本机 `dpi_for_system = 96`（100%），无第二个显示器，
  因此 125%/150% 下的落点精度**未实测**（提案中标为待测，附证伪方式）。
* **真实鼠标点击穿透**：`WindowFromPoint` 是输入路由实际使用的判定，但它不等于
  一次真实点击。工具不注入合成输入（那会移动用户鼠标并可能点到下层窗口）。
* 多显示器 / 混合 DPI、DPI 变更（`WM_DPICHANGED`）中途重定位。
