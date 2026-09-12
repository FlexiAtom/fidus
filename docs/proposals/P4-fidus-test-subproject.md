# 提案 · P4 独立测试子项目 `fidus-test`

> **状态：✅ 已转方案并完成发布收口（2026-09）** — 伪代码级规划见 [`docs/drafts/P4-fidus-test.md`](../drafts/P4-fidus-test.md)，规范已并入 [`docs/spec.md` §8](../spec.md)。CI 协议核心、live-host 协议桥接、本机 live-container 实测和 Debian runtime Release 归档均已完成；Release 资产带 digest、image ID、SHA-256 和离线 smoke 校验。
> 日期：2026-09 · 类型：工程基础设施
> 本文保留“原始主张 → 全量审核 → 修订后建议”作为审查记录，最终裁决以 §10–§12 和 spec §8 为准。

---

## 0. 原始主张

新增一个独立 workspace 子项目 `fidus-test`，专门承载 fidus 的**跨环境测试工具**，尤其是需要真实显示服务器、真实 compositor、真实窗口活动的测试。

它的第一目标不是增加定位功能，而是让以下问题可以被重复测量：

- 标记是否真的投射到了屏幕；
- 截屏是否遵守预期的帧时序；
- 真实桌面活动是否破坏 L9；
- Niri、Sway、Hyprland、KDE 等 compositor 的行为是否存在差异；
- 不同缩放、旋转、窗口内容和权限状态下，fidus 是成功、诚实失败，还是错误产出坐标。

当前 `fidus-live-calibrate`、`fidus-probe-marker` 和 `scripts/l9_live_soak.sh` 已经承担了这些工作，但它们分散在伞 crate 与脚本目录中。测试场景、采样工具、报告格式继续增加时，生产 crate 会被测试用例和诊断选项反向污染。

**提案结论（初始）**：增加 `crates/fidus-test/`，加入 workspace，但它只能依赖 fidus 的公开 API 和测试所需的基础工具；它不能复制校准器、检测器、仿射求解器或 backend 实现。

---

## 1. 解决谁的问题

### 1.1 现在的问题

当前测试分成三处：

| 位置 | 当前内容 | 问题 |
|---|---|---|
| `crates/*/tests/` | 无显示服务器仿真 | 贴近实现，但不能观察真实 compositor |
| `crates/fidus/src/bin/` | live smoke/probe 二进制 | 能跑，但属于伞 crate，职责混杂 |
| `scripts/` | scale/soak shell 脚本 | 能编排，但缺统一结果模型和退出语义 |

P3 已经证明测试工具本身会污染测试：脚本输出滚动终端，曾被误测成背景噪声。测试工具需要自己的生命周期、日志、报告和污染控制约束。

### 1.2 使用者

- 维护 backend 的开发者：验证“投射 + 截屏”原语是否真的满足契约；
- 维护校准器的开发者：在真实桌面活动下复现失败；
- 迁移到新 compositor / 新平台的开发者：先跑能力与时序测试；
- 项目审阅者：查看机器可读的失败分类，而不是只看一行 stdout。

### 1.3 不解决的问题

- 不提供生产运行时 API；
- 不替代 crate 内仿真测试；
- 不把某一台机器的测试结果宣称为所有平台的保证；
- 不建立第二个 fidus backend；
- 不通过平台窗口几何 API“验证”fidus 坐标。

---

## 2. 子项目边界

建议目录：

```text
crates/fidus-test/
├── Cargo.toml
├── src/
│   ├── lib.rs              # 协议、场景、分类、报告模型
│   └── bin/
│       └── fidus-test.rs   # 解析与场景入口；live 工具先保留原 crate
└── README.md
```

`fidus-live-calibrate` 与 `fidus-probe-marker` 第一阶段不直接搬入：旧入口继续由 `fidus` 伞 crate 构建，避免破坏命令兼容；只有先完成公开 facade 能力和协议 emitter 的设计，才可新增薄 wrapper。

### 2.1 允许依赖

第一版只允许：

- `fidus`：调用正式 facade；
- `fidus-core`：复用公开的 Frame / I/O 类型时才允许；
- Rust 标准库；
- 已有 workspace 依赖。

