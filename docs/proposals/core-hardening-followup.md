# 提案：全量审查后核心边界修复

> 阶段：已获人工批准的 R1–R4 范围；本轮实现已完成部分工作包，未关闭项仍需后续计划与复审。
>
> 依据：`AGENTS.md` §9–§13、`docs/spec.md` §8.8、`docs/review-process.md` §7。
>
> 当前结论：项目仍为“有条件通过／未收口”。本提案只处理无显示环境可验证的代码边界，不把 fake 证据升级为真实 compositor 或发布证据。

## 1. 为什么现在做

全量审查确认 WP-A–G 已覆盖一部分输入、融合、校准、buffer 正常提交和 P5 runner 边界，但仍有跨调用链风险：

- `FidusEngine::calibrate()` 没有把 Gate 变成强制状态转换；
- Wayland marker 更新、失败打开和 teardown 尚未形成完整的 configure/release 生命周期；
- EdgeSync 入口没有统一拒绝坏 Frame；
- 失败重校准可能保留旧 frame；
- 若干公开几何、配置和协议整数入口仍缺 fail-closed 校验。

这些问题的共同后果不是单纯报错，而可能是绕过能力门控、使用错误坐标、提前释放 compositor 仍持有的内存，或由 malformed 外部输入触发 panic。因此收益对象是调用方安全、概率池纯净性和后端协议正确性。

## 2. 自审裁枝

### 保留，进入本次修复

| 工作包 | 代码入口 | 保留理由 | 无显示证据 |
|---|---|---|---|
| R1 Gate 强制门控与重校准事务 | `fidus-core/src/engine.rs`、`gate.rs` | 直接修复 Engine 绕过 Gate 与失败后旧 frame 继续可用 | mock Gate/Calibrator/Io，断言拒绝时不打开 IO；失败重校准后 `NotCalibrated` |
| R2 EdgeSync/Classifier 坏 Frame fail-closed | `fidus-estimate/src/edge_sync.rs`、`screen_classifier.rs` | 公开 Frame 可 malformed，必须在差分/分类入口阻断 | 短 data、坏 stride、尺寸溢出、reference 不匹配均返回错误/动态，不 panic |
| R3 Wayland capture/marker 数值与生命周期 | `backend-wayland-layer/src/{capture,marker,session}.rs` | release 与 ready 语义是协议安全边界；几何转换存在截断/溢出风险 | fake 状态机覆盖延迟 release、失败 release、open 中途失败、checked geometry；真实 compositor 仍 pending |
| R4 公开配置和几何边界 | `core/{coord,frame,io,target}.rs`、`calibrate/{anchor,crosshair,detect}.rs`、X11 backend | 修复 NaN/Inf、负值、空框、整数截断和 checked arithmetic 的公共入口 | hostile-input 单测与不 panic 断言 |

### 删除或另立提案

- 不在本轮实现跨 compositor、稳定 identity、真实 SIGKILL/compositor restart 或真实桌面错误注入；这些需要环境与单独授权。
- 不在本轮改进 localizability 算法、创新 motion gate、周期纹理理论或新增图像框架；当前没有证明它们是 R1–R4 的必要依赖。
- 不在本轮重建 release artifact、签名、registry manifest、跨机器验证或 push。
- 不把所有 Medium 风险一次性扩张成“完全无风险”承诺；只修复有明确入口、调用链和无显示验收的项目。

## 3. 方案方向与失效模式

### R1：Gate 与 frame 事务

`calibrate()` 在打开 IO 前取得 calibrator method，并查询 `gate().query_calibrator_availability()`；仅 `Available/Degraded` 可继续。校准开始时暂时清除旧 frame；校准失败保持无 frame。若成功再一次性提交新 frame。

失效模式：Gate 的环境快照可能在打开会话后变化；本方案不把静态 Gate 当实时 compositor 证明，后续 backend 错误仍必须传播。若校准成功后提交 frame 的内部不变量失败，保持旧 frame 不应发生，改为返回错误并保持无 frame。

### R2：坏 Frame

所有消费像素的入口先调用 `Frame::is_valid()`，并验证 reference 与当前帧的几何/stride/format 一致；差分索引使用 checked arithmetic。坏输入返回明确错误，Classifier 不能把缺失字节当黑色证据。

失效模式：Frame 在检查后若被内部可变代码改变，仍需在切片访问前保持 checked 边界；不得以一次入口检查替代每个不安全索引的证明。

### R3：Wayland 生命周期

引入 configure generation/pending 状态，后续 surface commit 必须等待对应 configure；open 失败使用局部 cleanup guard；buffer retirement 只在 release 后销毁或替换 SHM 资源。`ready` 只允许读取，不能授权复用或释放。

失效模式：compositor 永不发送 release 时不能无限阻塞；超时必须错误退出并保留“无法证明已释放”的状态，不能强行销毁后宣称成功。fake 测试不得证明真实 compositor 行为。

### R4：公共边界

新增统一的 validated constructors 或入口检查：finite、非负/正值、协议整数范围、checked multiplication、合法 ROI/BoundingBox、非零尺寸和容量上限。拒绝退化输入，不使用 `clamp`、epsilon 或默认值伪造测量。

失效模式：过严的上限可能拒绝合法高分辨率输入；上限必须以现有协议/内存模型为依据并在错误信息中保持可诊断，不能偷偷缩放或截断。

## 4. 可执行性评估

- 目标：关闭 R1–R4 的公开调用链风险，并保持五条设计原则与 §8.8 边界。
- 顺序：R1 → R2 → R3 → R4 → 回归与全量审查。
- 依赖：现有 `Frame::is_valid`、Wayland `BufferState`、`Gate` trait、mock/fake IO；不新增外部依赖。
- 验收：新增单测/仿真先通过；随后 `cargo test --locked --offline --config .cargo/config.ci.toml --workspace`、clippy `-D warnings`、doc、脚本语法、P5 矩阵、runner、fixture、`git diff --check` 全绿。
- 回滚：每个工作包保持独立提交式差异（本工作树不自动提交）；若协议状态机无法在 fake 中证明安全，回退该包，不放宽拒绝条件。
- 授权条件：只允许无显示、纯函数、fake adapter、协议模型和文档修改；真实桌面 mutation、破坏性实验、跨机器验证、发布和 push 明确排除。
- 完成定义：R1–R4 的列举入口有 fail-closed 测试，审查记录更新为实际结果；未覆盖的真实 compositor、稳定 identity、发布 provenance 仍明确为 pending。

## 5. 提案结论

R1–R4 已获人工批准并进入实现。本轮已完成的无显示工作包包括：Engine Gate 与重校准事务、坏 Frame fail-closed、input-region 能力拒绝、无面积先验多 blob 拒绝、RgbaImage 封装与模板资源预算、Anchor/Crosshair 配置边界、公开 estimate 终局校验、Wayland open 失败清理/poisoned timeout 保护，以及 release 脚本的 source/artifact、SBOM、tag 契约加固。对应 workspace、clippy、rustdoc、脚本、P5 matrix、runner、fixture 和 diff 门禁均通过。

仍需单独计划并复审：完整 Wayland deferred retirement fake 状态机、X11 protocol fixture、大 ROI/search budget、checked-in 旧 manifest/provenance 迁移。真实桌面、跨机器、正式发布、签名和 push 继续排除。
