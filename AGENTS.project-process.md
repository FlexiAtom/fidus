# fidus 项目进程附件

> 本文件是 [`AGENTS.md`](AGENTS.md) 的项目进程附件：记录当前正在推进的草案、已冻结的方案、挂起事项和即将进行的任务。
> 它不替代 AGENTS.md 的设计原则与工程约定；若发生冲突，以 AGENTS.md 和 `docs/spec.md` 为准。

## 1. 流程总览

新方向严格经过：

```text
提案 → 自审裁枝 → 草案 → 方案 → 可执行性评估（必要时计划书）→ 实现 → 全量审查
```

各阶段的准入和产出：

- **提案**只是一个可能实现的想法：可以被否决，不能据此声称可行，不能授权实现。
- **草案**是一个可行的想法：包含简单实现细节和限定条件，足以判断“可以按这个方向继续”，但仍不能唯一限定最终程序逻辑。
- **方案**是实现约束：描述必须充分到使实现后的程序在逻辑上与方案一致；若仍存在会改变最终逻辑的空白，必须退回草案或增加计划书。
- **全量审查**必须在审查范围内明确判断：是否还有明显更优雅的替代方案，是否还有明显可继续剪掉的不合理设计，以及是否存在逻辑问题。没有时必须明确记录“未发现明显更优雅替代方案，未发现可继续剪掉的不合理设计，未发现逻辑问题”；“有条件通过”是审查者的条件性结论词，不是项目固定状态，所有条件必须列出。
- **方案冻结**表示已实现大部分的方案被发现无法实现或不值得继续投入，但已有实现可能生效或存在能生效的部分；冻结不等于完成、通过或失败，必须说明冻结的是设计、执行还是结论。
- **自审通过并转入方案**表示没有明显需要剪枝或更优雅替代方案，且设计逻辑没有发现问题；不表示实现完成。
- **人工审阅**表示人工阅读对应阶段文档并在该阶段语境下未发现问题，不自动授权实现；**人工批准**特指对提案、草案或方案文档的批准，不代表实现完成；**明确授权**是更广义的具体动作授权，包含真实桌面 mutation、破坏性实验、签名、发布和 push，人工批准只是其文档审批变体。
- **社区互助处理中**是挂起变体：由人工对接外部人员，本项目中由用户转达他人进度或证据；收到信息后仍须按证据等级重新审查，不能直接变成通过或完成。
- **继续**只有在 Agent 因意外情况中断后才是合法恢复指令；其他语境下必须要求明确动作，不得把“继续”解释为实现、授权或阶段推进。

| 阶段 | 主要问题 | 最低产出 | 是否允许生产实现 |
|---|---|---|---|
| 提案 | 该不该做？核心假设成立吗？ | 实测、收益对象、内部自洽性、失败模式 | 否 |
| 草案 | 怎么做？ | 接口、状态机、失效模式、测试矩阵 | 否 |
| 方案 | 定下来了什么？ | 并入 `docs/spec.md` 的约束和边界 | 经人工批准后可以 |
| 实现 | 是否按方案落地？ | 代码、仿真、文档和回归证据 | 仅限已批准范围 |
| 全量审查 | 整个项目是否仍然自洽？ | 全项目风险、规范、测试、文档和发布审查记录 | 审查完成前不得宣称里程碑收口 |

任何真实桌面 mutation、破坏性实验或发布动作仍需要调用方明确授权；全量审查不能替代这种授权。

## 2. 当前项目状态

### 2.1 已冻结/已完成

- P0–P4 的既有实现和发布记录继续以 `docs/spec.md`、`docs/review-process.md` 和 git 历史为准。
- P5 output mutation/recovery 已完成提案、草案全审和方案冻结，方案位于 `docs/spec.md §8.7`。
- P5 已有最小、显式授权的 `scripts/output_mutation_runner.sh`，默认入口不会调用它。
- `scripts/test_output_mutation_runner.sh` 使用 fake Niri 做无显示回归。
- `fidus-test` 的 P5 fake/fixture 测试已通过；vendor 驱动的 workspace test、clippy、doc、shell 语法、离线契约、SBOM 和 diff 检查已有通过记录。
- 当前 Niri 只有 NameOnly 身份证据；即使字段恢复一致，也不得报告 `recovery=confirmed`。