**不新增第三方依赖**。报告先输出稳定的文本与 JSON-like 行格式，等字段稳定后再考虑 serde；不能为了报告格式提前扩大依赖面。

### 2.2 禁止依赖与行为

测试子项目不得：

- 读取 compositor 报告的窗口坐标、窗口树或原生窗口几何；
- 调用 `GetWindowRect`、`xdg_toplevel` 之外的坐标旁路、Niri 窗口几何命令等作为真值；
- 复制 `detect.rs`、`crosshair.rs`、`anchor.rs`、backend capture/projection 代码；
- 在测试项目内根据截图重新解算一个“参考坐标”再与 fidus 比较；
- 把测试项目的成功包装成 fidus 成功；
- 默认启动会污染屏幕的动画或输出进程而不记录它们。

允许使用 compositor 命令做**测试控制**，例如切换 scale/transform、读取环境标识、恢复配置；这些不是坐标真值，报告中必须标为 control input。

---

## 3. 第一版功能

### 3.1 `fidus-live-calibrate`

第一阶段继续由 `fidus` 伞 crate 提供，保持现有命令和退出码：0 成功、1 校准失败、2 初始化失败、3 无可用方法。它暂不声称符合 `fidus-test` 的完整结果协议。

后续新增协议入口时，必须抽出共享的公开 facade 调用函数；旧入口只能成为薄兼容包装，不能维护第二套校准逻辑。旧命令的 `stage 4` 平台窗口检查仍只能是人读诊断提示，不能产生 `teardown=confirmed` 协议事实。

### 3.2 `fidus-probe-marker`

第一阶段继续由 `fidus` 伞 crate 提供，作为 backend 诊断工具，不迁移到 `fidus-test`。当前它直接使用 `fidus_backend_wayland_layer` 和 `fidus_core` 的低层能力，正式 facade 尚未暴露等价的投射/截屏 API；直接搬迁会违反“测试项目只调用正式 facade”的边界。

只有另行完成公开 facade 原语设计、feature 边界和零信任审查后，才可将它变为 `fidus-test` 的薄入口。现阶段 `fidus-test` 只消费/解析其未来稳定协议，不复制其 backend 实现。

保留的诊断能力：单标记投射与截图、marker/no-marker 对照、burst 连续采样、cycle 模式以及变化像素诊断。它不是“校准成功”的证明。

### 3.2 `fidus-probe-marker`

从现有诊断二进制迁入，保留：

- 单标记投射与截图；
- marker / no-marker 对照；
- burst 连续采样；
- cycle 模式；
- 输出变化像素、阈值像素与 bounding box。

它是诊断工具，不是“校准成功”的证明；报告必须明确 `probe` 与 `calibration` 的区别。

### 3.3 场景运行器

第一版只做场景编排，不做场景内容注入：

```text
fidus-scenario --scenario l9-live --runs 15 --scale 1,1.25
```

职责：

1. 记录开始时环境元数据；
2. 按配置切换 scale/transform；
3. 将每次运行的 stdout/stderr 写文件；
4. 运行结束后恢复 compositor 配置；
5. 对输出做有限、可审计的结果解析；
6. 生成汇总报告与退出码。

第一版不自动制造“假用户活动”。真实桌面活动必须由用户主动提供；自动干扰场景另列为 `synthetic-activity`，不能和真人实机场景混称。

### 3.4 容器化执行计划

`fidus-test` 采用**双边界、三模式**，容器化增强可重复性，但不把容器冒充成 compositor 隔离层：

| 模式 | 执行位置 | 显示会话 | 是否允许改宿主输出 | 用途 |
|---|---|---|---|---|
| `ci` | 容器 | 无 | 否 | 编译、测试、clippy、doc、解析器与报告单测 |
| `live-host` | 宿主机 | 当前用户会话 | 只有显式授权才允许 | 真人使用中的 Niri/Sway/Hyprland/KDE 实测 |
| `live-container` | 容器 | 显式挂载宿主会话 | 默认否；显式授权才允许 | 验证容器入口与宿主显示会话的兼容性 |

#### `ci` 模式

镜像使用 Debian 12 slim，固定 Rust 1.86.0 toolchain、系统开发包、Wayland/X11 headers、locale 和时区，并强制基础镜像 digest。默认无 `$XDG_RUNTIME_DIR`、Wayland/X11 socket、`/dev/dri`、宿主 HOME 和 compositor 控制命令。

