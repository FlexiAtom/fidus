# 草案：宿主 output mutation 与恢复状态机

> **历史阶段记录：草案全审有条件通过（2026-09）。**
> 当前状态：方案已并入 [`docs/spec.md §8.7`](../spec.md)，最小显式 runner 已实现；真实 compositor 异常路径和跨机器验证仍挂起。
>
> 对应提案：[`docs/proposals/P5-output-mutation-recovery.md`](../proposals/P5-output-mutation-recovery.md)。
> 本草案只确定接口、状态机、失败语义和测试边界；真实 compositor runner 必须在草案全审通过后实现。

## 1. 范围与非目标

### 1.1 范围

为 `live-host` 测试提供调用方显式授权的临时 output 配置控制：

```text
读取当前 output 状态
→ 保存原值
→ 修改 scale/transform
→ read-back 验证
→ 运行测试
→ 用原值恢复
→ read-back 验证恢复
```

### 1.2 非目标

本草案不：

- 把 compositor 状态当作 fidus 坐标真值；
- 把 output 几何送入概率池；
- 给 `fidus` 生产 crate 增加 compositor 私有 IPC；
- 让 `fidus-test` 复制 backend 或校准器；
- 让 live-container 获得 mutation/recovery 权限；
- 声称 SIGKILL、宿主崩溃或 compositor 重启后一定恢复；
- 在未保存原值时恢复到 `scale=1` / `transform=normal`。

## 2. 职责与接口

### 2.1 owner

| 组件 | 责任 | 禁止 |
|---|---|---|
| host runner | compositor 命令、快照、锁、恢复、read-back | 解析 fidus 坐标真值 |
| `fidus-test` | 协议、报告、子进程结果 | 调用 `niri msg`、恢复输出 |
| live-container | 承载测试 binary、回传结果 | mutation、恢复、compositor 控制 |
| production backend | 投射、截屏 | 查询窗口/输出几何作为真值 |

### 2.2 HostOutputAdapter

草案接口：

```text
trait HostOutputAdapter {
    fn list_outputs(&mut self) -> Result<Vec<ObservedOutput>, ReadError>;
    fn read_output(&mut self, selector: &OutputSelector)
        -> Result<ObservedOutput, ReadError>;
    fn set_scale(&mut self, selector: &OutputSelector, scale: Scale)
        -> Result<(), CommandError>;
    fn set_transform(&mut self, selector: &OutputSelector, transform: Transform)
        -> Result<(), CommandError>;
}
```

`HostOutputAdapter` 是宿主测试层接口，不进入 `fidus` 生产 facade。

`ObservedOutput`：

```text
ObservedOutput {
    selector_name: OutputName,
    identity: IdentityEvidence,
    scale: CanonicalScale,
    transform: AllowedTransform,
    observed_at_wall: DateTime,
    observed_at_monotonic: Instant,
}
```

