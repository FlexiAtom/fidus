# 提案：P5 人工 output 恢复工具

> 历史阶段记录：最初为提案；现已自审通过并转入 `docs/spec.md §8.7.4` 方案，且已实现。
>
> 本提案只处理 `output_mutation_runner.sh` 意外退出、`recovery=unverified` 或宿主会话崩溃后的**人工补救**。它不证明自动恢复可靠，不重新打开已冻结的 P5 真实桌面错误处理单点验证。

## 1. 核心问题与收益对象

当前 runner 已会在正常可达路径尝试恢复，但 `SIGKILL`、宿主崩溃、compositor 重启或恢复读回失败时，操作者只有文档中的手工命令，没有一个带 selector 唯一性检查、原值校验、读回确认和审计日志的固定入口。

收益对象是：在已明确授权 output mutation 的操作者，降低恢复失败后误恢复错误 output、猜测默认值或遗漏第二个字段的风险。

本工具不服务于：

- 定位窗口或生成 fidus 坐标；
- 自动 watcher 或崩溃后自动恢复；
- 证明稳定硬件 identity；
- 绕过 `recovery=unverified`；
- 默认 CI、live-container 或生产路径。

## 2. 方案假设的提案阶段核验

已有本机证据表明：

- `niri msg output <name> scale <value>` 与 `transform <value>` 是可执行控制入口；
- `niri msg outputs` 能读回 scale/transform；
- transform 需要把 `90° counter-clockwise` 等文本 canonicalize 为 `90`；
- 当前 Niri 只有 NameOnly selector，字段读回一致仍只能是 `recovery=unverified`；
- 未连接 output 可能返回 rc=0，因此必须解析 output 列表，不能只看退出码。

这些证据支持一个**人工补救入口**，但不支持自动恢复、稳定 identity 或跨机器保证。

## 3. 拟议接口

新增 `scripts/manual_restore_output.sh`，必须显式提供：

```text
--allow-output-mutation
--output NAME
--scale ORIGINAL_SCALE
--transform ORIGINAL_TRANSFORM
```

可选：

```text
--log PATH
--niri-bin PATH（通过 FIDUS_NIRI_BIN 提供）
```

工具必须：

1. 在执行任何 setter 前验证授权、参数 allowlist 和 log 路径；
2. 解析 `niri msg outputs`，确认 selector 唯一且 connected；
3. 记录恢复前状态；
4. 分别设置 scale 与 transform，即使第一个 setter 失败也继续尝试第二个；
5. 再次解析并逐字段比较原值；
6. 把命令结果、读回状态和最终结论写入屏幕外日志（默认 stderr，同时允许 `--log`）；
7. 读回失败或不一致时返回非零，并报告 `recovery=unverified`；
8. 不将手动恢复结果报告为 `recovery=confirmed`。

建议退出码：

```text
0  两字段均已读回原值（仍只是人工恢复成功，不是 confirmed）
2  环境不可用、selector 不唯一或读回失败
3  未授权或参数错误
4  setter 部分失败、字段不一致或恢复结果无法证明
```

## 4. 失败模式与边界

- **错误 selector**：若只按名称盲写，可能改变另一个 output；因此必须先确认唯一 connected output，失败即停止，不发送 setter。
- **原值填错**：工具无法知道操作者日志是否可信；只接受 allowlist，不能猜测 `1`/`normal`。
- **第一个 setter 部分成功**：第二个 setter仍必须尝试；最终逐字段读回并返回保守结果。
- **compositor 不可读**：不能把命令退出码当恢复成功；返回 `unverified`，保留人工处置。
- **恢复期间外部改变**：人工工具的目标是执行记录中的原值，无法证明变更来源；日志必须记录 before/after，调用方不得据此宣称自动恢复可靠。
- **SIGKILL/崩溃**：工具不承担自动接管；操作者须从 mutation 前屏外日志取得原值。
- **日志路径被覆盖或不可写**：不得静默丢失审计记录；至少写 stderr，若显式日志不可写则失败。

## 5. 验收与回滚

无显示 fake adapter 测试必须覆盖：

- 未授权拒绝且不调用 fake setter；
- selector 缺失、重复、deferred 拒绝且不调用 setter；
- 正常双字段恢复和 canonical transform；
- 第一个 setter 失败仍尝试第二个；
- 读回不一致返回非零与 `unverified`；
- 日志包含 before/after 和最终状态；
- malformed output 不被当作 connected output。

实现只新增脚本、测试和文档；回滚为删除新增脚本/测试并撤销方案段落，不触碰生产 crate、默认 CI mutation 权限或既有 runner 状态机。

## 6. 自审裁枝（提案阶段）

保留：

- selector 唯一性检查：直接防止恢复错误 output；
- 两字段都尝试并逐字段 read-back：直接防止半恢复被误报；
- 显式授权和 allowlist：保持 P5 mutation 边界；
- 屏外日志：应对 runner 崩溃后仍需人工恢复；
- fake negative/positive 回归：无需真实桌面即可验证脚本语义。

删除：

- 自动 watcher：会扩大权限和崩溃恢复责任，超出人工恢复目标；
- 稳定硬件 identity：当前 Niri 不提供，加入工具只会制造虚假完成度；
- TTY 自动检测：TTY 是观察/控制通道，不是隔离，也不能证明恢复；
- 生产 Rust API：P5 host tool 不应进入 fidus 坐标库；
- 默认 CI 集成：人工工具必须显式调用，默认路径不得 mutation。

## 7. 结论

历史阶段结论：自审通过并转入 `docs/spec.md §8.7.4` 方案；当前实现与 fake 回归状态以方案和项目状态记录为准。

通过理由：问题真实存在，解决对象明确，已有本机只读/正常路径证据支持接口假设；范围只提供人工补救，不把人工结果升级为自动 recovery 证据，也不重新打开已冻结的真实异常验证。