它只运行确定性内容：

- workspace 测试、clippy、doc；
- `fidus-test` 的配置/解析/报告单测；
- FakeIo 与无显示服务器场景。

`ci` 通过不代表任何真实 backend 或真实桌面通过。

#### `live-host` 模式

这是当前实机测试的默认可信路径：runner 在宿主机运行，直接使用用户的显示会话；用户可以继续操作电脑，噪声底照常记录。runner 负责记录环境、日志、恢复和报告，不使用窗口几何作为真值。

#### `live-container` 模式

只有调用方明确要求时才启用。容器可以只读挂载：

- Wayland socket 与必要的 `XDG_RUNTIME_DIR` 子路径，或 X11 `DISPLAY` / 明确的认证文件；
- 必要的环境变量；
- 输出目录。

默认不挂载整个 HOME、不授予 `--privileged`、不挂载 `/dev/dri`，也不把 `niri msg` 控制命令放进容器。宿主 runner 仍负责 compositor 控制与恢复。

报告必须记录：

```text
execution_mode=ci|live-host|live-container
host_display_session=true|false
wayland_socket_mounted=true|false
x11_socket_mounted=true|false
dri_device_mounted=true|false
output_mutation_requested=true|false
```

`live-container` 通过只能证明“该 Debian 容器入口在这个宿主会话上工作”，不能证明容器隔离了宿主 compositor。宿主 compositor、显示 socket 和真实桌面始终是信任边界。

#### 容器安全与恢复约束

- 任何改 scale/transform 的操作都必须由调用方主动传入 `--allow-output-mutation`；
- 优先由宿主 runner 改配置，容器只执行测试 binary；
- 修改前记录原值，恢复后读回确认；
- `SIGINT`、超时、子进程异常和正常退出都走恢复路径；
- `SIGKILL`、宿主崩溃、compositor 重启无法由 runner 保证恢复，报告必须标为 `RecoveryUnverified`，不能假装成功；
- 容器环境缺少显示 socket、权限或协议时报告 `EnvironmentUnavailable`，不得降级成测试通过。

#### 容器实施顺序

1. 先提交最小 `ci` 镜像与锁定的构建命令；
2. 在容器内验证报告解析、配置校验和退出码；
3. 保持 `live-host` 先行，迁移现有 live 工具；
4. 再以最小只读挂载实现 `live-container`；
5. 最后实测异常退出、socket 权限、teardown 与恢复失败分类。

---

## 4. 报告模型

每次运行至少记录：

```text
scenario
run_id
backend
compositor
output
scale
transform
activity_observations
noise_floor_changed_pairs
calibration_status
failure_class
rms_residual_px
verification_max_err_px
consistency_max_err_px
teardown_status
```

`activity_observations` 不能伪装成精确的“用户正在操作”布尔值，第一版应使用：

- `noise_floor_changed_pairs`：可测量；
- `operator_declared_active`：用户显式声明；
- `unknown`：没有证据。

失败分类必须区分：

- `BackendInit`
- `Permission`
- `NoCalibrationMethod`
- `DetectionNotFound`
- `DetectionAmbiguous`
- `AccuracyResidual`
- `Verification`
- `Consistency`
- `Timeout`
- `Teardown`
- `HarnessError`
- `EnvironmentUnavailable`
- `RecoveryUnverified`

如果日志解析失败，结果是 `HarnessError`，**不能默认为 calibration failed 或 success**。

`EnvironmentUnavailable` 与 `RecoveryUnverified` 都属于测试工具/环境结果，不得计入 fidus 校准失败率；但它们会使本次场景验收失败，防止测试工具把“没测成”伪装成“通过”。

---

## 5. 命令与退出语义

建议退出码：

| 退出码 | 含义 |
|---:|---|
| 0 | 场景执行完成，所有要求的运行通过 |
| 1 | fidus 明确失败，或验收阈值未通过 |
| 2 | 测试环境/权限/backend 初始化失败 |
| 3 | 测试工具自身错误、输出无法解析或恢复失败 |
| 4 | 用户主动中断；必须先恢复 compositor 配置 |

**不能把“没有运行”当成通过。** `N=0`、空 scale 列表、缺少报告字段都应是 harness error。