`Stable(fields)` 只能由 adapter 的版本化、受控状态源提供，并在原值、apply read-back、restore read-back 中逐次比较；fixture 只能验证 parser，不能把字段升级成真实稳定身份。
```

`OutputName` 只能作为命令 selector。selector 必须是显式、精确、无控制字符的 UTF-8 名称；协议中的 output 字段若不能安全表示，应脱敏或报告 `not-reported`，不能把未编码名称拼进 shell 命令。`IdentityEvidence` 必须区分：

```text
Stable(fields)
NameOnly(name)
Unknown
```

当前 Niri 文本输出只有名称选择器和显示描述，草案默认视为 `NameOnly`；因此不能凭名称确认恢复身份。

### 2.3 MutationConfig

```text
MutationConfig {
    execution_mode: LiveHost,
    allow_output_mutation: bool,
    output: ExplicitOutputName,
    scales: NonEmpty<Vec<AllowedScale>>,
    transforms: NonEmpty<Vec<AllowedTransform>>,
    command_timeout: Duration,
    readback_timeout: Duration,
    lock_path: CanonicalSessionLockPath,
}
```

构造时拒绝：

- `execution_mode != live-host`；
- `allow_output_mutation == false` 但存在变更请求；
- 空 output 名称；
- 多个 output 匹配 selector；
- 空 scales/transforms；
- 0、负数、NaN、Infinity scale；
- 未知 transform；
- timeout 为 0；
- lock path 不是宿主根据当前用户/compositor session 解析出的 canonical 路径；
- output selector 含控制字符、换行或无法按 Niri 参数安全传递的值。

第一版白名单：

```text
scale: 1, 1.25, 1.5, 1.75, 2
transform: normal, 90, 180, 270
```

白名单只防止危险输入，不证明 compositor 支持对应值。scale 在内部使用受限 decimal/rational 表示并规范化后比较（例如 `1.250` 与 `1.25` 等价）；禁止使用无界 epsilon 把异常值伪装为相等。transform 只接受 canonical enum。

## 3. Niri 文本 adapter

### 3.1 命令

Niri adapter 只允许固定参数数组：

```text
niri msg outputs
niri msg output <explicit-output-name> scale <validated-scale>
niri msg output <explicit-output-name> transform <validated-transform>
```

禁止 shell 字符串拼接、`eval` 和用户提供的任意命令片段。

### 3.2 parser

`niri msg outputs` parser 必须：

1. 按支持的 Niri 版本使用版本化 grammar；
2. 在 mutation 前确认 selector 当前已连接；Niri 对未连接 output 的控制命令可能返回 rc=0 并表示“连接后生效”，这不是当前 apply 成功；
3. 找到且只找到一个精确 output header，拒绝同名/多 header 歧义；
4. 精确读取该 output 的 `Scale:` 和 `Transform:`；
5. 拒绝字段重复、缺失、非法数值、控制字符、截断输出和模糊 selector；
6. 不读取 `Logical position` 作为 fidus 真值或稳定身份；
7. 未知格式返回错误，不回退旧值或默认值。

显示描述、Unicode 引号和 locale 变化必须由 fixture 明确覆盖；未被 grammar 接受的形式一律失败。

parser 应拥有按 Niri 版本保存的 fixture。fixture 只测试文本解释，不证明实时 compositor 行为。

### 3.3 read-back

当前实机记录见 [`docs/measurements/p5-output-mutation-recovery.md`](../measurements/p5-output-mutation-recovery.md)：scale `1.25` 可读回并恢复，transform 输入 `90` 的读回是 `90° counter-clockwise`。因此 transform 必须先 canonicalize 为 enum，再比较；不能做输入 token 与人类可读文本的字面比较。

`set_scale` / `set_transform` 返回成功后，adapter 不能直接报告已应用。runner 必须在 `readback_timeout` 内重复：

```text
read_output(selector)
→ 比较请求 scale/transform
→ 相等则 AppliedAndReadBack
→ 不等则短暂等待后重读
→ 超时则失败并进入恢复
```

最后一次旧状态不能被当作新状态。

## 4. 状态机

### 4.1 状态

```text
Idle
LockAcquired
ReadOriginal
AppliedAndReadBack
Running
RestoreRequested
RestoredAndReadBack
Finished
RecoveryUnverified
```

### 4.2 合法迁移

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

失败迁移（每条终止边都必须释放已取得的锁）：

```text
Idle -> Finished                    # validation/no-mutation result
LockAcquired -> Finished            # ReadOriginal failed; no recovery claim
ReadOriginal -> RestoreRequested    # snapshot exists but later setup failed
AppliedAndReadBack -> RestoreRequested
Running -> RestoreRequested
RestoreRequested -> RestoredAndReadBack
RestoreRequested -> RecoveryUnverified
RestoredAndReadBack -> Finished
```

`LockAcquired -> Finished` 只允许携带 `EnvironmentUnavailable` 或 `HarnessError`，不能携带 recovery success；`ReadOriginal` 之后的所有异常必须经过 `RestoreRequested`。锁释放失败不能被吞掉：若已经 `RecoveryUnverified`，保持该结果并追加 lock-cleanup diagnostic；否则升级为 `HarnessError`。

含义：

- 未取得原值：不得执行恢复，不得声称恢复成功；
- 已取得原值但 apply 失败：仍必须尝试恢复；
- 已进入运行：无论 child 结果如何都进入恢复；
- 恢复命令或 read-back 失败：最终 `RecoveryUnverified`；
- `RestoredAndReadBack` 表示字段读回相等，身份确认另行记录，NameOnly 不得变成 confirmed。

### 4.3 伪代码

```text
run(config):
    validate(config)
    if !config.allow_output_mutation:
        return run_without_mutation()       # must not acquire mutation lock or call set

    lock = acquire_lock(config.canonical_lock_path)
        or return HarnessError
    state = LockAcquired
    snapshot = None
    cases = []

    try:
        snapshot = adapter.read_output(config.output)
            or raise OriginalReadFailed
        state = ReadOriginal

        # Every case is restored before the next case starts. The snapshot is
        # immutable; a failed case cannot become the next case's baseline.
        for scale in config.scales:
            for transform in config.transforms:
                case = begin_case(scale, transform)
                try:
                    adapter.set_scale(config.output, scale)
                    adapter.set_transform(config.output, transform)
                    wait_until_readback(scale, transform)
                        or raise ApplyReadbackFailed
                    state = AppliedAndReadBack
                    state = Running
                    case.child = run_supervised_child()
                catch error:
                    case.error = classify(error)
                finally:
                    state = RestoreRequested
                    case.restore = best_effort_restore_and_read_back(snapshot)
                    # Try both restore fields even if the first set command fails.
                    cases.append(case)
                    if !case.restore.readback_equal:
                        state = RecoveryUnverified
                        raise RecoveryFailed
                    state = RestoredAndReadBack

        state = Finished
        return fold_cases(cases)
    catch error:
        if snapshot exists and state != RestoredAndReadBack:
            state = RestoreRequested
            emergency = best_effort_restore_and_read_back(snapshot)
            if !emergency.readback_equal:
                state = RecoveryUnverified
                return recovery_failure(cases, error)
            state = RestoredAndReadBack
        return classify_with_cases(cases, error)
    finally:
        release = release_lock(lock)
        if !release.proven:
            record_lock_cleanup_failure()
            upgrade_result_if_needed(HarnessError)
