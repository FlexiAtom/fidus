# 草案：核心库完全体收口与边界硬化

> **阶段：历史草案，已转入方案。**
> 当前规范方案见 [`docs/spec.md §8.8`](../spec.md)；本文件保留自审裁枝、工作包和历史决策依据，不单独授权生产实现。
> 本草案回答“剩余工作怎么做”，不把静态审查结论写成已修复事实。
> 进入方案前必须完成草案全审，并由人工批准具体范围。
>
> 依据：`docs/spec.md`、`AGENTS.md` §13、`AGENTS.project-process.md`、当前全量审查记录。
>
> 当前总体结论：核心库主体已实现；P5 方案和最小无显示 runner 已实现；项目仍为“有条件通过／未收口”。

## 1. 草案目标

将当前“主体可用、存在已知高风险边界”的状态，收口为以下可声明状态：

1. 已支持的投射 + 截屏抽象上，不存在已知 Critical/High 级静默错误、NaN 传播或可由公开输入触发的 panic；
2. L0/L1/L4/L7/L8/L9 的调用链和降级语义彼此一致；
3. P5 最小 runner 在无法证明清理或恢复时保守失败，不误报 confirmed；
4. 无显示回归能覆盖每个已修复的纯函数、协议和 fake 生命周期边界；
5. 文档、矩阵、manifest、provenance 和审查记录与实际 revision 一致；
6. 真实 compositor 异常、稳定 identity、跨 compositor 和跨机器验证仍明确标为未完成，不被本草案“收口”掩盖。

本草案不把“完全体”定义为实现 L3、L5、L10，也不把真实桌面实验默认为自动化门禁。

## 2. 提案自审裁枝记录

本节记录从“全部可能缺口”到本草案范围的自审，不等同于人工批准或草案全审。

### 2.1 保留并进入草案

| 候选 | 裁决理由 |
|---|---|
| Fused confidence ceiling 全链路 | 直接关系 L1 测量是否以错误高置信度进入 L7/Kalman，属于静默错误，收益明确且可用无显示测试验证 |
| 非有限输入、坏 Frame、尺寸/坐标溢出、负 ROI、空图和 `passes` 边界 | 公开入口可触发 NaN、越界或 panic，违反“不确定输入不得伪装为测量”的核心原则；可按入口逐项测试 |
| Anchor 一般 affine、多轮一致性、quality/map 配对和验证点数量 | 现有检查可能拒绝合法变换或接受不充分验证，直接影响校准正确性；无需真实桌面即可构造 shear/多轮模型 |
| Wayland screencopy/marker release 生命周期 | 属于协议资源生命周期，`ready` 与 `release` 语义不能混淆；必须保留为后端正确性工作包 |
| P5 runner 清理、partial apply、TOCTOU、锁错误和信号语义 | 可能误报 teardown 或覆盖用户状态，且涉及安全边界；保留，但真实破坏性实验仍单独授权 |
| wrapper summary/child exit/broken pipe | 直接影响 CI 是否把失败判为成功；协议和退出码可在无显示环境验证 |
| 文档、矩阵和当前 revision 发布绑定 | 当前历史文档与旧 artifact 已出现事实漂移；不修复就无法判断“完成”究竟对应哪个源码版本 |

### 2.2 合并或降级为验收条件

- “innovation/motion consistency gate”不单独承诺为新增算法；先确保已有 confidence ceiling 在完整调用链生效，只有周期别名回归仍能穿透时才提出独立方案。
- 周期/渐变/合成 PNG 不单独扩展为图像框架；只增加能证明 L1→L7→Kalman 边界的最小 fixture，若现有测试入口无法消费 PNG，则文档只声明 PNG 可复现，不虚称算法覆盖。
- Wayland 真实 compositor、P5 异常桌面、跨机器发布属于证据验收条件，不作为本轮自动实现范围。
- shellcheck、更多平台和外部签名信任根属于工程增强或发布运营，不阻塞核心代码硬化方案。

### 2.3 删除或继续挂起

- L3 Beacon、L5 TUIScan、L10 GradientField、L2.5 PointerProbe：与当前核心目标无关，分别维持移出、否决或删除状态。
- 立即实现跨 compositor、稳定 identity、SIGKILL/compositor restart 自动化：当前没有授权环境，也不能用 fake 证据替代真实证据。
- “实现全程离线构建”作为独立目标：当前需求是离线验证契约；Docker 的联网准备阶段不应被包装为已全程离线，也不应在本草案扩张为新的构建系统。
- 将声明式 SBOM 伪装成完整供应链证明：保留现有限制说明，只有明确的逐层/逐包需求才另立计划书。