---

## 6. 仿真测试放在哪里

原有 crate 内仿真测试继续保留：

- `fidus-calibrate/tests/sim.rs` 保留 L9 行为与 FakeIo；
- `fidus-calibrate/tests/anchor_sim.rs` 保留 L0；
- `fidus-estimate/tests/*` 保留 C 层测试。

原因：这些测试需要贴近生产 crate 的类型边界，并且有些是当前实现的回归测试，不是跨环境测试工具。

`fidus-test` 只测试：

- 工具编排；
- 日志与结果解析；
- 报告完整性；
- 真实 backend 的黑盒行为。

---

## 7. 全量审核与纠偏

以下是对上面初始主张的全量审核结果。按“快审 → 全审”文档约定，不直接接受“独立子项目有明显收益”作为实现充分理由。

### 7.1 🔴 迁移二进制会破坏现有用户命令

原始主张说“从 `fidus` 迁入 `fidus-test`”。这会让已有命令失效：

```bash
cargo run --release -p fidus --bin fidus-live-calibrate
```

**纠偏**：第一阶段保留原二进制作为兼容入口，新增 `fidus-test` 版本；等一个明确的弃用周期后再删除，或者干脆长期保留一个极薄的转发入口。不能为了目录整洁直接破坏命令。

### 7.2 🔴 `fidus-test` 依赖 `fidus` 会造成 feature/backend 选择歧义

`fidus` 当前默认启用 Wayland 与 X11。测试子项目若直接依赖默认 feature，无法表达“只测 Wayland”“只测 X11”，也会让没有某个系统依赖的环境构建失败。

**纠偏**：在草案阶段先确定 feature 传播矩阵：

```toml
fidus = { workspace = true, default-features = false, features = ["wayland-layer"] }
```

但不能猜测第一版需要哪些 feature；应先列出构建矩阵并实测。

### 7.3 🔴 场景运行器不能默认切换用户 compositor 配置

直接调用 `niri msg output ...` 会修改用户当前桌面。即使有 `trap`，进程被 kill -9、compositor 重启或命令半成功都会留下错误配置。

**纠偏**：

- 默认只读/不改变环境；
- `--allow-output-mutation` 作为调用方主动要求的逃生门；
- 修改前记录原始 scale/transform；
- 恢复失败必须退出码 3；
- 不得声称恢复成功，除非读回确认。

这符合项目既有“逃生门必须调用方主动要求”的约束。

### 7.4 🔴 `operator_declared_active` 不是测量事实

用户声明正在使用电脑与屏幕实际变化不是同一属性。只记录声明会让报告看起来比证据更精确。

**纠偏**：以 `noise_floor_changed_pairs` 为实测事实，用户声明只能是附加字段；全程没有变化时报告 `activity evidence: absent`，不能把 0/30 当忙碌桌面结果。

### 7.5 🟡 报告解析生产 CLI 文本脆弱

从 stdout 正则解析质量和错误，生产 CLI 文案一改，测试工具就会误报。

**纠偏**：草案阶段必须先定义机器可读输出协议（例如固定 `FIDUS_RESULT key=value` 行），同时保留人读日志；解析不到协议行就是 `HarnessError`。不能先写正则再把它称为协议。

### 7.6 🟡 `probe-marker` 的现有采样可能污染屏幕

即使输出重定向，probe 自己创建/销毁 surface 仍然是屏幕活动；更重要的是 probe 的 marker 是被测对象，不应把 probe 的输出作为背景噪声基准。

**纠偏**：噪声底测量必须是单独的 no-marker capture 会话，并在报告中标注采样窗口；probe 输出不能同时作为 calibration 通过证据。

### 7.7 🟡 独立测试子项目会增加 workspace 编译成本

每个 live 工具都作为独立 binary target 会增加构建与文档目标，并可能重新触发命名冲突。

**纠偏**：共享逻辑放 `fidus-test/src/lib.rs`；二进制只做薄 CLI；为每个 binary 指定唯一名称；`cargo doc` 后必须零警告。脚本与容器不被删除：脚本负责宿主编排，容器负责固定执行环境，专用 crate 负责协议与报告。

### 7.8 🟡 三种载体并无本质冲突，但职责不能重复