### 2.2 正在进行的草案/方案

- **核心库完全体收口与边界硬化**：已由 `docs/drafts/complete-state-hardening.md` 转入 `docs/spec.md §8.8` 方案；WP-A–G/R1–R4 已完成部分代码实现与无显示回归，但本轮全量复审发现检测器最大 blob 误选、Gate 未消费 input-region 能力、公开 RgbaImage 可伪造尺寸、配置溢出/资源预算及 Wayland deferred retirement 等 Critical/High 风险仍开放。真实 compositor 异常、稳定 identity、跨机器发布和正式发布仍独立挂起。

| 项目 | 当前阶段 | 依据 | 当前边界 |
|---|---|---|---|
| 核心目标：自身窗口定位 | 核心链路已实现；无显示与本机证据持续维护 | `docs/spec.md §4–§6`、workspace tests | 只定位调用方自身窗口；任意第三方窗口、实例身份和未实现平台不倒灌为核心目标 |
| 安全与完整性：P5 output mutation/recovery | 无显示实现级门槛已完成；真实异常验证已冻结（约等于挂起） | `docs/spec.md §8.7`、`docs/drafts/P5-output-mutation-recovery.md` | 意外恢复不可靠；NameOnly 不得 confirmed；人工恢复仅为补救 |
| 安全与完整性：P5 exit-code/summary truth table | 已实现保守 runner 语义并完成 fake 回归 | `docs/spec.md §8.7.3`、`docs/measurements/p5-f1-f42-matrix.md` | lifecycle+summary 单次输出、recovery code 4、child/timeout/external-change 优先级已验证 |
| 发布与扩展：跨机器发布验证 | 社区互助处理中（挂起变体） | `AGENTS.project-process.md §2.4` | 等待用户转达独立机器的可核验证据，不影响核心自身窗口定位结论 |

“正在进行”表示仍有验收或实现工作，不表示已经完成或可以对外承诺。

### 2.3 挂起事项

以下事项是明确挂起，不是通过，也不是失败；恢复前不得从清单删除：

1. **P5 真实桌面错误处理单点验证：已冻结（约等于挂起）**。不再要求或默认执行真实 compositor 异常实验；TTY/独立终端只能作为观察和人工恢复通道，不能构成隔离或自动恢复证据。意外退出、宿主崩溃或 SIGKILL 后恢复明确不可靠，若曾获单独授权执行 mutation，必须按 `docs/spec.md §8.7.3` 的人工恢复流程处置。

### 2.4 社区互助处理中

“社区互助处理中”与挂起近似：表示问题已移交社区协助或等待外部环境/经验输入，当前没有足够的本机证据将其标记为通过；在获得可核验结果前，不得用于发布承诺或跨环境结论。它与“挂起”的区别仅是已有明确的外部协助路径，不能视为进展或完成。

1. **跨机器发布验证：社区互助处理中**。当前记录只覆盖本机产物、镜像、归档和环境；待社区在独立机器上复现并提供可核验记录后，才能更新状态，当前仍不能宣称跨机器复现。

挂起或社区互助处理中期间，可以继续进行不依赖这些证据的纯函数、fake adapter、协议和文档工作，但不得把其结果升级为真实桌面或跨机器结论。

## 3. P5 完成后的强制全量审查

> **目标校准**：本项目核心完成度的对象是“定位调用方自身窗口”，即调用方可提供离屏渲染模板的窗口。全量审查必须先围绕这一目标评估，再按 `AGENTS.md` §12 执行评估后剪枝；任意第三方窗口定位、窗口枚举/实例身份、未实现平台和跨机器发布属于独立扩展或发布门槛，不能无理由倒灌为核心定位未完成。

