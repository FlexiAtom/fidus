# 草案 · P4 `fidus-test` 跨环境测试子项目

> **来源**：[`docs/proposals/P4-fidus-test-subproject.md`](../proposals/P4-fidus-test-subproject.md)
> **状态**：✅ 已提升为方案并完成发布收口（2026-09）；CI 协议核心、live-host 协议桥接、本机 live-container 实测和 Debian runtime Release 归档已完成。内容已并入 [`docs/spec.md` §8「P4 测试子项目方案」](../spec.md)。
>
> 关键纠偏：脚本、专用 crate、容器不是三选一，而是职责不同的共存层。
> **裁决**：十项裁决已确认；容器采用 `ci` / `live-host` / `live-container` 三模式。

## 1. 草案目标

创建 `crates/fidus-test`，承载跨环境测试的编排、诊断和报告，不承载 fidus 生产算法，也不迁移生产 crate 内的仿真测试。

第一阶段目标：

1. 让 CI 能在固定容器中重复执行 build/test/clippy/doc；
2. 让真实桌面测试能记录环境、噪声底、校准结果和 teardown；
3. 让容器访问宿主显示会话成为显式实验，而不是默认行为；
4. 保留现有 `fidus-live-calibrate` 与脚本兼容入口；
5. 把“测试没跑成”“fidus 失败”“测试工具解析失败”分开。

## 2. 实现路径审查：三者共存，不是三选一

### 2.1 三者处于不同维度

**脚本、专用 crate、容器可以共存，且应当共存：**

| 层 | 负责什么 | 不负责什么 |
|---|---|---|
| 宿主脚本 | shell 入口、环境探测、compositor 控制、容器启动、恢复兜底 | 生产算法、结果协议解析、第二套校准器 |
| `fidus-test` 专用 crate | 场景模型、结果协议、parser、报告、退出语义、薄 CLI | 直接拥有宿主 compositor 控制权、复制 backend |
| 容器 | 固定 toolchain/系统包、提供 `ci` 执行环境、可选承载 live binary | 自动获得宿主显示权限、替代真实桌面 |

它们的组合关系是：

```text
宿主脚本
  ├─ 直接启动 fidus-test binary        → live-host
  └─ 启动 fidus-test binary 的容器      → ci / live-container

fidus-test binary → 正式 fidus API → 被测系统
```

### 2.2 真正被否决的是“单层包办一切”

以下方案仍然否决：

- 只继续堆 shell：无法稳定承载协议、报告和生命周期模型；
- 只在 `fidus` 伞 crate 堆工具：继续混淆生产 facade 与测试职责；
- 只建专用 crate、不固定 CI 环境：缺少可重复构建载体；
- 一个高权限万能容器：扩大权限并掩盖真实环境差异。

这不是否决脚本、专用 crate 或容器本身，而是否决让任一层独占全部职责。

### 2.3 三者共存时的不可重复职责

共存没有本质冲突，但以下职责必须各有唯一拥有者：

1. **结果协议与解析**：只在 `fidus-test` 实现；脚本不重新正则解析，容器不复制 parser；
2. **compositor mutation authority**：只在宿主 runner；容器内 binary 不直接修改 scale/transform；
3. **恢复状态机**：由宿主 runner 统一负责，不能脚本和容器各自恢复；
4. **生产定位算法**：只在正式 fidus crate；测试层只调用公开 API；
5. **环境封装**：容器固定 CI 依赖，宿主保留真实 compositor 与真实用户活动。

### 2.4 三者的组合模式

- `live-host` = 宿主脚本 + `fidus-test` binary + 宿主显示会话；
- `ci` = 容器 + `fidus-test` binary + 无显示测试；
- `live-container` = 宿主脚本（控制/恢复） + 容器中的 `fidus-test` binary + 显式挂载宿主显示会话。

三种模式分开报告、分开统计、分开验收。

## 3. 目标目录与 Cargo 边界

```text
crates/fidus-test/
├── Cargo.toml
├── src/
│   ├── lib.rs
│   └── bin/
│       ├── fidus-test-live-calibrate.rs
│       └── fidus-test.rs
# 现有 fidus-live-calibrate / fidus-probe-marker 第一阶段仍由 fidus crate 保留
└── tests/
    ├── protocol.rs
    ├── config.rs
    └── lifecycle.rs
```