之前的草案审查把“仅脚本”“仅专用 crate”“仅容器”写成互相替代的路径，表述不准确。它们是不同层：

- 脚本：宿主入口、compositor 控制、容器启动、恢复兜底；
- 专用 crate：场景模型、结果协议、解析器、报告、退出语义；
- 容器：固定 CI toolchain/系统包，或显式承载 live binary。

三者应共存。真正不可共存的是多个 owner 同时负责同一职责：parser 不能被脚本和 crate 各写一份；output mutation 与恢复不能由容器和宿主各自执行；生产算法不能在测试层复制。

**修订**：采用 `live-host = 脚本 + fidus-test + 宿主会话`，`ci = 容器 + fidus-test`，`live-container = 宿主脚本 + 容器内 fidus-test + 显式显示挂载`。三种模式分开报告与统计。

### 7.9 🟡 报告 JSON 不是“顺手加个格式”

无 serde 时手写 JSON 容易产生转义和兼容性错误；加 serde 则扩大依赖。

**纠偏**：第一版先输出稳定的行式 key-value 和人读汇总；只有字段冻结且确有 CI/外部消费需求时，再单独提案引入序列化依赖。

### 7.10 🟡 真实场景不可硬编码“通过率 0/15”

真实桌面活动具有随机性，0/15 是本机一次观测，不是所有 compositor 的规格保证。

**纠偏**：报告同时记录原始运行和噪声底；验收阈值必须按场景配置，并明确“本次观测”与“规范要求”两种层次。

### 7.11 ✅ 零信任边界保持成立

测试工具只调用 fidus 自己的投射、截屏和校准接口，不使用窗口几何 API，因此没有发现原则冲突。

### 7.12 ✅ 仿真测试不迁移

保留测试在生产 crate 内是正确的，不应为“统一目录”破坏测试与实现的局部性。

### 7.12 🔴 容器不等于显示隔离

“进入容器”容易被误读成“测试不影响宿主桌面”。只要挂载 Wayland/X11 会话，容器中的程序仍然在操作宿主 compositor 的显示会话；只要允许输出控制，容器就可能改变用户桌面。

**纠偏**：将 `ci`、`live-host`、`live-container` 明确分开；报告记录 socket、设备和输出修改权限；`live-container` 只能验证容器入口，不提高宿主 compositor 的可信度。

### 7.13 🔴 容器镜像不能隐藏依赖漂移

仅固定 Rust 镜像标签并不能固定系统行为。Wayland/X11 headers、动态库、locale、时区、字体和 compositor 版本仍可能不同；反过来把整个宿主 `/usr` 或 HOME 挂进去又破坏了隔离。

**纠偏**：`ci` 镜像固定基础镜像 digest、Rust toolchain、系统包清单、locale/timezone，并把镜像版本写入报告；`live-container` 报告宿主 compositor 与镜像元数据，但不宣称跨宿主可复现。

### 7.14 🔴 强杀后不能保证恢复

`trap` 只能覆盖可处理的退出；`SIGKILL`、宿主崩溃和 compositor 重启都可能留下 scale/transform 状态未知。容器的生命周期反而增加了这种异常路径。

**纠偏**：把 `RecoveryUnverified` 作为独立状态；恢复读回失败不是普通 calibration failure，而是 harness/environment failure。实现阶段必须用可控子进程测试 SIGINT、超时和异常退出，不能声称覆盖 SIGKILL。

### 7.15 🟡 live-container 需要 UID/GID 与 socket 权限实测

“挂载 socket”不等于容器能使用它。`XDG_RUNTIME_DIR` 的目录权限、socket 所属 UID/GID、Wayland/X11 认证和容器用户身份都可能不同。

**纠偏**：全审要求在实现前实测：同 UID 非 root、不同 UID、缺 socket、socket 权限拒绝四种结果；失败必须为 `EnvironmentUnavailable`，不能落入普通校准失败。

### 7.16 🟡 `/dev/dri` 不应预先加入

当前 layer-shell + screencopy 路径不证明需要 GPU 设备。预先加入 `/dev/dri` 会扩大权限并增加机器差异。

**纠偏**：默认不挂载；只有某个 backend 的实测错误明确证明需要时，另提案增加最小设备权限，并补充安全理由与回归测试。

