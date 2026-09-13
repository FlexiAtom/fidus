# 提案审查流程：快审与全审

> 本文记录项目内部的审查层级。它不是对 AGENTS §9/§10 的替代；它规定的是**如何在不同时间预算下执行审查**。

## 1. 快审是什么

**快审（quick review）**是对一个提案的短时间风险筛选，目标是回答：

> 这个方向是否存在明显收益，是否有立即可见的原则冲突或致命边界？

快审可以在提案正文尚未完整时进行，也可以只基于主张、目录和当前架构进行。它适合决定“值得不值得进入全审”，不适合授权实现。

快审至少检查：

1. 是否与五条设计原则直接冲突；
2. 是否会复制生产实现或偷偷引入第二套真值；
3. 是否有明显的 workspace / API / 生命周期破坏；
4. 是否有清晰的收益对象；
5. 是否能指出主要失败模式。

快审可以暂时跳过：

- 核心数值假设的实测；
- 完整仿真回归测试；
- 所有平台与 compositor 的矩阵；
- 性能、依赖、报告兼容性细节；
- 实施顺序与迁移兼容性。

**快审结论只能是**：

- `通过快审，进入全审`；
- `有条件进入全审`；
- `方向否决`。

“通过快审”不等于“批准实现”。

## 2. 全审是什么

**全审（full review）**是提案进入草案/方案前的完整审查。它必须回答 AGENTS §10 的三个问题：

1. 核心假设成立吗？是否已经实测并把数字贴进提案？
2. 方案解决谁的问题？现有方案在哪个真实场景不够？
3. 方案内部自洽吗？组成部分是否互相拆台？

除 AGENTS §10 外，全审还必须检查：

- 生产 API 与测试/工具 API 的边界；
- 零信任与概率池纯净性；
- 权限、teardown、取消、超时和恢复路径；
- 默认行为是否会改变用户当前环境；
- 逃生门是否是调用方主动要求；
- 兼容命令、迁移和弃用策略；
- 依赖新增及其许可证；
- cargo test / clippy / doc；
- 报告格式、退出码和日志解析失败语义；
- 仿真回归测试与真实场景验证；
- 失效模式注释和参数是否真的被实现读取；
- 代码与文档中的可验证声称是否有证据。

全审可以推翻快审阶段看起来有收益的方向。推翻不是流程失败，而是防止收益主张在实现阶段变成返工。

## 3. 两种审查的关系

```text
提案主张
  ↓
快审：是否值得投入全审？
  ├─ 否决 → 结束
  └─ 通过 → 全审：假设、边界、实现、自洽性
                   ├─ 纠偏后继续提案
                   ├─ 转草案
                   └─ 否决
```

快审追求**尽快发现明显坏方向**；全审追求**在写实现前尽量消除返工**。二者优化目标不同，因此不能用快审替代全审，也不能要求每个早期想法一开始就承受完整审查成本。

## 4. 本次 P4 测试子项目的记录

本次“增加独立测试子项目”先经过快审：收益明显，方向没有立即违反零信任原则，因此进入全审。

全审随后发现了快审未覆盖的风险：

- 直接迁移 CLI 会破坏已有命令；
- 依赖 `fidus` 的默认 feature 会造成 backend 构建歧义；
- 场景运行器修改 scale/transform 必须有调用方主动要求的逃生门；
- 用户声明“我正在操作”不能替代噪声底测量；
- 解析生产 CLI 文本不能自动成为稳定协议；
- 不应把真实桌面通过率写成跨平台保证；
- `corroboration`、probe、报告和迁移边界都需要独立测试；
- 容器不等于显示隔离：`ci`、`live-host`、`live-container` 必须分开，挂载 socket、设备和输出修改权必须显式记录；
- 强杀、宿主崩溃和 compositor 重启后的恢复状态不可伪造，必须允许 `RecoveryUnverified`。

因此当前结论是：

> **P4 的 CI 协议核心、Debian CI build/run、live-host、本机 live-container 和 Debian runtime Release 资产已完成实测与收口。**

十项裁决与伪代码方案仍有效；协议 kind schema/终态、旧 CLI 兼容、probe facade 边界、唯一 owner 和 teardown 合法证据已冻结。本机临时镜像已通过当前 Wayland socket 完成同 UID 校准；不同 UID、缺 socket、非法 runtime 已分别得到预期的环境/工具错误。宿主参数契约有 `scripts/test_live_container.sh` 自动测试。正式交付已锁定 DaoCloud Docker Hub mirror 上的 Debian 12 slim digest、Rust 1.86.0 toolchain 和系统包；本句保留为 P4 收口前的历史记录，后续 P5 审查不重复打开该门槛。

## 5. 当前 P5 记录

P5 output mutation/recovery 提案已获人工批准，草案全审有条件通过，方案已并入 `docs/spec.md` §8.7。当前草案为 [`docs/drafts/P5-output-mutation-recovery.md`](drafts/P5-output-mutation-recovery.md)，已定义宿主 adapter、快照、状态机、锁、失败优先级和 fake adapter F1–F42；全审已发现并修正生命周期早退、未授权分支、NameOnly 误确认、矩阵聚合、信号监督、锁原子性和 parser 边界问题。七项设计决策已写入草案，其中 recovery 三值已落到 `fidus-test` parser 和单测；最小 `scripts/output_mutation_runner.sh` 和 fake Niri 回归已实现；审计覆盖显式授权、snapshot 后早退恢复、recovery 优先、实际 PGID、symlink lock、ASCII 控制字符、malformed/deferred parser 和 flock 竞争。真实 Niri 异常路径、完整 timeout/signal 矩阵和跨 compositor 身份证明仍是后续验收门槛。其中“真实桌面错误处理单点验证”按用户要求挂起；“跨机器发布验证”同样挂起，二者均不得标记为通过。本轮 `cargo test/clippy/doc` 尝试因 Cargo registry 缓存目录只读而未启动：默认缓存报 Read-only file system，项目内离线缓存缺少 `thiserror`；不得将该次尝试记为 Rust 验证通过。实际实验已完成：只读 probe 通过；recovery parser 4/4 通过；`flock` 抢锁拒绝/释放后重获通过；`setsid` 实际 PGID 进程组 TERM→KILL 回收通过；临时信号/恢复模型 5/5 通过；当前 Niri 的 scale apply/read-back/restore 和 transform canonical read-back/restore 通过。

## 6. 审查记录要求

每次对提案执行快审或全审，至少记录：

```text
review_level: quick | full
reviewer: 人或代理标识
input_revision: 提案版本/提交
decision: pass | conditional | reject
skipped_checks: 快审明确跳过的项目
findings: 风险与纠偏
next_step: 继续提案/草案/方案/结束
```

审查记录可以直接写在提案中，也可以放在独立审查文档中；不能只存在聊天上下文里。这样后续接手者能知道：哪些事情已经验证，哪些只是快审判断，哪些问题仍然开放。