初始依赖边界：

```toml
[dependencies]
fidus = { workspace = true, default-features = false }
# feature 由草案实现矩阵决定，不能隐式打开 Wayland + X11
```

上面的 `fidus` 拼写仅代表占位，真正 Cargo 文件必须使用合法的 `fidus` key；feature 矩阵先在实现前确认：

| 构建 | fidus features | 预期用途 |
|---|---|---|
| `ci-core` | `default-features = false` | parser/config/report 与无显示测试 |
| `live-wayland` | `wayland-layer` | Wayland live/probe |
| `live-x11` | `x11` | 真 X11 live/probe |
| `live-all` | `wayland-layer`, `x11` | 开发者完整构建，非最小 CI |

禁止测试 crate 自己依赖 backend crate 来复制 backend 行为；它只通过正式 facade 运行被测系统。

## 4. 稳定结果协议

协议 emitter 接入后，新的 harness/live wrapper 在人读日志之外输出固定前缀行；现有旧 live binary 在兼容迁移完成前继续只输出人读日志：

```text
FIDUS_RESULT version=1 kind=environment run_id=r01 status=ready execution_mode=live-host backend=wayland compositor=niri output=eDP-1 scale=1.25 transform=normal
FIDUS_RESULT version=1 kind=calibration run_id=r01 status=ok execution_mode=live-host backend=wayland method=crosshair rms_residual_px=0.203 verification_max_err_px=0.731 consistency_max_err_px=0.856
FIDUS_RESULT version=1 kind=lifecycle run_id=r01 status=ok execution_mode=live-host teardown=confirmed recovery=not_requested
FIDUS_RESULT version=1 kind=summary run_id=r01 status=ok execution_mode=live-host records_total=3 records_ok=3 records_failed=0
```

协议规则：

- 行首必须是 `FIDUS_RESULT `；
- 公共字段 `version/kind/run_id/status/execution_mode` 唯一且必需；
- `kind` 为 `environment/calibration/lifecycle/summary` 之一，并按 kind schema 校验字段和 status；
- key 只允许 `[a-z0-9_]+`；value 第一版只允许无空格 ASCII token；
- 重复 key、未知 `version`、缺字段、非法数字、run_id/mode 不一致均为 `HarnessError`；
- `summary` 是唯一终态，且 `records_total/ok/failed` 只统计 summary 之前的业务记录；无 summary 不得判为通过或失败；
- 人读 stdout/stderr 与机器结果行同时保存。

### 4.1 伪代码：解析与整 run 校验

```text
parse_result_line(line):
    if not line.starts_with("FIDUS_RESULT "):
        return NotAResultLine
    fields = parse_unique_ascii_key_values(line.after_prefix)
    require fields.version == "1"
    require fields has kind, run_id, status, execution_mode
    require fields.kind in {environment, calibration, lifecycle, summary}
    validate_kind_schema_and_status(fields)
    return ResultRecord(fields)

validate_run(records):
    require records is non-empty
    require exactly one summary record
    require every record has same run_id and execution_mode
    business = records excluding summary
    require summary.records_total == business.len
    require summary.records_ok + summary.records_failed == business.len
    require summary counts match business statuses
    return summary
```

## 5. 场景配置与 output mutation

配置只允许白名单值：

```text
ScenarioConfig {
    scenario: L9Live | Probe | Smoke,
    backend: Auto | Wayland | X11,
    output: OptionalOutputName,
    scales: NonEmpty<Vec<FinitePositiveScale>>,
    transforms: NonEmpty<Vec<Normal|90|180|270>>,
    runs: PositiveInteger,
    allow_output_mutation: bool,
    execution_mode: Ci | LiveHost | LiveContainer,
}
```

伪代码：

```text
run_scenario(config):
    reject runs == 0
    reject scales.empty or transforms.empty
    reject live-container + allow_output_mutation
    require mode-specific prerequisites

    original = None
    if config.allow_output_mutation:
        require config.execution_mode == LiveHost
        original = host_adapter.read_output_state(config.output)

    result = with_recovery_guard(original):
        for scale in config.scales:
            for transform in config.transforms:
                if mutation_requested:
                    host_adapter.set_and_verify(scale, transform)
                noise = run_no_marker_noise_floor()
                for run in 1..=config.runs:
                    record = run_calibration_once()
                    emit_record(execution_mode, noise, record)

    if original exists:
        restore_and_read_back(original) or return RecoveryUnverified
    return aggregate_without_cross_mode_merging(result)
```

