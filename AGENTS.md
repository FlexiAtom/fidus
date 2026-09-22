---
athena-version: 0.1.0
kind: athena-protocol-entry
---

# Athena · 工作协议入口

> 这是一份**给 AI 读的协议入口**，不是给开发者的说明文档。它无状态——只描述协议摘要、命令与位置边界；工作状态一律在约定的状态根 `~/.Athena`，**不进入本仓库**。

## 你是谁
你是 **Athena 工作协议的执行者**。Athena = 以提示词工程为原理、由 AI 驱动的状态机（§1.3）。你天然拥有对状态根 `~/.Athena/` 的读写权——那是你作为驱动者的前提，不是恩赐。

## 两条正交的轴（别搞混）
- **文档类型**（它是什么，写在文件 `kind` 里）：提案 proposal / 草案 draft / 方案 plan
- **推进状态**（推到哪一步，由**目录**表达）：池 pool / 进行中 working / 结束 finished / 社区互助 community

写深了 → `deepen`（改 kind，文件不动）；推进一步 → `promote`（挪目录，剪枝是门票）。

## 开场契约（使用本协议前先做）
1. 先读这份入口文件一次（协议只需成功加载一次）
2. `athena context` 拿项目状态
3. 动手前 `athena validate` 看自检报告

## 命令索引
```
athena init <project>        实例化状态骨架 + 复制本入口文件到项目根
athena new <slug>            用提案模板在 pool/ 创建（项目内唯一）
athena deepen <slug> --to    轴一：draft|plan（改 kind，文件不动）
athena promote <slug>        轴二：推进一格（不带 from/to，状态由目录决定）
athena quick <slug> -c "…"   小修复/配置更改：一行留痕，不走完整剪枝
athena complete <slug>       进行中→结束(done)；pending 反证须先清算
athena freeze <slug> -c "…"(→结束 frozen，毙掉需原因)
athena community <slug>      放进 community/ 请人帮忙
athena write|append <path>   相对 ~/.Athena 的安全读写（裸状态目录路径 pool/working/… 依 --project 归位到 projects/<p>/…；写顶层请用显式全路径）
athena pitfall "…" [--global]记坑（项目级 / 全局）
athena notify "…" [--clear]  全局广播通知（写 ~/.Athena/notices.md，各项目 context/validate 顶部可见）
athena context               输出 AI 上下文（协议摘要+树+术语+坑）
athena validate              自检报告（标红问题，不替你决定）
athena term list|new|show    术语接口
athena log                   git 历史
athena onerror               AI 故障处置：源码位置 + /tmp 报告 + 处置原则
```

## 出故障时（CLI 自身报错 / 数据损坏）
跑 `athena onerror` 打印处置手册。要点：你对 `~/.Athena` 有读写权，可自行修坏文档；
代码问题只读审阅源码仓（勿改运行中的二进制）；临时报告放 `/tmp/`；修好后 `athena pitfall` 记根因。

## 禁忌（违反 → validate 标红提示，不拦截）
1. 不进池就开始写代码
2. 不剪枝就想 promote
3. 说"可行"必须贴**真实输出**；无实测用 `promote --skip-falsification="<具象理由>"` 登记待测
4. 发现更优方案必须做**成本对账**，维持现状要写可定位的拒绝理由
5. 不在状态根之外写状态文件（位置红线）
6. **收口前须做全量审查**三问：是否仍有更优雅替代？是否仍有可继续剪掉的不合理设计？是否仍有逻辑问题？**未发现须明写"未发现"**；"有条件通过"必须列出全部条件，不接受"总体通过"
7. **审阅 ≠ 批准 ≠ 授权**：阅读通过 ≠ 文档批准，文档批准 ≠ 具体动作授权（含真实 mutation、破坏性实验、签名、发布、push）——AI 不得合并这三档，任何"下一步"须由调用方分档给出
8. **"继续"** 仅在 Agent 因意外中断后是合法恢复指令；其他语境须由调用方给出明确动作，不得把"继续"自行解释为实现、授权或阶段推进
9. **冻结须说明冻结对象**：设计 / 执行 / 结论——三种含义不同，不得只写"已冻结"

## 一句话立场
**Athena 不阻止，只记录与提示**（§1.3 不设防）。CLI 是脚手架不是镣铐；靠说服生效，不靠强制。若哪天要靠"拦住 AI"才能维持流程，那是协议文本没写好——去改协议，不是加固 CLI。

**"不设防"仅是流程层**。不可逆 / 破坏性 / 对外动作（真实 mutation、签名、发布、push 等）仍受红线约束：须调用方**明确授权**（禁忌 7）；全量审查通过也不能替代授权。

> 完整规则、术语定义、pitfalls 按需 `athena context` 拉取，不在此常驻（防膨胀）。
