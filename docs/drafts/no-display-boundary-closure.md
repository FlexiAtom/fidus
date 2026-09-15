# 草案：无显示核心边界收口

> 阶段：已通过人工复核并转入方案；以下内容保留为实现细节与验收依据，不单独扩大授权范围。
>
> 上游提案：[`docs/proposals/core-hardening-followup.md`](../proposals/core-hardening-followup.md)
>
> 范围：仅处理无显示、非破坏性、可重复的 fake adapter、协议模型、纯函数、脚本和文档工作。真实桌面 mutation、破坏性实验、跨机器验证、正式发布、签名和 push 均不在本草案授权范围内。

## 1. 目标与完成定义

目标是在不把 fake 证据升级为真实环境证据的前提下，补齐当前全量复审中尚未关闭、且可以在本机无显示完成的边界工作：

1. 将 Wayland buffer 的 deferred retirement 从单个状态判定扩展为可驱动的 fake 状态机；
2. 将 X11 几何 fixture 扩展为可模拟协议错误和部分创建失败的 fixture；
3. 处理当前 checked-in 历史 manifest/provenance 与新格式的并存关系，不重写历史产物；
4. 对 `ProbabilisticPosition::new()` 的兼容策略作出可执行决策并补迁移约束；
5. 保持 SearchRoi 预算、SBOM 精确校验和既有 R1–R4 回归全绿。

完成定义：每个保留工作包都有对应代码入口、无显示测试和失败模式说明；workspace test、clippy、rustdoc、脚本、SBOM、P5 matrix、fixture 与 diff 门禁通过；审查记录明确区分“fake/本机证据”和仍 pending 的真实环境证据。

## 2. 工作包与代码入口

### WP-1：Wayland deferred retirement fake 状态机

**入口：** `crates/fidus-backend-wayland-layer/src/session.rs`、`marker.rs`、`capture.rs`。

**做法：**

- 在测试可见的协议模型中分别建模 capture、marker、clear buffer；
- 明确状态转移：`Idle -> Submitted -> Ready/Failed -> Released`，以及 `Submitted -> Released` 的异常/快速释放路径；
- 将 frame callback、screencopy ready、wl_buffer release 作为互不替代的事件；
- 模拟 release 延迟、release 不到达、surface closed、失败提交和旧事件晚到；
- 验证未收到 release 时不得 replacement、reuse 或宣称 teardown 成功；
- 验证 poisoned projector 不会重新提交旧 buffer；
- 只在所有相关 buffer 达到可退休状态后模拟 SHM 资源销毁。

**失效模式：** 如果 fake 状态机把 `ready` 或 frame callback 错当 release，测试会错误放行 buffer 复用；如果把超时直接转为 `Released`，会掩盖真实 compositor 永不释放的风险。fake 只能证明本地状态转移和拒绝策略，不能证明真实 compositor 的事件顺序。

**验收：** 新增状态转移表测试和组合路径测试；不引入真实 Wayland 连接，不修改真实桌面。

### WP-2：X11 协议错误 fixture

**入口：** `crates/fidus-backend-x11/src/lib.rs`，必要时新增测试专用模块或 `tests/` fixture。

**做法：**

- 保留现有纯函数几何 fixture；
- 增加 fake reply 解码入口，覆盖 depth/visual 不可接受、reply 数据短于 `width*height*4`、超长数据截断；
- 增加 rootless `BadMatch` 的错误映射 fixture；
- 模拟多 marker 创建中第 N 个请求失败，验证已创建 marker 全部清理；
- 模拟 `map_window` 或同步失败，验证 cleanup 错误不被成功结果覆盖；
- 所有 fixture 必须使用协议 reply/错误模型，不连接真实 X server。

**失效模式：** 仅测试纯几何不能证明 X11 reply 错误映射；若 fake server 偷偷接受非法 depth 或自动补齐短数据，会重新制造“看似合法”的 Frame。fixture 必须拒绝缺失数据，不用默认像素填充。

**验收：** fixture 覆盖上述错误分支且不 panic；真实 X server 的 BadMatch 和断连仍标为 pending。

### WP-3：历史发布元数据与新格式并存治理

**历史归档标注文件（采纳的修正）：**

- 新增 `fidus-live-debian12.archive-status.json`，作为机器可读的历史状态旁证；
- 标注文件绑定旧归档的 `filename`、`sha256`、`size_bytes`；
- `status` 固定为 `historical`，`current_release_eligible` 固定为 `false`；
- 旧来源字段使用 `historical_commit`、`historical_tree`，不得写入当前格式的 `source.revision_binding`；
- `source_binding.status` 使用 `legacy-unbound`，表示历史来源记录不能证明当前源码绑定；
- 标注文件加入正常 Git source digest，不加入可变签名输出排除列表；
- 校验结果区分 `current-bound`、`historical-unbound`、`invalid`；历史标注只能提高可解释性，不能绕过当前绑定、签名或发布检查；
- 标注文件自身 hash/size 不匹配、被改成当前状态、字段缺失或 JSON 非法时必须返回 `invalid`；
- 标注文件不修改旧 archive、manifest、provenance、signature，也不声称可复现或已正式发布。

