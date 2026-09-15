# P5 F1–F42 无显示验收矩阵

> 本表只记录不需要真实 compositor 的验收。`fake` 表示 fake Niri/内存模型回归，`protocol` 表示正式 `fidus-test` parser 回归，`real-pending` 表示必须在用户明确授权和正在使用的桌面上验证。不能把 fake 或 model 结果升级为真实 Niri 证据。

| 编号 | 场景 | 当前证据 | 状态 |
|---:|---|---|---|
| F1 | Stable identity 正常恢复并确认 | 当前 Niri 无稳定 identity；fake 仅验证恢复字段 | real-pending |
| F2 | 未授权 mutation 不调用 set | fake runner | pass |
| F3 | live-container 拒绝 mutation | `scripts/live_container.sh` 参数回归 | pass |
| F4 | 空/非法 scale 拒绝 | runner allowlist + shell 语法 | pass |
| F5 | apply 命令失败仍恢复 | fake adapter/model | pass |
| F6 | apply read-back 不匹配 | fake parser/model | pass |
| F7 | child 非零仍进入恢复 | fake runner | pass |
| F8 | SIGINT 后恢复 | fake/model；真实桌面挂起 | real-pending |
| F9 | child timeout 后恢复 | fake runner `--timeout-seconds` | pass |
| F10 | restore 第一字段失败继续第二字段 | `fidus-test` fake adapter | pass |
| F11 | restore read-back 不匹配 | fake adapter/model | pass |
| F12 | identity 改变不得确认 | `fidus-test` fake adapter | pass |
| F13 | 并发抢锁 | fake runner + `flock` | pass |
| F14 | parser 缺失/重复字段 | parser 单测 + fake fixture | pass |
| F15 | parser 未知格式不 fallback | fake fixture | pass |
| F16 | SIGKILL 分类不伪造恢复 | model；真实桌面挂起 | real-pending |
| F17 | 用户外部修改不强制覆盖 | fake runner 外部状态变化 | pass |
| F18 | NameOnly 恢复不 confirmed | fake runner，当前 Niri 规则 | pass |
| F19 | ReadOriginal 失败释放锁 | fake runner | pass |
| F20 | scale 成功、transform 失败仍双字段恢复 | fake adapter | pass |
| F21 | child spawn 失败后恢复 | model；真实宿主异常挂起 | real-pending |
| F22 | 首个 case 失败后停止 | `fidus-test` model | pass |
| F23 | 旧值/乱序 read-back 拒绝 | fake parser/model | pass |
| F24 | 恢复期间信号不重入 | `fidus-test` model | pass |
| F25 | child 后代按组回收 | 实际 PGID 实验；fake supervisor | pass |
| F26 | lock symlink/path 替换拒绝 | fake runner | pass |
| F27 | stale metadata 不绕过 held lock | `flock` model/实验 | pass |
| F28 | lock release failure 升级结果 | 需要专门注入 fd/unlock 故障 | real-pending |
| F29 | output 消失后同名出现不确认 | NameOnly/identity model | pass |
| F30 | compositor 重启 | 真实 compositor 异常 | real-pending |
| F31 | 恢复前用户修改 output | fake runner 外部修改 | pass |
| F32 | summary 只统计已发业务记录 | parser validation | pass |
| F33 | SIGKILL/宿主崩溃分类 | 语义已冻结；真实路径挂起 | real-pending |
| F34 | scale canonicalization | fake/parser 单测 | pass |
| F35 | 多 header/控制字符/截断 | fake fixture + parser 边界 | pass |
| F36 | launcher PID 与 PGID 不同、TERM/KILL | 实际 PGID 实验 + fake runner | pass |
| F37 | 重复 SIGINT/SIGTERM 不重入 | model/`fidus-test` 单测 | pass |
| F38 | 已有 snapshot 的 spawn 失败 | model；真实 spawn 故障挂起 | real-pending |
| F39 | restore 第一字段失败继续第二字段 | fake adapter | pass |
| F40 | stale metadata + kernel lock | `flock` 实验 | pass |
| F41 | recovery 覆盖 child success | runner recovery exit 4 + summary failed | pass |
| F42 | 未连接 output rc=0/deferred 拒绝 | fake deferred fixture + Niri 实验记录 | pass |

## 结论

当前可在无显示环境完成的 P5 代码、协议、fake adapter、fixture、锁模型和 supervisor 模型门槛已完成；`fidus-test` 与 runner 回归必须持续保持全绿。剩余 `real-pending` 项均属于真实 compositor/宿主异常路径，不能通过 fake 证据替代；真实桌面错误处理单点验证已按项目进程附件冻结（约等于挂起），不再要求或默认执行。意外恢复不可靠，若获得单独授权，只提供文档中的人工恢复路径。

跨机器发布验证已进入社区互助处理中，是独立的项目级发布事项，不属于本表的 P5 本机实现证据。