自审结论：保留 WP-A–G 的方向，但删除其不必要扩张；WP-A 的 innovation gate、PNG 语义接线、真实桌面和跨机器项必须以条件/证据形式处理，不作为无授权的默认实现。

## 3. 代码现状裁决

### 2.1 保留为核心的层

| 层 | 当前归属 | 草案判断 |
|---|---|---|
| L0 Anchor | `fidus-calibrate` | 保留；硬化一般 affine、多轮验证和输入边界 |
| L1 Fingerprint | `fidus-estimate` | 保留；硬化歧义置信度传递 |
| L4 Relative | `fidus-estimate` | 保留为运动模型和 Kalman 状态转移 |
| L7 Fusion | `fidus-estimate` | 保留；必须统一 confidence ceiling 和 innovation 约束 |
| L8 EdgeSync | `fidus-estimate` | 保留；拒绝非有限观测，维持时间窗口语义 |
| L9 Crosshair | `fidus-calibrate` | 保留；真实异常验收继续挂起 |

### 2.2 明确不在本草案实现的层

- L2 不恢复为独立的全局指针输入层；交互位移继续由 L4 运动模型表达；
- L3 Beacon 继续移出核心，待未来独立 `fidus-extras` 提案；
- L5 TUIScan 继续移出核心，待未来独立提案；
- L6 不恢复为独立层，继续并入 L1 Fingerprint；
- L10 GradientField 维持否决和 feature-disabled 占位，不复活旧方案；
- L2.5 PointerProbe 维持删除。

## 3. 工作包

### WP-A：Fused 置信度与错误吸收

**问题**：Fused 路径可能绕过 ambiguous-template 的 confidence ceiling，令渐变或周期别名以 raw confidence 进入 Kalman。

**拟定接口约束**：

- L1 输出必须携带“原始质量”和“可接受置信度上限”两个不可混淆的量；
- L7 只能吸收应用 ceiling 后的 confidence；
- 默认不可定位模板仍返回拒绝；显式 opt-in 只能降低可信度，不能恢复满置信；
- 必要时增加 innovation/motion consistency gate，但不得把预测值伪装成测量。

**无显示验收**：固定合成渐变、约 6px 周期纹理、多峰纹理、真实可定位模板，分别验证拒绝、降权、正常跟踪和 Kalman 不被错误位置吸收。

### WP-B：外部输入与数值边界硬化

统一处理所有公开输入：

- `NaN`、`+Inf`、`-Inf` 的 affine/correspondence/MotionGate 输入；
- `Frame` 宽高、stride、buffer 长度和 `rgba_at` 边界；
- `from_raw` 尺寸乘法、BoundingBox 极值运算；
- Fused 负 ROI、`min > max`；
- 空图、`1×1` 图和空输出尺寸；
- Anchor `passes=0`、验证点不足和低 scale 小 marker。

**统一规则**：坏输入返回 `Err`/`None` 或明确拒绝，不使用 epsilon、clamp 或默认值把坏输入包装成合法测量。

**验收要求**：每个入口至少有一个 malformed/极值测试；测试必须断言“不 panic”，并检查错误分类稳定。

### WP-C：Anchor 几何与多轮一致性

- 将“对角线等长”改为一般 affine 真正保持的不变量，例如平行四边形关系和对角线中点一致；
- 所有 pass 都参与一致性判断，不只比较首尾；
- map、quality、验证点必须来自同一有效 pass 或同一明确聚合结果；
- `passes=0` 和验证点不足必须在配置/运行阶段失败；
- 增加 shear、非均匀缩放、旋转组合和小 marker 仿真。

### WP-D：Wayland buffer 生命周期

- screencopy buffer 只在收到 compositor `release` 后复用；
- marker buffer 只在 release 后复用或销毁；
- `frame.ready` 只表示内容可读，不等价于 buffer 可复用；
- 在 session 状态机中明确 ready、failed、released、destroyed 的顺序。

**验收**：协议级 fake 覆盖延迟 release、连续 capture、快速 show/clear、failed 后 release；具备显示环境后再做真实 compositor 单点验证。fake 结果不得升级为真实证据。

### WP-E：P5 runner 安全收口

- escaped session/setsid 无法证明清理时，必须报告 `teardown=unknown` 或 `recovery=unverified`；
- 每个成功 setter 后记录实际已应用状态；partial apply 也必须恢复已知状态；
- restore 前后都要检查外部变化；检查与 setter 间不可证明的 TOCTOU 必须保守失败，不默认覆盖；
- lock 目录清理失败必须诊断并升级结果；
- runtime mount 参数采用严格 allowlist/结构化参数，不能依赖逗号拼接字符串；
- apply/read-back/child 各阶段收到信号都必须进入不可重入的恢复路径；
- wrapper 的 stdout/stderr 写出失败不得静默吞掉。