P5 的实现工作完成后，下一阶段不是立即发布，而是对整个项目执行一次**全量审查**（不是“全量验证”）。审查至少覆盖：

### 3.1 规范与流程

- `AGENTS.md`、本附件、`docs/spec.md`、提案、草案和 measurement 是否互相一致；
- 五条设计原则是否仍优先于新增功能；
- 是否存在未经提案→草案→方案的实现；
- 当前挂起事项是否被误记为通过；
- P5 的“方案冻结”“最小 runner 已实现”“完整 P5 未收口”是否被准确区分。

### 3.2 代码与边界

- `fidus` 是否仍只依赖投射和截屏原语；
- P5 是否污染坐标真值、概率池或生产 estimator；
- 默认路径是否绝不修改 compositor；
- `live-container` 是否仍拒绝 mutation；
- lock、snapshot、read-back、restore、PGID 和 recovery 优先级是否与规范一致；
- NameOnly 是否仍不能产生 `recovery=confirmed`。

### 3.3 测试与工具链

- `cargo test --workspace`；
- `cargo clippy --workspace --all-targets -- -D warnings`；
- `cargo doc --workspace --no-deps`；
- `bash -n scripts/*.sh`；
- `git diff --check`；
- fake/fixture 与真实实验的证据边界；
- F1–F42 逐项映射及 exit-code/summary truth table。

### 3.4 发布与实机证据

- Debian 镜像 digest、tar.zst、SHA-256 和 image ID；
- 当前已有的本机 Niri 正常路径证据；
- 挂起事项与“社区互助处理中”事项不得被伪造为完成；
- 若重新进行真实桌面实验，必须假设用户正在使用电脑并取得明确授权；
- 未经明确请求不得 push 或发布。

全量审查的产出应记录到 `docs/review-process.md`，并给出：

```text
通过 / 有条件通过 / 阻塞
```

审查结论为“有条件通过”时，必须列出剩余门槛；不能只写“总体通过”。

## 4. 即将进行的任务

- **P5 人工 output 恢复提案已自审通过并转入方案**：`docs/proposals/P5-manual-output-recovery.md` 已核验核心假设并删除自动 watcher、TTY 自动检测、稳定 identity 和生产 Rust API；方案已并入 `docs/spec.md §8.7.4`，实现仅限显式人工脚本与 fake 回归，不重新打开已冻结的真实桌面验证。
- **无显示边界方案已通过全量审查并进入执行**：`docs/drafts/no-display-boundary-closure.md` 已经人工复核并转入 `docs/spec.md §8.8`；正文保留 Wayland deferred-retirement fake 状态机、X11 协议错误 fixture、历史归档标注文件、离线发布元数据治理和兼容构造 API 决策的细化实现细节；历史归档状态标注文件与正向/篡改校验脚本已实现并接入无显示门禁；`verify_release_metadata.py` 已补充 current-bound manifest/provenance 的 artifact、SBOM、source binding、image ID 和 subject 一致性校验及合成负向回归。真实环境证据与正式发布动作明确排除。

按优先级执行：