### 7.17 🟡 容器内 runner 与宿主 runner 不应各自恢复

如果容器和宿主脚本都负责 scale/transform 恢复，异常时可能发生竞态，报告也无法判断谁最后写入了配置。

**纠偏**：只有一层拥有 output mutation authority，建议是宿主 runner；容器 runner 只请求或报告，不直接执行 compositor 控制。

### 7.18 🟡 容器化会改变屏幕时序

容器的调度、进程启动和 socket 转发可能改变 baseline/post 间隔。容器 live 结果不能和宿主 live 结果直接合并统计。

**纠偏**：报告必须有 `execution_mode`，三种模式分开统计；live-container 首轮目标是验证入口，不用于替代 live-host 的真实性能结论。

### 7.19 🟡 配置注入与命令执行边界

场景文件、输出路径和 compositor 参数如果直接拼接 shell 字符串，会产生命令注入，也会让结果不可复现。

**纠偏**：实现时使用参数数组而不是 shell 拼接；场景配置只允许白名单 backend、output、scale、transform；路径规范化并拒绝工作区外的隐式覆盖，除非调用方明确指定。

### 7.20 ✅ 容器计划仍符合零信任

容器层没有引入平台坐标真值；它只是执行环境和报告边界。显示 socket 是传输入口，不是可信坐标来源。只要维持“投射 + 截屏”唯一生产原语和禁止窗口几何旁路，容器化不会改变五条设计原则。

---

## 8. 全审后的修订建议

提案不能直接进入实现，建议转成以下顺序：

1. 先创建 `fidus-test` 空 crate，仅验证 workspace、feature 和 doc 不冲突；
2. 定义固定机器可读结果行协议，并给 parser 写纯字符串单测；
3. 以复制兼容方式加入 live-calibrate，不删除原入口；
4. 迁移 probe，但先完成 no-marker 噪声底的独立报告；
5. 加入 `--allow-output-mutation`，默认禁止修改输出；只有宿主 runner 拥有 mutation authority；
6. 先做 `ci` 容器：固定 toolchain/系统包/locale，验证 build、test、clippy、doc 和报告；
7. 再做 `live-host`：在人正在使用的桌面上验证报告、退出码、teardown、恢复与噪声底；
8. 以最小只读 socket 挂载实现 `live-container`，实测同 UID、不同 UID、缺 socket、权限拒绝；不挂 `/dev/dri`，不默认挂整个 HOME；
9. 用可控子进程测试 SIGINT、超时、异常退出；SIGKILL/宿主崩溃只报告 `RecoveryUnverified`，不声称可自动恢复；
10. 容器与宿主结果按 `execution_mode` 分开统计，不能合并为一组通过率；
11. 结果通过后才进入草案/方案阶段。

---

## 9. 明确不做

- 不迁移 crate 内仿真测试；
- 不复活 L10；
- 不把测试子项目变成新的 backend；
- 不读取平台窗口几何作为真值；
- 不默认改变用户当前 compositor 配置；
- 不把日志解析失败当作测试失败或测试通过；
- 不在第一版引入 serde/JSON 依赖；
- 不删除原有 live CLI 和脚本，除非后续兼容方案获批。
- 不把 `live-container` 的结果与 `live-host` 合并统计。
- 不默认挂载 `$XDG_RUNTIME_DIR`、Wayland/X11 socket、整个 HOME 或 `/dev/dri`。
- 不把 SIGKILL、宿主崩溃或 compositor 重启后的配置状态声称为已恢复。

---

## 10. 十项裁决（结合当前项目实际情况）

> 本节已按当前仓库、已有命令、真实 Niri 测试结果、workspace feature 结构和 AGENTS 约束逐项裁决。结论不是“十项都无条件通过”；带有“有条件”的项目必须在实现前满足对应门槛。

### 10.1 ✅ 子项目名采用 `fidus-test`

**裁决：采用。** 名称直接表达它是 fidus 的测试工具子项目，与 `fidus-core`、`fidus-calibrate`、`fidus-estimate` 的 workspace 命名一致；不使用 `fidus-tests`，避免被误解为只包含 Cargo integration tests，也不使用 `fidus-harness`，因为它还包含 probe 和 live 工具。