**验收**：escaped session、第二 setter 失败后外改、read-check 后外改、lock cleanup failure、特殊 mount 字符、in-flight signal 各有 fake 回归；真实异常项继续列为 real-pending。

### WP-F：协议 wrapper 与证据链

- 保持 `summary.status`、业务记录状态和 counts 三者一致；
- child exit=0 但 summary failed/harness_error 时 wrapper 必须非零；
- malformed public record 必须返回错误而不是 panic；
- stdout/stderr broken pipe 有明确 harness error 语义；
- 为 `live-calibrate` 增加真实 wrapper integration test，而不只测试纯函数。

### WP-G：文档与发布收口

- 将 `docs/spec.md §11.6` 分层为“最小 runner 已实现、真实异常/稳定 identity 未完成”；
- 在 proposal §11、draft §10 等历史段落增加“历史快照，不代表当前阶段”标识；
- 将 F42 拆成 fixture/protocol pass 与真实 Niri 观察边界；
- 审查记录每轮绑定明确 `input_revision`；
- 只在 clean 当前 HEAD 上生成 archive、image ID、manifest、SBOM、provenance 和签名；
- `verify_release_image.sh`、manifest 和 provenance 必须互相验证 source commit/tree、Dockerfile 输入摘要和 artifact 摘要；
- GPG agent、跨机器验证或 registry manifest 缺失时保持 fail-closed。

## 4. 执行顺序与准入

本草案不直接进入代码实现。建议执行顺序：

```text
WP-A/WP-B 纯算法与输入边界
→ WP-C Anchor 几何
→ WP-D Wayland 生命周期
→ WP-E P5 runner
→ WP-F wrapper 协议
→ WP-G 文档与发布
→ 全量审查
```

每个工作包进入实现前必须提供：

1. 目标调用链和收益对象；
2. 失效模式；
3. 接口/状态机变化；
4. 无显示测试矩阵；
5. 不需要真实桌面即可验证的证据；
6. 对真实 compositor 或跨机器部分的明确挂起声明。

以下事项需要额外明确授权，不由草案自动授权：

- 真实桌面 mutation；
- SIGKILL、compositor 重启等破坏性异常实验；
- 跨机器发布验证；
- push 或正式发布。

## 5. 验收矩阵

| 范围 | 草案准入结果 |
|---|---|
| 编译、纯函数、协议、fake adapter | 必须全绿 |
| F1–F42 无显示矩阵 | 修复后重新运行；仍允许 `real-pending`，不得伪造 pass |
| 真实 Niri 正常路径 | 只记录本机观察，不产生稳定 identity |
| 真实异常恢复 | 需要单独授权，失败也保留挂起/未验证 |
| 跨 compositor | 不由 Niri 证据推导 |
| 跨机器发布 | 单独环境和单独记录 |
| SBOM/signature/provenance | 必须绑定同一 clean source revision；本机密钥不等于外部信任根 |

草案阶段的最低结论是：所有工作包的接口、失效模式和无显示验收矩阵自洽；不要求在草案阶段宣称代码修复完成。

## 6. 非目标与禁止事项

- 不为了让矩阵变绿而放宽拒绝条件；
- 不把 `real-pending` 改成 `pass`；
- 不将 fake、模型、静态图片或协议观察写成真实桌面证据；
- 不用当前旧 release artifact 反推当前源码已发布；
- 不将 L3/L5/L10 重新塞回核心；
- 不在草案未审、方案未批准前实现新增生产行为。

## 7. 草案阶段审查问题

全审至少回答：

1. WP-A 的 confidence ceiling 是否在所有 L1→L7→Kalman 调用链生效？
2. WP-B 的拒绝是否覆盖每个公开输入入口，且没有 fallback 伪造测量？
3. WP-C 的几何不变量是否对一般 affine 成立？
4. WP-D 的 release 状态机是否允许安全复用且不依赖 `ready` 误判？
5. WP-E 是否在“无法证明清理”时保守，而不是报告 confirmed？
6. WP-F 的 child exit、summary 和 broken pipe 语义是否唯一且可测试？
7. WP-G 的历史文档、当前 HEAD、manifest、provenance 和签名是否同一事实时间线？
8. 这些改动是否仍遵守“投射 + 截屏”原语和五条设计原则？

## 8. 当前阶段结论

```text
草案：待全审
生产实现：未授权
项目总体：有条件通过／未收口
```
