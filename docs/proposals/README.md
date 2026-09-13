# 提案目录

新定位思路走 **提案 → 草案 → 方案** 三步（AGENTS §9、§10）。**目的是防止重复修改**：在写代码前把错误方向拦掉，而不是实现到一半才发现原理不成立。

## 三步的分工

| 阶段 | 产出位置 | 回答什么 | 谁来批 |
|---|---|---|---|
| **提案** | `docs/proposals/` | **该不该做？** 核心假设成立吗、解决谁的问题、内部自洽吗 | **人工审阅** |
| **草案** | `docs/drafts/` | **怎么做？** 接口、数据流、失效模式、测试策略 | 人工审阅 |
| **方案** | `docs/spec.md` | **定下来了。** 并入规格，成为规范 | — |

**提案未获批准前不写实现代码。**

## 提案的硬要求（AGENTS §10）

1. **核心假设必须在提案阶段实测**，数字贴进提案，脚本附在末尾可复现。
   不接受"需实机验证（未承诺可用）"——那是把验证推给实现阶段，正是返工的来源。
2. **必须回答"解决谁的问题"**：现有方案在什么场景下真的不够用？答不出就不该实现。
3. **必须检查内部自洽**：各组成部分是否互相拆台。

> **提案可以得出"不做"的结论，这正是它的价值。**
> 拦下一个原理上不成立的方案，代价是几十行脚本和一次审阅；放它进实现，代价是一个里程碑。

## 当前提案

| 提案 | 状态 | 结论 |
|---|---|---|
| [P3 · L10 GradientField](P3-L10-gradient-field.md) | ✅ **已裁决：三个方向全部否决** | 两轮实测：①「对数螺旋 + 锁主频」自相矛盾；②实机发现 L9 的真实短板在**投射端取整**（分数缩放 rms 0.308px），精修类方案不对症。方向 B（否决 L10）采纳，方向 A/C 否决；规格 §4.2 已重写。 |
| [P4 · 独立测试子项目](P4-fidus-test-subproject.md) | ✅ **已转方案** | `fidus-test` 与宿主脚本、容器共存；容器分为 `ci` / `live-host` / `live-container`；伪代码与替代路径全审见 [`docs/drafts/P4-fidus-test.md`](../drafts/P4-fidus-test.md)，规范见 spec §8。 |
| [P5 · output mutation/recovery](P5-output-mutation-recovery.md) | ✅ **已转方案，最小 runner 已实现** | `scripts/output_mutation_runner.sh` 仅供显式 `--allow-output-mutation` 手工调用；fake Niri 回归见 `scripts/test_output_mutation_runner.sh`；真实 Niri 异常路径和跨 compositor 身份证明仍未完成。 |

## 实测记录

提案与规格引用的实机数据存放在 [`docs/measurements/`](../measurements/)，脚本在 [`scripts/`](../../scripts/)。

| 记录 | 结论 |
|---|---|
| [L9 分数缩放与旋转精度](../measurements/l9-fractional-scaling.md) | 旋转零误差；**整数缩放完美、分数缩放系统性退化**，成因是逻辑→物理取整 |
