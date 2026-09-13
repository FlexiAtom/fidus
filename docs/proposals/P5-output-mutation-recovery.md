# 提案：宿主 output mutation 与恢复状态机

> **状态：✅ 提案已获批准，已转草案阶段（2026-09）；草案见 [`docs/drafts/P5-output-mutation-recovery.md`](../drafts/P5-output-mutation-recovery.md)。**
>
> 本提案只讨论测试宿主 runner 如何临时修改并恢复输出配置；不改变 fidus 的坐标真值边界，不把 compositor 状态当作定位测量。

## 1. 要解决的问题

当前 `scripts/l9_precision_matrix.sh` 与 `scripts/l9_scale_stats.sh` 会直接调用：

```text
niri msg output eDP-1 scale <value>
niri msg output eDP-1 transform <value>
```

它们存在明确风险：

- output 名称硬编码为 `eDP-1`；
- 恢复值硬编码为 `scale=1`、`transform=normal`；
- 没有修改前的原值快照；
- 没有逐字段 read-back；
- 没有并发锁；
- `SIGKILL`、宿主崩溃、compositor 重启或命令半成功时，无法证明桌面状态。

目标不是让 fidus 获得 compositor 真值，而是让**调用方明确授权的测试控制**具备最小可审计的保存、修改、验证和恢复边界。

## 2. 已验证的核心假设

### 2.1 Niri 提供临时 output 控制

可复现的只读探针为 [`scripts/p5_niri_output_probe.sh`](../../scripts/p5_niri_output_probe.sh)。它只检查帮助文本和 `niri msg outputs`，不会调用任何修改命令；它证明的是“控制入口存在、状态字段可读”，不证明 apply/restore 已经安全实现。

当前机器实测：

```text
niri msg output --help
```

支持：

```text
scale
transform
mode
position
vrr
on/off
```

帮助文本明确说明 output 配置是临时修改，不写入配置文件。这满足“测试控制而非永久配置”的必要条件，但不证明命令成功后状态一定已经应用。

### 2.2 Niri 可读回 output 状态

当前机器实测：

```text
niri msg outputs
```

返回：

```text
Output "Chimei Innolux Corporation 0x1493 Unknown" (eDP-1)
  Logical size: 1366x768
  Scale: 1
  Transform: normal
```

因此可以把 `niri msg outputs` 作为**宿主控制状态的读回来源**，但不能把其中的 logical position 或其它字段当成 fidus 坐标真值。

### 2.3 没有 JSON 输出接口

实测：

```text
niri msg outputs --json
```

返回 unexpected argument。当前方案若只支持 Niri，必须解析受控文本；解析失败必须返回 `EnvironmentUnavailable` / `RecoveryUnverified`，不能猜测默认值。

### 2.4 当前 output 身份

本机当前观察到：

```text
output_name=eDP-1
scale=1
transform=normal
```

这只是本次宿主观察结果，不应写成跨机器保证。output 名称可能因硬件、热插拔或 compositor 配置改变。

只读探针连续读取 5 次 `niri msg outputs`，完整输出 SHA-256 均为 `19ed1a93f3bb4068092e3500f5d3ad3ba202d4bdf85c2db14c9a69eea51ee2f3`；这只证明本机短时间读回稳定，不证明热插拔、compositor 重启或跨版本格式稳定。

## 3. 提案边界

### 3.1 唯一 owner

只有宿主 runner 可以：

- 读取 output 控制状态；
- 修改 scale/transform；
- 获取和释放 mutation lock；
- 恢复原值；
- 判断恢复是否已被 read-back 证明。

`fidus-test` 负责协议解析和报告；容器只执行测试 binary，不获得 `niri msg`，不拥有恢复权。

### 3.2 明确授权

以下条件必须同时满足才允许修改：

```text
execution_mode=live-host
--allow-output-mutation
显式 output 名称
非空 scale/transform 白名单
```

`live-container` 永远拒绝 output mutation 请求。没有 `--allow-output-mutation` 时，测试只能使用当前状态，不能执行控制命令。

### 3.3 不承诺的情况

以下情况不能报告“已恢复”：

- `SIGKILL`；
- 宿主进程崩溃；
- compositor 重启；
- output 热插拔或身份变化；
- `niri msg outputs` 无法解析；
- restore 命令返回成功但 read-back 不等于原值；
- 同一 output 在 run 中消失或更换身份。