```

`best_effort_restore_and_read_back` 对 scale、transform 分别记录命令结果，即使第一字段失败也继续第二字段，最后统一 read-back；它还必须比较身份和字段。`run_supervised_child` 负责进程组、超时和信号处理，见 §7。

实际实现不得依赖 `finally` 覆盖 SIGKILL；SIGKILL 的状态必须按不可验证处理。

## 5. 快照与身份策略

### 5.1 快照内容

```text
Snapshot {
    selector_name,
    identity_evidence,
    original_scale,
    original_transform,
    observed_at,
}
```

禁止把以下内容当快照：

- 硬编码 scale=1；
- 硬编码 transform=normal；
- 之前 run 的缓存；
- 仅有 logical position 的记录；
- 没有成功 read-back 的猜测值。

### 5.2 当前 Niri 限制

当前 Niri 只有 name selector，无法提供足够稳定的 connector/serial 证据。草案因此规定：

```text
当前 Niri：可以选择、可以读取、可以尝试恢复；不能证明硬件身份未变。
```

所以当前 Niri 路径恢复后最多报告：

```text
recovery=unverified
```

只有未来 adapter 获得稳定身份字段，并通过 fixture 与实机验证，才可启用 `recovery=confirmed`。

## 6. 锁

锁必须在原值读取前获取，并持续到最终 read-back 完成：

```text
LockAcquired
→ ReadOriginal
→ AppliedAndReadBack
→ Running
→ RestoreRequested
→ RestoredAndReadBack
```

实现必须使用 canonical session lock 的内核 advisory lock（如 `flock`）或安全目录内的原子 `O_CREAT|O_EXCL`；锁文件权限限制为当前用户，拒绝 symlink/path 替换。PID、run_id 等元数据只用于诊断，不是 ownership 证据；PID 复用不能证明锁过期。

锁文件元数据：

```text
pid
run_id
output_name
acquired_at
```

获取失败立即拒绝，不等待后覆盖其他 runner。

过期锁不自动删除。人工清理必须是显式操作，并记录风险。

锁只协调本工具实例；不能阻止用户或其它工具直接改 output。快照必须是本次 run 内不可变对象，同时保存 wall-clock 和 monotonic 时间；不跨进程猜测未完成 mutation。

恢复前重新读取并比较本 run 最后一次已验证的 applied 状态（身份、scale、transform）：

- 相等：继续用原始快照恢复；
- 不相等：默认判定外部修改冲突，拒绝覆盖并报告 `RecoveryUnverified`；
- 显式一次性 `--force-restore` 只能由调用方主动要求，记录操作者和原因，仍需逐字段 read-back，不能把身份不明变成 confirmed。

宿主崩溃后的残留快照只能供人工诊断；不能自动猜测恢复或声称已恢复。

## 7. 失败与退出语义

### 7.1 分类优先级与矩阵折叠

每个 scale×transform 是独立 case；默认策略是任一 case 发生 apply、child 或恢复异常后停止后续 case，并先恢复原始快照。只有未来显式的 `continue-after-case-failure` 选项经过单独审查，才能继续矩阵。

全局结果按以下顺序折叠，绝不使用最后一个 case 覆盖前面的结果：

```text
RecoveryUnverified
  > HarnessError
  > EnvironmentUnavailable
  > child signal/timeout/nonzero
  > all cases successful