**入口：** `scripts/source_revision.py`、`scripts/build_release.sh`、`scripts/sign_release.sh`、`scripts/verify_sbom.sh`、`fidus-live-debian12.*` 文档。

**做法：**

- 不修改旧归档的 hash、source、签名或 provenance，使历史文件保持历史事实；
- 为新格式定义明确 schema/version 和 source binding 必需字段；
- 增加离线 manifest/provenance 一致性检查：artifact filename/hash/size、SBOM filename/hash、image reference、source binding 必须互相对应；
- 对历史格式给出明确的“不可作为当前 release 证据”错误或说明；
- 用临时目录和合成小文件测试正向/负向校验，不执行 Docker build 或签名。

**失效模式：** 如果为了让旧文件通过验证而补写当前 source binding，就会把历史归档错误归因于新源码；如果只校验文件存在而不校验 hash/size，manifest 仍可与错误归档配对。所有不一致必须 fail-closed。

**验收：** 历史文件不会被重绑定；合成 manifest 的篡改、缺字段、hash/size/SBOM 不一致均被拒绝；正式 Docker、签名和发布仍 pending。

### WP-4：兼容构造 API 决策

**入口：** `crates/fidus-core/src/estimate.rs` 及调用方。

**做法：**

- 先盘点公开调用方和 semver 约束；
- 当前版本允许增加弃用提示而不改变返回类型：`new()` 已标记 `deprecated`，继续保留 ABI/API 兼容性，并引导到 `try_new()`；
- `new()` 仍不是可信边界；所有 Engine/public output 入口继续执行终局验证；下一个 breaking release 再评估删除或改变返回类型；
- 补充非法值经过旧构造后在 Engine 出口被拒绝的回归。

**失效模式：** 直接删除或改变返回类型会破坏下游编译，且不能自动提高已有二进制的安全性；继续保留旧构造但没有出口校验则会允许非法概率测量流入融合。决策必须以实际调用方和版本策略为依据。

**验收：** 形成明确兼容策略、迁移说明和回归，不在未决策前偷偷改变公共 API。

## 3. 顺序、依赖与回滚

执行顺序：

1. WP-1 状态模型与测试；
2. WP-2 X11 fixture；
3. WP-3 发布元数据离线契约；
4. WP-4 API 兼容决策；
5. 全量门禁与整体复审。

依赖：现有 `BufferState`、`Frame::is_valid()`、X11 错误类型、临时目录测试工具和现有 SBOM/manifest 脚本；不新增外部依赖。

回滚边界：每个 WP 保持独立差异；若 fake 状态无法证明某个转移，删除该放行路径，保留 fail-closed 行为，不把测试改成宽松断言。历史发布文件不得通过重写来“修复”。

## 4. 验收命令

```sh
cargo test --locked --offline --config .cargo/config.ci.toml --workspace
cargo clippy --locked --offline --config .cargo/config.ci.toml --workspace --all-targets -- -D warnings
cargo doc --locked --offline --config .cargo/config.ci.toml --workspace --no-deps
bash -n scripts/*.sh
python3 -m py_compile scripts/*.py tools/*.py
bash scripts/verify_sbom.sh
bash scripts/check_p5_matrix.sh
bash scripts/test_output_mutation_runner.sh
python3 tools/test_generate_test_images.py
git diff --check
```

禁止把以下动作作为本草案验收：真实桌面 mutation、SIGKILL/compositor restart、真实 X server/Wayland compositor 错误注入、跨机器验证、Docker 正向发布、GPG 签名或 push。

## 5. 自审裁枝与未纳入项

保留：WP-1 至 WP-4。它们都有明确代码入口、实际失败模式和无显示验收路径。

不纳入：

- 真实 compositor 的 release/recovery、稳定 identity、跨机器验证：属于环境证据，继续挂起；
- 新定位算法、GradientField、周期纹理理论、motion gate 创新：没有证明是当前边界修复的必要依赖；
- 正式 release、签名、registry manifest 发布：本草案只做离线一致性契约；
- 通过修改历史归档内容消除旧格式差异：违反历史 provenance 不可重写原则。

## 6. 可执行性评估

- 目标明确：补齐可在无显示环境验证的边界，不声称真实环境闭环；
- 入口明确：四个 WP 均列出实际源文件和脚本；
- 依赖明确：只依赖现有 Rust、Python、fake/protocol 模型；
- 顺序明确：先状态模型，再协议 fixture，再发布契约，最后 API 决策；
- 验收明确：命令、负向断言和完成定义已列出；
- 回滚明确：各 WP 独立回退，失败时保持拒绝；
- 授权明确：仅无显示、非破坏性工作；真实环境和发布动作排除。

因此本方案已通过全量审查，允许在既定无显示、非破坏性范围内按 WP-1 至 WP-4 细化实现；WP-3 历史标注与 current-bound 元数据离线校验已实现，WP-4 兼容 API 已按 deprecated 保留策略实现；WP-1/WP-2 仍需继续补齐；每个工作包仍须单独提交实际门禁证据，不扩大到真实环境或正式发布。