### 10.2 ✅ 第一阶段保留双入口，但新入口不得复制生产逻辑

**裁决：接受，且要求兼容入口优先做薄转发。** 当前 `fidus-live-calibrate` 已被文档和实测脚本使用，直接删除会制造无收益的破坏。新 `fidus-test` 应共享一个实现函数或公共工具模块；旧入口只负责调用它并保持旧命令/退出码。禁止维护两份完整 CLI。

迁移期间的“副本”只能是 binary target 的兼容包装，不是复制 `crosshair.rs`、backend 或解析器。等新命令经过至少一轮真实桌面验证后，再另行决定旧入口是否弃用；本提案不授权删除。

### 10.3 ✅ 强制 `--allow-output-mutation`

**裁决：接受，所有会改 scale/transform 的命令强制要求。** 当前 `scripts/l9_live_soak.sh` 会主动修改 Niri 输出配置，因此测试工具如果获得同等能力，默认静默修改用户桌面不符合“逃生门必须调用方主动要求”。

无此 flag 时：

- 可以运行只读环境探测、当前 scale 下测试和报告解析；
- 不能切换 scale/transform；
- 不能把“无法执行请求场景”报告成通过。

有此 flag 也不等于恢复成功：实现必须记录旧值、恢复、读回确认；恢复失败为 `RecoveryUnverified` / harness failure。

### 10.4 ✅ 第一版采用稳定行式 key-value，不引入 JSON 依赖

**裁决：接受。** 当前 workspace 只有 `thiserror` 外部依赖，测试工具第一版不应为了报告格式引入 serde 生态。现有 CLI 是人读文本，不能直接当协议；因此新增固定前缀结果行，例如：

```text
FIDUS_RESULT version=1 kind=environment run_id=r01 status=ready execution_mode=live-host backend=wayland compositor=niri output=eDP-1 scale=1.25 transform=normal
FIDUS_RESULT version=1 kind=calibration run_id=r01 status=ok execution_mode=live-host backend=wayland method=crosshair rms_residual_px=0.203 verification_max_err_px=0.731 consistency_max_err_px=0.856
FIDUS_RESULT version=1 kind=summary run_id=r01 status=ok execution_mode=live-host records_total=2 records_ok=2 records_failed=0
```

字段值必须限制为无空格 token 或经过明确转义；解析失败是 `HarnessError`。人读日志与机器结果行同时保留。JSON 不是永久否决，只有字段稳定且有外部消费需求时另提案。

### 10.5 ✅ 测试子项目暂不包含生产仿真测试

**裁决：接受。** 当前仿真测试紧贴 `FakeIo`、检测器和估计器内部边界，留在原 crate 能更早发现实现回归。`fidus-test` 可以包含**自己的** parser/config/report 单元测试，也可以包含不依赖生产私有实现的 harness 测试；“不迁移仿真测试”不等于“测试子项目没有测试”。

### 10.6 ✅ 采用 `ci / live-host / live-container` 三模式

**裁决：接受，但实现顺序固定为 `ci → live-host → live-container`。** `ci` 解决可重复构建和报告测试；`live-host` 保留真实用户桌面证据；`live-container` 只验证容器访问宿主会话的入口兼容性。三种模式的结果必须分开，不能合并失败率或质量分布。

`live-container` 首版不作为跨平台真实性能验收，只作为显式实验模式。

### 10.7 ✅ 由宿主 runner 独占 output mutation authority

**裁决：接受。** 当前真正能可靠控制 Niri 的是宿主用户会话；容器内 runner 不应同时执行恢复逻辑。宿主 runner 是唯一可以调用 compositor 控制命令的层，容器只运行测试 binary 并回传结果。

如果未来某 compositor 只能从容器内控制，必须另提案证明权限、恢复和竞态；本提案不预授权这种例外。

### 10.8 ✅ 第一版 `live-container` 不挂 `/dev/dri`

**裁决：接受。** 当前 layer-shell + screencopy 实测没有 `/dev/dri` 需求证据。提前授予设备权限只会扩大攻击面与环境差异。缺少 `/dev/dri` 导致测试失败时，先分类为 `EnvironmentUnavailable`，不能自动加权限或降级通过。

只有出现可复现、明确指向 GPU 设备的 backend 错误时，才另提案最小设备授权。

