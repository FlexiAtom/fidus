# 草案目录

草案阶段回答“怎么做”，但尚未成为规范，也不能直接授权生产实现。流程仍为：

```text
提案 → 草案 → 方案
```

## 当前草案

| 草案 | 状态 | 下一步 |
|---|---|---|
| [P5 · output mutation/recovery](P5-output-mutation-recovery.md) | ✅ **已转方案** | 方案已并入 [`docs/spec.md`](../spec.md) §8.7；草案仍作为接口、伪代码和测试矩阵的历史依据，真实 mutation 实现尚未授权。 |
| [核心库完全体收口与边界硬化](complete-state-hardening.md) | 📝 **草案待全审** | WP-A–G 定义 Fused、输入边界、Anchor、Wayland 生命周期、P5 runner、协议与发布收口；全审通过并人工批准方案前不得实现新增生产行为。 |