1. **保持 P5 无显示回归全绿**：F1–F42 逐项映射见 `docs/measurements/p5-f1-f42-matrix.md`；当前 real-pending 项不得伪造为 pass。
2. **保持 exit-code/summary 语义一致**：`not_requested | confirmed | unverified`、recovery code 4、child/timeout/external-change 和唯一 summary 已由 runner fake 回归覆盖。
3. **P5 人工 output 恢复已实现**：`scripts/manual_restore_output.sh` 仅接受显式授权，执行 selector 唯一性检查、双字段 setter、逐字段 read-back 和可选屏外日志；fake 正负回归已接入 `scripts/ci_fidus_test.sh`，不提供自动 watcher，不改变 `recovery=unverified`。
4. **全量审查后核心边界修复执行中**：当前审查见 `docs/review-process.md` §7，批准范围见 `docs/proposals/core-hardening-followup.md`。本轮已修复 Gate input-region 能力误报、无面积先验时的多 blob 最大候选误选、目标模板伪造尺寸/资源预算、initial center 非有限值，以及 Anchor/Crosshair 配置计数、有限性、scale 区间和工作预算，并补充对应无显示回归；Engine 估计结果和 `RgbaImage::is_valid()` 也已加入 fail-closed 边界。Wayland 完整 deferred retirement fake 状态机、X11 真实协议 fixture、ProbabilisticPosition 旧兼容构造已标记 deprecated，后续 breaking release 再评估移除；checked-in 旧 manifest/provenance 格式仍未完成；X11 marker/capture 几何边界的无显示 fixture 已补充；SearchRoi 大 ROI 预算已实现并有无显示回归，且保留正常全屏搜索；`BufferState::may_reuse()` 已统一 release 复用边界并补状态回归，已补测试专用 `BufferEvent` 状态转移模型，覆盖延迟 release、失败后 release、late ready 与 destroy 后 late release，仍未替代真实 compositor 证据；open 失败陈旧 proxy 清理和 timeout 后 projector poisoned 保护已实现；`try_new()`、Engine 终局校验、签名脚本防重绑定、build_release SBOM/tag 输出和 verifier manifest reference 已实现并通过离线/语法边界验证。RgbaImage 字段已私有化并迁移到 checked constructor/getter/validity API。真实 compositor 异常、稳定 identity、跨机器验证、正式发布和 push 仍排除。
5. **供应链与离线门禁**：vendor 离线 CI、声明式 SPDX SBOM、OpenPGP signature 与 SLSA provenance attestation 已实现并通过本机验证；SBOM 离线校验现精确比较包名、版本、来源、checksum 和依赖关系；签名是本机发布者密钥，不等于外部 CI 身份。
6. **下一轮候选**：修正发布产物与当前 source revision 的绑定，补 registry manifest/跨机器验证；评估是否把经典壁纸纳入不含外部版权素材的可复现合成测试；shellcheck 仅在工具纳入 CI 后执行。
   - 本轮已裁决并实现：不引入图片依赖、不读取外部壁纸，采用 `tools/generate_test_images.py` 生成 fractal/texture/gradient/periodic 四类 RGB PNG，并以 `tests/fixtures/images/manifest.json` 登记尺寸与 SHA-256；`tools/test_generate_test_images.py` 已接入 `scripts/ci_fidus_test.sh` 无显示门禁。
   - 实际证据：`python3 tools/test_generate_test_images.py`（4 fixtures verified）、Python 语法检查和 `git diff --check` 通过。未宣称 workspace Rust 全量验证，本轮新增范围仅为 Python fixture 回归。
- 发布重绑定当前状态：已实现 `scripts/source_revision.py` 与隔离输出的 `scripts/build_release.sh`；当前 HEAD 镜像已完成重建和归档候选，source binding 工具已验证；本机 GPG agent 在最终签名阶段超时，故仓库正式发布文件未更新，旧 revision 仍不得改写为当前发布声明。
7. **处理挂起及社区互助事项**：真实桌面错误处理单点验证已冻结，不主动恢复；只有新的明确决策和授权才能重新打开。跨机器发布验证须等待社区提供独立机器上的可核验复现记录后再更新状态。
8. **审查通过后再决定发布**：push 或发布仍需单独确认，不因全量审查有条件通过而自动执行。

## 5. 任务变更规则

- 新任务开始前更新本文件的“当前项目状态”或“即将进行的任务”。
- 任务完成后记录实际证据和未完成边界；不得用“代码存在”替代“测试通过”。
- 新增草案必须登记路径、阶段、准入条件和下一动作。
- 任何挂起事项恢复时，记录恢复原因、授权、环境和结果；失败或无法验证时继续保留挂起状态。
- 任何人要求“继续”时，先依据本文件确认当前阶段，避免跳过全量审查或方案准入。
