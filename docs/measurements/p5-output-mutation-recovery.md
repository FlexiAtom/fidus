# P5 output mutation/recovery 实测记录

日期：2026-09（当前会话）

> 本记录只证明当前机器、当前 Niri 会话的行为，不是跨机器或跨 compositor 保证。实验在用户明确授权后执行；没有实现正式 mutation runner。

## 环境

```text
compositor=niri
output=eDP-1
original_scale=1
original_transform=normal
```

原值通过 `niri msg outputs` 读取。当前输出文本提供 output 名称、scale 和 transform，但没有可确认的稳定 connector/serial 身份字段，因此身份证据仍为 `NameOnly`。

## 实验 A：scale

执行流程：

```text
读取原值
→ niri msg output eDP-1 scale 1.25
→ 等待 1 秒
→ niri msg outputs
→ 恢复 scale 1
→ 等待 1 秒
→ niri msg outputs
```

结果：

```text
apply requested=1.25 readback=1.25
restore expected=1 readback=1
```

结论：当前 Niri 上 scale 命令和 scale 文本读回成立；这不证明异常退出、热插拔或身份恢复成立。

## 实验 B：transform

第一轮实验使用输入 token `90` 与读回文本直接比较，读回为：

```text
90° counter-clockwise
```

因此第一轮被脚本判为 mismatch，但不是 compositor apply 失败；EXIT trap 随后执行 `transform normal`，最终读回：

```text
scale=1
transform=normal
```

第二轮按 canonical 文本处理：

```text
apply requested=90 canonical_readback=90° counter-clockwise
restore expected=normal readback=normal
```

结论：transform 必须解析为 canonical enum 后比较，不能比较用户输入 token 与显示文本的字面字符串。成功退出码不能替代 read-back；parser/runner 若漏掉该映射会误报环境失败。

## 恢复证据边界

本次两组字段最终均读回原值：

```text
fields_restored scale=1 transform=normal
```

但 output 身份只有 `eDP-1` 名称选择器，没有稳定身份字段。因此依据 P5 草案：

```text
identity_evidence=name-only
recovery=unverified
```

不能报告 `recovery=confirmed`。

本次只覆盖正常宿主存活路径。没有覆盖：

- SIGKILL；
- 宿主崩溃；
- compositor 重启；
- output 热插拔；
- 并发 runner；
- child timeout/SIGINT supervisor；
- 用户在恢复前外部修改 output。

## 实验 C：无显示服务器 fake 异常路径

使用临时 bash fake adapter（未写入项目、未调用 Niri）验证状态和调用序列：

```text
read-original-fail       PASS
apply-transform-fail     PASS
child-timeout/process-group PASS
restore-failure-priority PASS
flock contention/reacquire PASS
recovery dominates child exit 0 PASS
```

结果：

```text
fake_experiment_pass=6 fail=0
```

本实验修正了第一版实验脚本的 shell function 作用域错误后重跑；第一版的 4 个 FAIL 是 harness 自身找不到 `fake_run`，不计入状态机证据。fake 结果只证明预定模型中的调用顺序和优先级，不证明正式 runner 已实现，也不证明 Niri 异常路径。

## 实验 D：锁与进程组异常路径

不触碰 compositor 的实际实验结果：

```text
flock contention/reacquire        PASS
symlink path detectable           PASS
PID metadata not ownership proof  PASS
kernel lock release               PASS
```

进程组实验先暴露了一个重要实现细节：后台 launcher 的 PID 不能凭约定当作 PGID；必须读取 supervisor 实际 PGID。向 PGID 发送 TERM 后，故意忽略 TERM 的 child 仍存活；随后向同一 PGID 发送 KILL，child 终止（`child_alive_after_term=1`, `child_final_state=gone`）。这验证了：

```text
TERM whole process group
→ bounded wait
→ KILL the actual process group
→ reap/wait
```

不能只 TERM supervisor，也不能用 launcher PID 猜测进程组。该实验没有修改桌面或 compositor。

## 实验 E：协议 parser

使用现有 `target/debug/fidus-test parse` 实际验证 v1 recovery：

```text
recovery=not_requested  rc=0 PASS
recovery=confirmed      rc=0 PASS
recovery=unverified     rc=0 PASS
recovery=maybe          rc=3 PASS (rejected)
```

结果：

```text
protocol_experiment_pass=4 fail=0
```

