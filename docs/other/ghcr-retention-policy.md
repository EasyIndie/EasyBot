# GHCR 保留策略：正式 release 包 + main 最新镜像 — 方案

状态：**已批准 · 实现完成**（`scripts/ghcr-reconcile.py` 已实测；M1/M2 工作流未提交）。日期 2026-09-05。
需求：**保留**正式 release 包（全部）+ main 分支最新镜像；**不保留**历史 dev（sha7）/孤儿/幽灵条目；**要求**列表不随 CI 复涨。

## 1. 现状与病根

`ghcr.io/easyindie/easybot`（**org 级**包）。一次性清理后收敛到 31 版本；实测 `ghcr-reconcile.py` dry-run 又发现 2 个**未被任何保留 index 引用的 untagged 孤儿**（buildx/push 残留，故此前"已清干净"并不完全成立），`--apply` 后现为 **29 版本全保留、delete candidates=0**，7 tag（main/latest/0.0/0.0.36/37/38/40）均可解析。**当前已是终态。**

病根：`docker.yml` 每次 main push 发布一棵 dev 树（2 个 `push-by-digest` 平台子 + `merge` 打 `sha7`/`main`/`latest` index）；下次 push 把 `main`/`latest` 移走后，上一棵退化为 sha7-only + untagged 子，GHCR 无 GC → 永久累积。

## 2. 目标终态（长期不变量）

任一时刻 GHCR 仅含：**全部正式 release 树 + 当前 `main`/`latest` 树**（含各自子/attestation）。其余自动删除。

## 3. 机制 M1（扩展版，推荐）：每次 main push 成功后全量 reconcile

`docker.yml` `merge` 作业在成功推送新 main index **后**调用 `scripts/ghcr-reconcile.py`，执行一次完整图感知 prune：

- 保留集 = 当前 `main`/`latest` 树（新 index 及其子）+ **全部 release 树**（含子与 attestation）；
- 删除其余：历史 sha7 树、被取代的旧 main 树、孤儿 untagged、孤儿 attestation；
- 图感知：删除集永不包含保留集内 index 引用的子（子 manifest 可能在 main/release 间共享 → 先按保留集算 protected）；子先于父；保留 index manifest 读不到 → fail-closed 中止（宁可少删不误删）；幂等；
- push 失败则本次跳过（旧 main 保留、main 不悬空），下次成功时 reconcile 一并扫净失败残留；
- step 为 **best-effort**（`continue-on-error`，仅当 secret `GHCR_PRUNE_TOKEN` 存在才跑）：镜像发布是热路径，清扫失败不应使其变红；失败残留由下一次成功自愈 + §4 M2 兜底。

**不变量**：每次成功 push 都把列表恢复到"release 全 + 1 棵 main"（当前 4 release + 1 main ≈ 29 版本，数量随树结构固定）→ **常规下不随 push 复涨**。失败窗口期泄漏由下一次成功自愈；唯一理论缺口 = docker.yml 长期连续无成功（由 §4 M2 兜底）。

> 朴素 M1（只删"被这一棵取代的上一棵"）**不推荐**：merge 失败各留 ≤2 孤儿、清理 step 漏跑永久泄漏一棵 sha7 树，无自愈 → 仍会按失败次数慢速复涨。

## 4. 机制 M2（可选保险带）：定时 reconcile

`scripts/ghcr-reconcile.py`（同一脚本）+ `.github/workflows/ghcr-prune.yml`：`schedule` 每周（周一 05:17 UTC）+ `workflow_dispatch` 手动，保留集同上。独立于 docker.yml 运行——**docker.yml 长期故障/人为 buildx push 时仍兜底**；将来若想给 release 加保留上限也可在此收紧（`--keep-releases N`）。此工作流**不** `continue-on-error`——清扫是它唯一职责，坏了就该亮红灯。默认 `--dry-run`，工作流内显式 `--apply`。

## 5. 前置条件（M1/M2 均需）

包为 org 级，repo 的 `GITHUB_TOKEN` 无权删 org 级包 → 需维护者创建 **org 级 fine-grained PAT**（仅 org `EasyIndie`、Packages read+write/delete）存 repo secret（如 `GHCR_PRUNE_TOKEN`）。一次性创建、可随时撤销；M1 场景 docker.yml 亦使用该 secret（与现有 `packages: write` GITHUB_TOKEN 不冲突）。

## 6. 权衡

- dev 侧仅保留最新 main（无历史 sha7 预览镜像）；release 全保留（每发版 +1 棵树，低频、可预期）。
- 已删的 0.0.35 及更早 release 不可恢复。
- reconcile 在 docker.yml merge 热路径后执行；脚本默认 dry-run、fail-closed，风险收敛于"最多少删待下次自愈"，不会误删保留镜像。

## 7. 执行 checklist

1. ✅ 实现 `scripts/ghcr-reconcile.py`（默认 `--dry-run` 打印计划，`--apply` 执行；图感知、幂等、fail-closed、`--keep-releases N` 可选收紧）。
2. ⏳ 维护者创建 org 级 fine-grained PAT（仅 org `EasyIndie`、Packages read+write/delete）存 repo secret `GHCR_PRUNE_TOKEN`——未配置则 M1/M2 均自动跳过。
3. ✅ 改 docker.yml `merge` 作业尾部（best-effort `--apply` step）＋ 新增 `ghcr-prune.yml`（每周 + dispatch，`--apply`）。
4. ✅ 实测验证：`ghcr-reconcile.py` dry-run 在已清理 registry 上先报 2 孤儿 → `--apply` 清扫 → 再跑报 **delete candidates=0**（幂等、图安全成立）。
5. 上线观察：合入后下一个 main push，确认工作流绿 + 列表仍 = release 全 + 1 棵 main，tag 均可解析。
