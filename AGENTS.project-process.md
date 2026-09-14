# fidus 项目进程附件

> 本文件是 [`AGENTS.md`](AGENTS.md) 的项目进程附件：记录当前正在推进的草案、已冻结的方案、挂起事项和即将进行的任务。
> 它不替代 AGENTS.md 的设计原则与工程约定；若发生冲突，以 AGENTS.md 和 `docs/spec.md` 为准。

## 1. 流程总览

新方向严格经过：

```text
提案 → 自审裁枝 → 草案 → 方案 → 可执行性评估（必要时计划书）→ 实现 → 全量审查
```

各阶段的准入和产出：

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
| P5 output mutation/recovery | 无显示实现级门槛已完成；真实异常证据挂起 | `docs/spec.md §8.7`、`docs/drafts/P5-output-mutation-recovery.md` | 当前只剩用户明确授权下的真实 compositor/宿主异常实验；稳定 identity 在当前 Niri 上不可证明，NameOnly 不得 confirmed |
| P5 exit-code/summary truth table | 已实现保守 runner 语义并完成 fake 回归 | `docs/spec.md §8.7.3`、`docs/measurements/p5-f1-f42-matrix.md` | lifecycle+summary 单次输出、recovery code 4、child/timeout/external-change 优先级已验证；真实异常结果仍挂起 |

“正在进行”表示仍有验收或实现工作，不表示已经完成或可以对外承诺。

### 2.3 挂起事项

以下两项是明确挂起，不是通过，也不是失败；恢复前不得从清单删除：

1. **跨机器发布验证：挂起**。当前记录只覆盖本机产物、镜像、归档和环境，不能宣称跨机器复现。
2. **P5 真实桌面错误处理单点验证：挂起**。当前无法安全、独立地完成该单点验证；fake Niri 和无显示回归不能替代真实 compositor 异常证据。

挂起期间可以继续进行不依赖这些证据的纯函数、fake adapter、协议和文档工作，但不得把其结果升级为真实桌面或跨机器结论。

## 3. P5 完成后的强制全量审查

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
- 两项挂起事项不得被伪造为完成；
- 若重新进行真实桌面实验，必须假设用户正在使用电脑并取得明确授权；
- 未经明确请求不得 push 或发布。

全量审查的产出应记录到 `docs/review-process.md`，并给出：

```text
通过 / 有条件通过 / 阻塞
```

审查结论为“有条件通过”时，必须列出剩余门槛；不能只写“总体通过”。

## 4. 即将进行的任务

按优先级执行：

1. **保持 P5 无显示回归全绿**：F1–F42 逐项映射见 `docs/measurements/p5-f1-f42-matrix.md`；当前 real-pending 项不得伪造为 pass。
2. **保持 exit-code/summary 语义一致**：`not_requested | confirmed | unverified`、recovery code 4、child/timeout/external-change 和唯一 summary 已由 runner fake 回归覆盖。
3. **全量审查后核心边界修复执行中**：当前审查见 `docs/review-process.md` §7，批准范围见 `docs/proposals/core-hardening-followup.md`。本轮已修复 Gate input-region 能力误报、无面积先验时的多 blob 最大候选误选、目标模板伪造尺寸/资源预算、initial center 非有限值，以及 Anchor/Crosshair 配置计数、有限性、scale 区间和工作预算，并补充对应无显示回归；Engine 估计结果和 `RgbaImage::is_valid()` 也已加入 fail-closed 边界。Wayland 完整 deferred retirement fake 状态机、X11 真实协议 fixture、ProbabilisticPosition 旧兼容构造的 API 收紧与 checked-in 旧 manifest/provenance 格式仍未完成；X11 marker/capture 几何边界的无显示 fixture 已补充；SearchRoi 大 ROI 预算已实现并有无显示回归，且保留正常全屏搜索；`BufferState::may_reuse()` 已统一 release 复用边界并补状态回归，尚未替代完整 deferred retirement 模拟；open 失败陈旧 proxy 清理和 timeout 后 projector poisoned 保护已实现；`try_new()`、Engine 终局校验、签名脚本防重绑定、build_release SBOM/tag 输出和 verifier manifest reference 已实现并通过离线/语法边界验证。RgbaImage 字段已私有化并迁移到 checked constructor/getter/validity API。真实 compositor 异常、稳定 identity、跨机器验证、正式发布和 push 仍排除。
4. **供应链与离线门禁**：vendor 离线 CI、声明式 SPDX SBOM、OpenPGP signature 与 SLSA provenance attestation 已实现并通过本机验证；SBOM 离线校验现精确比较包名、版本、来源、checksum 和依赖关系；签名是本机发布者密钥，不等于外部 CI 身份。
5. **下一轮候选**：修正发布产物与当前 source revision 的绑定，补 registry manifest/跨机器验证；评估是否把经典壁纸纳入不含外部版权素材的可复现合成测试；shellcheck 仅在工具纳入 CI 后执行。
   - 本轮已裁决并实现：不引入图片依赖、不读取外部壁纸，采用 `tools/generate_test_images.py` 生成 fractal/texture/gradient/periodic 四类 RGB PNG，并以 `tests/fixtures/images/manifest.json` 登记尺寸与 SHA-256；`tools/test_generate_test_images.py` 已接入 `scripts/ci_fidus_test.sh` 无显示门禁。
   - 实际证据：`python3 tools/test_generate_test_images.py`（4 fixtures verified）、Python 语法检查和 `git diff --check` 通过。未宣称 workspace Rust 全量验证，本轮新增范围仅为 Python fixture 回归。
- 发布重绑定当前状态：已实现 `scripts/source_revision.py` 与隔离输出的 `scripts/build_release.sh`；当前 HEAD 镜像已完成重建和归档候选，source binding 工具已验证；本机 GPG agent 在最终签名阶段超时，故仓库正式发布文件未更新，旧 revision 仍不得改写为当前发布声明。
6. **处理挂起事项**：只有获得适当环境和明确授权后，分别恢复跨机器发布验证与真实桌面错误处理单点验证。
7. **审查通过后再决定发布**：push 或发布仍需单独确认，不因全量审查有条件通过而自动执行。

## 5. 任务变更规则

- 新任务开始前更新本文件的“当前项目状态”或“即将进行的任务”。
- 任务完成后记录实际证据和未完成边界；不得用“代码存在”替代“测试通过”。
- 新增草案必须登记路径、阶段、准入条件和下一动作。
- 任何挂起事项恢复时，记录恢复原因、授权、环境和结果；失败或无法验证时继续保留挂起状态。
- 任何人要求“继续”时，先依据本文件确认当前阶段，避免跳过全量审查或方案准入。