### 10.9 ✅ 接受 `RecoveryUnverified` 终态

**裁决：接受。** 对 `SIGKILL`、宿主崩溃、compositor 重启等不可观测路径，工具无法证明原 scale/transform 已恢复。把它报告为成功会污染测试结论，也可能留下用户桌面状态。

可处理路径必须测试：正常退出、SIGINT、超时、子进程返回错误。不可处理路径只能报告未验证，不允许承诺自动修复。

### 10.10 ✅ live-container 与 live-host 结果绝不合并统计

**裁决：接受。** 容器会改变启动、调度和 socket 时序；live-host 测量的是用户真实桌面，live-container 测量的是“容器入口 + 宿主显示会话”。两者问题定义不同，合并会制造虚假的样本量和通过率。

报告、目录、验收结果和历史趋势都必须带 `execution_mode`，缺失该字段的旧报告不能参与跨模式汇总。

---

## 11. 十项裁决后的二次全审

### 11.1 历史审查结论（提案阶段）

> 本节保留 2026-09 提案审查时的历史结论，不代表当前实现状态。当前状态见文档开头和 `docs/spec.md` §8.6。

**全审结论：有条件通过，允许进入草案阶段；不允许直接写生产实现。**

条件是先实现草案级别的协议与边界验证，而不是先迁移所有工具：

1. 创建空 `fidus-test`，验证 workspace、feature 与 `cargo doc`；
2. 先定义 `FIDUS_RESULT version=1` 行协议及 parser 单测；
3. 以共享逻辑 + 薄兼容入口实现 `fidus-live-calibrate`；
4. 在 `ci` 容器中验证构建、测试、clippy、doc、配置和报告；
5. 在宿主机真实活动桌面验证 `live-host`，保留原 soak 脚本作为对照；
6. 最后才设计 `live-container` 的最小挂载与 UID/GID 权限实验。

### 11.2 全审后的剩余风险（提案阶段历史记录）

| 风险 | 当前处置 | 是否阻塞草案 |
|---|---|---|
| `fidus-test` feature 如何选择 Wayland/X11 | 草案前列构建矩阵实测 | 是，阻塞具体 Cargo 配置 |
| 结果行字段与转义 | 已定版本前缀，字段仍需草案冻结 | 是，阻塞 parser 实现 |
| Niri 输出读回与恢复命令 | 宿主 runner 独占，需实机验证 | 是，阻塞 mutation runner |
| 容器 UID/GID/socket 权限 | live-container 后置实测 | 否，阻塞 live-container，不阻塞 ci/live-host |
| `/dev/dri` 是否需要 | 默认不挂，等待错误证据 | 否 |
| SIGKILL 后恢复 | 明确不可证明，使用 `RecoveryUnverified` | 否，不能把它改成成功 |
| 仿真测试是否迁移 | 明确不迁移 | 否 |

### 11.3 不一致性复查

- 双入口与“不得复制生产逻辑”一致：旧入口只做兼容包装；
- `--allow-output-mutation` 与“逃生门必须主动要求”一致；
- `ci` 无显示、`live-host` 真实显示、`live-container` 显式挂载，三者边界清楚；
- `EnvironmentUnavailable` / `RecoveryUnverified` 不计入 fidus 失败率，但会使场景验收失败；
- 不迁移生产仿真测试与“测试子项目负责跨环境黑盒”一致；
- 不挂 `/dev/dri` 与当前证据一致，不是永久禁止；
- 所有真实桌面结果按 `execution_mode` 分开，避免混样本。

### 11.4 进入草案的门槛（提案阶段历史记录）

只有以下事项完成后，才能把本提案提升为草案：

- 人工接受十项裁决；
- `fidus-test` 的 feature 矩阵有构建证据；
- `FIDUS_RESULT version=1` 的字段、转义、重复键和缺失字段规则确定；
- 旧 CLI 兼容入口的行为与退出码有回归测试；
- `ci` 容器的基础镜像、toolchain、系统包和许可证已锁定；
- mutation authority 与恢复状态模型有可控子进程测试。

---

## 12. 裁决状态

十项裁决已由人工确认；草案已完成伪代码规划并提升为方案。实现仍须遵守 §11.4 的门槛，不得把方案文档当作已完成代码。