统一报告：

```text
RecoveryUnverified
```

## 4. 状态机

建议实现以下不可跳跃状态：

```text
Idle
  -> LockAcquired
  -> ReadOriginal
  -> AppliedAndReadBack
  -> Running
  -> RestoreRequested
  -> RestoredAndReadBack
  -> Finished
```

任何异常都必须进入 `RestoreRequested`，除非尚未到达 `ReadOriginal`；未取得原值时不能伪造恢复结果。

### 4.1 `ReadOriginal`

保存最小必要快照：

```text
output_name
output_identity_fields
scale
transform
observed_at
```

`output_name` 只能作为命令选择器，不能单独作为恢复身份。只有 Niri 输出中存在可证明稳定的 connector/serial 等字段时，才能将其纳入 `output_identity_fields` 并允许恢复确认；不能把显示名称、EDID 描述或 logical position 猜作稳定身份。当前实测输出只暴露 `eDP-1` 和显示描述，没有可确认的稳定 connector/serial，因此当前 Niri 路径即使 scale/transform 恢复相等，也只能报告 `RecoveryUnverified`，不能报告 `recovery=confirmed`。

### 4.2 `AppliedAndReadBack`

每次设置都必须：

1. 使用参数数组调用命令；
2. 检查命令退出码；
3. 重新读取 `niri msg outputs`；
4. 解析目标 output；
5. 逐字段确认 scale/transform 等于请求值；
6. 任一条件失败即停止运行并进入恢复。

成功退出码本身不是应用成功证据。

### 4.3 `RestoredAndReadBack`

恢复必须使用快照中的原值，不能使用 `1` / `normal` 等默认值。恢复后再次读取并逐字段比较：

```text
actual.output_identity == original.output_identity
actual.scale == original.scale
actual.transform == original.transform
```

只有全部相等，且 `output_identity_fields` 本身具有稳定身份证明，才允许报告 `recovery=confirmed`。否则必须报告 `RecoveryUnverified`，并覆盖原始校准成功/失败结果。当前 Niri 只有名称选择器时，即使 scale/transform 相等，也必须报告 `recovery=unverified`。

## 5. 并发与锁

宿主 runner 使用固定的用户态 lock 文件或等价锁机制，锁的范围覆盖：

```text
ReadOriginal → AppliedAndReadBack → Running → RestoredAndReadBack
```

获取不到锁必须拒绝本次 mutation，不得等待后偷偷覆盖其他 runner 的快照。锁释放必须使用异常安全路径；但不能声称 `SIGKILL` 后 trap 一定执行。

锁中应记录：

```text
pid
run_id
output_name
acquired_at
```

过期锁不能自动删除并继续修改，除非调用方明确执行人工清理并承担恢复风险。

## 6. 协议映射

P4 的 `lifecycle` 记录已经有 `recovery` 字段，但当前 v1 只冻结字段存在和 `status` 集合，并未冻结 `recovery=confirmed` 等值集合。本提案不直接扩展 v1；进入草案时必须先决定并测试 recovery 值集合，或新增协议版本。示意值如下，当前 parser 不应被视为已承诺支持：

```text
FIDUS_RESULT version=1 kind=lifecycle run_id=r01 status=ok execution_mode=live-host teardown=confirmed recovery=confirmed
```

恢复未被证明时：

```text
FIDUS_RESULT version=1 kind=lifecycle run_id=r01 status=unverified execution_mode=live-host teardown=confirmed recovery=unverified
```

未来协议决策必须保证：`summary` 反映恢复失败优先级；不能因为 calibration 已成功就报告整 run 成功。

## 7. 配置白名单

第一版只允许：

```text
scale ∈ {1, 1.25, 1.5, 1.75, 2}
transform ∈ {normal, 90, 180, 270}
```

实际可用值必须由 backend/compositor 验证；白名单只是防止参数注入和明显危险输入，不是能力证明。

以下输入拒绝：

- 空 output 名称；
- 多 output 模糊匹配；
- NaN / infinity / 负数 / 0 scale；
- 未知 transform；
- 含空格或 shell 元字符的未转义命令片段；
- 容器 mutation；
- 没有调用方显式授权。

## 8. 失败模式审查

