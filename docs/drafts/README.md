# 草案目录

草案是含有简单实现细节和限定条件的**可行想法**：它说明方向可以怎样落地，但尚未完整限定最终程序逻辑，不能替代方案，也不能直接授权生产实现。流程仍为：

```text
提案 → 草案 → 方案
```

## 当前草案

| 草案 | 状态 | 下一步 |
|---|---|---|
| [P5 · output mutation/recovery](P5-output-mutation-recovery.md) | ✅ **已转方案** | 方案已并入 [`docs/spec.md`](../spec.md) §8.7；草案仍作为接口、伪代码和测试矩阵的历史依据，真实 mutation 实现尚未授权。 |
| [核心库完全体收口与边界硬化](complete-state-hardening.md) | ✅ **已转方案** | 自审裁枝与方案可执行性评估已完成，约束已并入 [`docs/spec.md §8.8`](../spec.md)；实现仍须按工作包提交证据并经过全量审查。 |
| [无显示核心边界收口](no-display-boundary-closure.md) | ✅ **已转方案** | 经人工复核后纳入 `docs/spec.md §8.8`；本文保留 Wayland/X11 fake、历史归档标注、离线发布元数据和兼容 API 的细化实现细节，真实环境证据、正式发布和 push 明确排除。 |