```

在已经取得原值后，恢复失败必须覆盖 child 和 calibration 结果。最终协议 `summary.status` 必须为 `failed` 或 `harness_error`，绝不能因 child exit 0 而为 `ok`；CLI 最终退出码使用独立的 recovery/harness 错误码，具体数值须在方案阶段冻结，不能复用 child 的成功码。

`records_total/ok/failed` 只统计已经发出的业务 records；未开始的 case 不伪造为失败记录。宿主统一在恢复和锁处理结束后发出唯一 summary；summary 生成失败本身是 `HarnessError`。

### 7.2 子进程监督与信号

host runner 是唯一 supervisor：启动 child 为独立进程组，并记录内核返回/实际观察到的 PGID；不能用 launcher 的 shell job PID 猜测 PGID。超时或收到 SIGINT/SIGTERM 时先向实际整个进程组发送 TERM，等待有限的 child grace period，再向同一 PGID 发送 KILL 并等待回收。实际实验发现只 TERM supervisor 时，忽略 TERM 的后代仍可存活；因此只终止 supervisor 不合格。监督器进入恢复阶段后屏蔽重复中断，只设置 `interrupted-during-recovery` 标志；恢复使用独立且更短的 deadline，不因 child 不退出而无限等待。

`SIGINT`、timeout 和 child 非零都必须在宿主仍存活时进入 `RestoreRequested`。宿主自身被 SIGKILL 或崩溃时，任何 finally/trap 都不可靠，不能生成恢复成功记录；下次人工检查只能把本次标为 `RecoveryUnverified`。child 的 KILL 只表示 child 未正常结束，宿主仍必须尝试恢复并按 read-back 结果分类。

### 7.3 建议状态映射

| 情况 | 分类 |
|---|---|
| 参数、白名单、模式错误 | `HarnessError` |
| 获取锁失败 | `HarnessError` |
| 无法读原值 | `EnvironmentUnavailable` |
| apply 命令失败 | `EnvironmentUnavailable`，随后尝试恢复 |
| apply read-back 超时 | `EnvironmentUnavailable`，随后尝试恢复 |
| child exit 1/2/3 | 保留 child 语义，随后尝试恢复 |
| SIGINT / timeout | 先恢复，再按恢复结果分类 |
| restore 命令失败 | `RecoveryUnverified` |
| restore read-back 不匹配 | `RecoveryUnverified` |
| output 身份不稳定 | `RecoveryUnverified` |
| SIGKILL / 宿主崩溃 / compositor 重启 | `RecoveryUnverified`，不得声称成功 |

### 7.3 协议裁决（本轮冻结）

采用方案一：不升 v2，冻结 v1 的 recovery 值集合：

```text
not_requested | confirmed | unverified
```

语义固定为：

| recovery | 允许条件 |
|---|---|
| `not_requested` | 本次没有 output mutation 请求，或尚未取得原值而没有可执行恢复 |
| `confirmed` | 原始身份字段稳定、scale/transform 逐字段规范化相等、锁释放结果可证明 |
| `unverified` | 发生 mutation 且身份、字段、恢复、锁释放或宿主存活证据任一不足 |

`status=unverified` 只用于生命周期记录；`summary.status` 必须为 `failed`，不能让 child exit 0 抵消恢复失败。当前 Niri 的 `NameOnly` 身份永远不允许 `confirmed`。本裁决只冻结语义，正式 parser 校验和 producer 修改必须在方案阶段进行，避免草案阶段改变已发布生产行为。

## 8. fake adapter 测试矩阵

实现前必须使用内存 fake adapter，不调用 Niri：

| 编号 | 场景 | 期望 |
|---|---|---|
| F1 | 正常 apply/run/restore（Stable identity） | 迁移完整，原值恢复并可确认 |
| F18 | 正常 apply/run/restore（NameOnly identity） | 配置可恢复但不得报告 confirmed |
| F2 | 未授权 mutation | 不调用 set |
| F3 | live-container mutation | 配置拒绝 |
| F4 | 空/非法 scale | 配置拒绝 |
| F5 | apply 命令失败 | 尝试恢复，结果非成功 |
| F6 | apply read-back 不匹配 | 超时后恢复 |
| F7 | child exit 1/2/3 | child 语义保留，仍恢复 |
| F8 | SIGINT | 恢复后结束 |
| F9 | timeout | 恢复后结束 |
| F10 | restore 第一字段失败 | 尝试其它字段，最终未验证 |
| F11 | restore read-back 不匹配 | `RecoveryUnverified` |
| F12 | identity 改变 | 不确认恢复 |
| F13 | 并发抢锁 | 第二 runner 立即拒绝 |
| F14 | parser 缺字段/重复字段 | parser error |
| F15 | parser 未知格式 | 不回退默认值 |
| F16 | SIGKILL 模拟 | 只生成未验证证据 |
| F17 | 用户外部改动 | 默认不强制覆盖 |
| F19 | ReadOriginal 失败 | 锁释放，无 recovery 声明 |
| F20 | set_scale 成功、set_transform 失败 | 两字段均尝试恢复 |
| F21 | child spawn 失败 | 仍恢复并折叠结果 |
| F22 | 前 case 失败、后 case 未运行 | 不丢失前 case 失败 |
| F23 | read-back 返回旧值或乱序异步值 | 超时/失败，不接受旧值 |
| F24 | 恢复期间 SIGINT | 屏蔽重入，完成有限恢复窗口 |
| F25 | child 含后代进程 | 按进程组终止并回收 |
| F26 | lock symlink/path 替换 | 拒绝并报告 HarnessError |
| F27 | PID 复用/stale lock | 不自动删除，人工确认 |
| F28 | lock release 失败 | 记录并升级结果 |
| F29 | output 消失后同名出现 | 不确认身份 |
| F30 | compositor 重启 | RecoveryUnverified |
| F31 | 恢复前用户修改 output | 默认不覆盖 |
| F32 | 恢复后的 records/summary 计数 | 只统计已发业务记录 |
| F33 | SIGKILL/宿主崩溃分类 | 不伪造恢复记录 |
| F34 | scale canonicalization | `1.250` 等价 `1.25`，异常不 epsilon 放行 |
| F35 | 多 header/控制字符/截断 parser | 拒绝且不 fallback；协议边界 7/7 已实测 |
| F36 | launcher PID 与实际 PGID 不同/child 忽略 TERM | 使用实际 PGID，TERM 后 bounded KILL 整组并回收 |
| F37 | 恢复期间重复 SIGINT/SIGTERM | 只记录中断，不重入恢复 |
| F38 | child spawn 失败（已有 snapshot） | 进入恢复 |
| F39 | restore 第一字段失败 | 继续尝试第二字段 |
| F40 | stale metadata + held kernel lock | 不得绕过锁 |
| F41 | 矩阵 summary/exit 折叠 | 恢复失败覆盖 child 成功 |
| F42 | 未连接 output rc=0/deferred | 必须拒绝视为当前 apply 成功 |

每个 fake adapter 测试必须检查：

- 状态迁移顺序；
- compositor 调用参数数组等价性；
- restore 使用 snapshot 原值；
- 恢复失败覆盖 child 结果；
- lock 的获取和释放；
- 没有任何默认值 fallback。

## 9. 实机实验门槛

fake adapter 全部通过后，才允许在当前 Niri 上做真实实验。实验期间必须假设用户正在使用电脑：

- 终端持续滚动；
- 光标移动；
- 其它窗口刷新；
- 不将 probe 自身输出留在测量画面上；
- 每次实验先只读记录原值；
- 每次 apply 都 read-back；
- 每次 restore 都 read-back；
- 输出身份不足时不报告 confirmed；
- Ctrl-C、timeout 和正常退出分别记录；
- 失败后检查桌面状态，但不把人工观察当作协议真值。

SIGKILL、宿主崩溃和 compositor 重启只做分类实验，不做恢复成功率承诺。

## 10. 七项决策总表

| # | 决策 | 本轮裁决 | 证据/剩余验收 |
|---|---|---|---|
| 1 | fake adapter F1–F42 | 接受为唯一无显示服务器的状态机验收层；不作为 Niri 证据 | 临时模型实验 11/11 通过；`fidus-test` 已加入 12 个 P5 测试函数，并以可写 Cargo 缓存通过 27/27 crate 测试、clippy 和 doc；正式覆盖仍不等于真实 Niri 证据 |
| 2 | recovery 协议值 | v1 冻结 `not_requested` / `confirmed` / `unverified`；未知值拒绝 | parser 实际实验 4/4 通过，单测已补；producer 仍待方案阶段接入 |
| 3 | Niri parser fixture | 采用版本化 grammar；缺失/重复/控制字符/截断/未知格式 fail-closed；transform 使用 canonical enum | 实机输出形状 7/7、合成歧义边界 3/3、FIDUS_RESULT 协议边界 7/7 通过；`90` 读回 `90° counter-clockwise`；已加入 `NIRI_OUTPUTS_V25_FIXTURE` 和 deferred fixture 测试；仍不等于真实 parser adapter |
| 4 | canonical lock/stale lock | 宿主 session canonical lock；优先内核 advisory lock；stale 不自动删除 | `flock` 抢占拒绝/释放后重获、symlink 检测、PID metadata 不绕过锁、并发竞争通过；真实 release failure 仍挂起 |
| 5 | signal supervisor | 宿主独立监督 child 进程组；TERM→有限等待→KILL；恢复期防重入 | runner 已实现实际 PGID、timeout、TERM→bounded wait→KILL→wait；fake child 回归通过；真实信号异常路径挂起 |
| 6 | failure/matrix/exit truth table | 恢复失败最高优先；默认首个 case 失败即停止；summary 由宿主唯一生成 | recovery 三值、code 4、child/timeout/external-change 优先级和单一 lifecycle+summary 已通过 fake 回归；F1–F42 映射见 `docs/measurements/p5-f1-f42-matrix.md` |
| 7 | 活动桌面实机门槛 | 必须假设用户持续使用；只验证本机 Niri 行为；NameOnly 不得 confirmed | scale/transform 正常路径字段恢复通过；canonical transform 已验证；NameOnly 仍为 unverified；异常路径未做桌面实验 |

## 11. 草案全审结论（历史快照）

```text
有条件通过；七项设计决策已完成，允许转入方案编写；不允许直接实现真实 mutation。
```

上述结论是草案阶段记录。当前已进入方案并实现最小显式 runner；当前 runner 的无显示回归、timeout、外部修改保护、单一 lifecycle+summary 输出和 F1–F42 映射见 `docs/measurements/p5-f1-f42-matrix.md`。真实桌面错误处理单点验证已冻结（约等于挂起），跨机器发布验证进入社区互助处理中。已完成的协议决策不能被这些实验重新放宽：NameOnly 永远不能升级为稳定身份，SIGKILL/宿主崩溃/compositor 重启永远不能声称恢复成功；意外恢复不可靠，必要时只能按规范提供人工恢复。