### 8.1 文本格式变化

**收益**：不绑定 Niri 私有 IPC，只利用已有宿主控制命令和状态输出。

**失效**：Niri 改变 `niri msg outputs` 的文本格式，解析器可能漏字段或误读。

**处理**：解析失败即 `EnvironmentUnavailable` / `RecoveryUnverified`；不能回退到上次值或默认值。应为每个支持的 Niri 版本保存 fixture，并把未知格式当失败。

### 8.2 output 热插拔

**收益**：身份比较能防止把另一块屏幕误当原 output 恢复。

**失效**：目标 output 消失后同名 output 重新出现。

**处理**：只比较稳定身份字段；若只有名称可用，名称重现不能证明身份未变，恢复状态应为 `RecoveryUnverified`。

### 8.3 命令成功但 compositor 尚未应用

**收益**：apply/read-back 阻止测试在错误配置上继续运行。

**失效**：命令返回成功，但状态异步更新尚未完成。

**处理**：短暂轮询并设定超时；超时即恢复/未验证。轮询不是无限等待，也不能把最后一次旧值当新值。

### 8.4 恢复命令半成功

**收益**：逐字段 read-back 防止只恢复 scale、未恢复 transform 的假成功。

**失效**：第一个恢复命令成功，第二个失败，或 compositor 在两次命令之间重启。

**处理**：继续尝试所有可恢复字段，最后统一 read-back；任一字段不等于原值即 `RecoveryUnverified`。

### 8.5 用户同时操作桌面

**收益**：锁只保护测试 runner，不阻止用户正常使用电脑。

**失效**：用户或其它工具同时改变同一 output。

**处理**：restore 前重新确认 output 身份；若当前配置已被外部改变，必须记录冲突并拒绝覆盖，或由调用方明确选择强制恢复。强制恢复不能成为默认路径。

## 9. 必须先做的实验

提案批准前，不实现正式 mutation runner；只允许用可控 fake compositor adapter 验证状态机：

1. 正常 apply → run → restore；
2. apply 命令失败；
3. apply read-back 不匹配；
4. child exit 1；
5. child exit 2；
6. child exit 3；
7. SIGINT；
8. timeout；
9. restore 第一字段失败；
10. restore read-back 不匹配；
11. 并发 runner 抢锁；
12. output identity 改变；
13. `SIGKILL` 只产生 `RecoveryUnverified`，不声称恢复成功。

此外需要一次用户正在使用电脑的 Niri 实机实验：

- 终端滚动；
- 鼠标移动；
- 其它窗口刷新；
- scale/transform 修改后观察桌面；
- 正常退出、Ctrl-C、timeout；
- 每次都读取原值并确认恢复。

实验输出只能证明本机 Niri 行为，不能扩展成跨 compositor 保证。

## 10. 实际 mutation 实验补充

用户明确授权后，在当前活动 Niri 桌面完成了有限正常路径实验，记录见 [`docs/measurements/p5-output-mutation-recovery.md`](../measurements/p5-output-mutation-recovery.md)：scale `1.25` apply/read-back/restore 成功；transform `90` 的 canonical read-back 为 `90° counter-clockwise`，按 canonical enum 重试后恢复 `normal` 成功。第一次字面比较误报 mismatch，说明 parser 必须 canonicalize，不能将输入 token 与显示文本直接比较。

本实验只证明当前机器正常宿主存活路径的字段读回和恢复，不证明稳定身份、异常退出、并发或跨 compositor 恢复；依据 NameOnly 边界，不能报告 `recovery=confirmed`。

## 11. 方案判断

当前建议：**接受提案方向，但暂不进入草案。**

理由：

- Niri 临时控制命令和状态读回的核心假设已实测成立；
- 没有 JSON 输出，文本解析和版本 fixture 是新增风险；
- 现有脚本的硬编码恢复值不能直接升级为生产实现；
- 必须先用 fake adapter 验证状态机和失败优先级；
- mutation 会改变用户正在使用的电脑，真实实验必须默认用户有活动；
- `SIGKILL`、宿主崩溃和 compositor 重启仍不可证明恢复。

提案批准后，下一步应是：

```text
fake adapter 状态机实验
→ 快审
→ 草案
→ 全审
→ 方案
→ 宿主 runner 实现
```