这证明当前 parser 执行了已冻结的 recovery 三值；不证明 production producer 已接入 mutation 生命周期。

## 实验 F：parser 恶意/截断边界

使用现有 `target/debug/fidus-test parse` 验证以下输入均 fail-closed，退出码均为 3：

```text
duplicate key             PASS
missing required field    PASS
unknown version           PASS
truncated summary         PASS
run_id mismatch           PASS
inconsistent summary     PASS
control character value  PASS
```

结果：

```text
parser_boundary_pass=7 fail=0
```

这是协议 parser 的实际回归，不等同于 Niri `outputs` 文本 grammar fixture；后者仍需在方案阶段补齐。

## 实验 G：Niri parser 输出形状

对当前 `niri msg outputs` 实际输出检查：

```text
unique output header       PASS
unique Scale field         PASS
unique Transform field     PASS
scale decimal              PASS
transform canonical        PASS
scale singleton             PASS
transform singleton         PASS
```

结果：

```text
niri_parser_shape_pass=7 fail=0 scale=1 transform=normal
```

当前会话只有一个 output，因此无法用真实多屏环境证明 selector 歧义处理。使用当前输出格式的临时合成文本验证：

```text
duplicate header rejected  PASS
missing transform detected PASS
exact single header        PASS
```

结果：

```text
niri_parser_synthetic_pass=3 fail=0
```

两轮合成实验第一次的 3 个 FAIL 来自断言函数错误地把测试名当命令执行；修正 harness 后重跑 3/3 通过，不计第一轮为 parser 证据。合成文本不证明 Niri 实时行为。`fidus-test` 现已加入版本化 `NIRI_OUTPUTS_V25_FIXTURE` 和未连接 deferred fixture，随后在可写 Cargo 缓存中通过 27/27 测试、clippy 和 doc；fixture 测试仍不等同于真实 Niri parser adapter。

## 实验 H：信号重入与恢复模型

使用临时 bash supervisor 模型（未写入项目、未调用 Niri）验证：

```text
signal re-entry during recovery       PASS
child spawn failure still restores    PASS
partial restore attempts both fields  PASS
partial restore preserves failure     PASS
stale metadata cannot bypass lock     PASS
```

结果：

```text
signal_experiment_pass=5 fail=0
```

该模型只验证草案规定的事件顺序和优先级，不证明正式 supervisor 已实现。恢复阶段重复信号只记录 `interrupted-during-recovery`，不启动第二次恢复；child 已启动前只要 snapshot 存在，spawn 失败也必须恢复；第一个 restore 字段失败时仍尝试第二个字段。

## 实验 I：Niri 未连接 output 的零退出码

对不存在的 output 执行 scale 控制：

```text
niri msg output __fidus_nonexistent__ scale 1.25
rc=0
stdout: Output "__fidus_nonexistent__" is not connected.
       The change will apply when it is connected.
```

当前真实 output 状态仍为：

```text
scale=1
transform=normal
```

非法 scale `invalid` 返回 `rc=2`，但不存在 output 的命令返回 0 是更危险的边界：它表示延迟配置，不表示当前 output 已应用。结论：runner 必须检查 output 状态文本和 apply read-back，不能只信命令退出码；否则可能把未连接 selector 当作 mutation 成功。

本实验没有改变当前连接 output 的显示状态。

## 复现入口

只读能力探针：

```bash
scripts/p5_niri_output_probe.sh
```

该探针不修改 output。实际 mutation 实验需要用户明确授权，不能把本记录脚本化为默认行为。

`fidus-test` 已加入 P5 纯 Rust fake/fixture 测试；`scripts/test_output_mutation_runner.sh` 已加入 fake Niri 集成回归，覆盖未授权、NameOnly recovery=4、child failure recovery 优先、restore、symlink lock、flock 竞争、malformed/deferred parser；测试通过。完整 workspace test、clippy、doc、shell 语法和 diff 检查也通过。

## 当前挂起事项

1. **跨机器发布验证：挂起**。当前记录只证明本机产物、镜像和当前环境，不能宣称跨机器复现。
2. **真实桌面错误处理单点验证：挂起**。用户要求先挂起该项，因此 fake Niri/无显示回归不能替代真实 compositor 异常路径证据。

两项均不是通过或失败，恢复前不得从 pending 清单中删除。