约束：只有宿主 adapter 能调用 compositor 控制命令；`live-container` 只能请求或读取，不拥有 mutation authority。

## 6. 三种执行模式伪代码

### 6.1 `ci`

```text
ci_entrypoint:
    assert no display socket is required
    run cargo test --workspace
    run cargo clippy --workspace --all-targets
    run cargo doc --workspace --no-deps
    run fidus-test protocol/config/lifecycle tests
    report Debian base image digest, Rust 1.86.0 toolchain, packages, locale, timezone
```

`ci` 成功只说明确定性项目检查成功，不报告 backend live 成功。

### 6.2 `live-host`

```text
live_host:
    verify current user session and selected backend
    capture activity declaration as metadata only
    noise = no_marker_burst_in_same_process()
    run fidus-test binary with stdout/stderr redirected
    parse FIDUS_RESULT records
    mark teardown=confirmed only when fidus-owned destroy_projector returns successfully
    # compositor window-tree queries remain human diagnostics, never protocol truth
    if mutation:
        host_runner.restore_original_output_and_read_back()
    classify failure or success
```

### 6.3 `live-container`

```text
live_container:
    reject output mutation request
    assert explicit container-live opt-in
    mount only declared socket/session paths read-only where possible
    run as current uid:gid by default; accept explicit UID:GID for permission matrix tests
    reject malformed UID:GID before invoking runtime
    do not mount HOME or /dev/dri by default
    execute test binary
    record mount/uid/image metadata
    classify missing socket/permission as EnvironmentUnavailable
    never merge result with live-host
```

宿主 runner 可以在容器外恢复输出；容器内不能拥有第二个恢复器。

## 7. 生命周期与异常状态

状态模型：

```text
Prepared
  -> Running
  -> Completed
  -> FidusFailed
  -> EnvironmentUnavailable
  -> HarnessError
  -> RecoveryUnverified
```

伪代码：

```text
on_exit(signal_or_result):
    if output_was_mutated:
        if recover_and_read_back():
            lifecycle = Restored
        else:
            lifecycle = RecoveryUnverified
    if signal == SIGKILL or host_lost:
        lifecycle = RecoveryUnverified
```

可测试：正常退出、SIGINT、超时、子进程错误、恢复命令失败。

不可承诺：SIGKILL、宿主崩溃、compositor 重启后的自动恢复。

## 8. 测试分层

保留生产 crate 内仿真测试；`fidus-test` 新增：

- 协议 parser：正常、未知版本、重复 key、缺字段、非法数字、非结果日志；
- 配置 parser：空列表、零 runs、非法 scale、容器 mutation 组合；
- 生命周期：正常、SIGINT/timeout 模拟、恢复成功/失败；
- 报告：三种 execution mode 分离、环境错误不计入 fidus 失败率；
- CI smoke：镜像内的固定命令与依赖清单。

## 9. 草案全审后的定案

### 9.1 其他实现是否更好

再次比较后，没有需要并行采用的替代方案：

- shell 只保留宿主控制与兼容 wrapper；
- `fidus` 伞 crate 只保留旧入口，不继续扩张；
- `fidus-test` 承担共享协议/报告/场景模型；
- `ci` 容器承担确定性检查；
- `live-host` 承担真实桌面证据；
- `live-container` 后置为显式实验。

### 9.2 仍未实现但必须保持的门槛

- feature 矩阵需实际构建验证；
- 镜像 digest、toolchain 和系统包需锁定；
- live-container UID/GID/socket 权限需在实现阶段实测；
- mutation authority 与恢复状态需有子进程回归；
- 生产 CLI 的结果行协议需先落地并测试。

### 9.3 草案结论

草案已经足够具体，可以提升为方案；但提升只批准接口、边界和实施顺序，不代表已经完成 `fidus-test` 实现，也不代表 live-container 已经在当前机器上可用。
