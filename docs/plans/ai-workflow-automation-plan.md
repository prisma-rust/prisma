# Prisma AI 工作流自动化优化方案（阶梯验证、变更门禁与预算治理）

> **层级**：L3 流程治理 · 实施计划（落地后并入 [`ai-workflow.md`](../ai-workflow.md) §11 与 [`integrations.md`](../integrations.md)）
> **状态**：已评审优化，按 M1–M4 分 PR 落地
> **日期**：2026-11-19
> **作者**：JasonGu 的 AI agent（DeepSeek V4.1 Flash / OpenCode）
> **关联**：[`ai-workflow.md`](../ai-workflow.md)、[`integrations.md`](../integrations.md)、[`agent-issues.md`](../agent-issues.md)、`scripts/pr_ready.py`、`.github/workflows/ci.yml`

本文定义 Prisma 现有 AI 协作流水线的优化方案：把“每轮全量验证”改成按变更面收敛的阶梯验证，把周期自动化限制在**有新提交时**才运行，并堵住当前门禁中唯一一处“信任而非校验”的缺口。方案以降低 token、墙钟与人审注意力为目标，不以拆仓库或拆 job 为手段。

---

## 1. 目标、范围与完成定义

### 1.1 目标

1. **门禁可信**：PR 里的自检块必须绑定当前 HEAD，过期或伪造的块不能通过。
2. **验证分层**：agent 本地、PR、周期、发布四层各自只跑该层需要的检查，贵检查不进入高频回路。
3. **周期自动化有变更才跑**：无新提交时直接在 guard 处结束，下游全部跳过。
4. **外循环零爆炸半径**：先上只读巡检，代码写入型 worker 默认关闭、走白名单。

### 1.2 非目标

- 不拆分仓库、不拆分 crate、不新增 workspace。
- 不引入 automerge。
- 不做 7×24 常驻 agent 进程。
- 不做 fuzzing 自动开 issue 的自愈回路。
- 不为了并行度把 CI 拆成更多 job。

### 1.3 预算前提

`prisma-rust/prisma` 为公开仓库，GitHub 托管 runner 的 Actions 分钟不计费。因此“预算余额”指的不是 CI 分钟，而是：

| 预算 | 消耗方式 | 本文对应手段 |
|---|---|---|
| LLM token | 每轮会话读长文档、编译失败后的往返 | §8、§5.2、§4 的 quick-first |
| 墙钟 | 每个 job 的固定开销与重复编译 | §7 |
| 人审注意力 | 复核自检块、判断绿是否可信 | §5.1 |

若日后切换自托管 runner，§7 的收益会从“墙钟”变成“真金白银”，优先级相应上升。

### 1.4 完成定义

| 里程碑 | 内容 | 验收 |
|---|---|---|
| M1 | 自检块绑定 HEAD | 同一 HEAD 重复粘贴通过；旧 HEAD 的块被拒；`needs/self-check` 周触发趋近 0 |
| M2 | 变更门禁 + 只读巡检 | 无新提交的日子 workflow 在 `guard` 结束，下游全 `skipped`；有新提交时只跑一次 |
| M3 | 文档与上下文分层 | 每个 `area/*` 有明确最小必读集；`ai-workflow.md` 核心部分 <100 行 |
| M4 | worker（默认关闭） | 开关打开后单 issue 只产出一个 PR；无重复 PR；无 `dev` 直推 |

---

## 2. 现状与证据

以下结论均可直接定位到仓库文件；“结论强度”区分已确认事实与待测推断。

| 观察 | 证据 | 结论强度 |
|---|---|---|
| 仓库为公开仓库，Actions 分钟不计费 | `api.github.com/repos/prisma-rust/prisma` → `"private": false` | 已确认 |
| 编译后的 SPIR-V 与反射 JSON 已入库 | 51 个 `.spv`、21 个 `reflection/*.json`，合计约 1.3 MB；`.gitignore` 第 81 行注明 “Compiled SPIR-V is committed” | 已确认 |
| `ci.yml` 中“`.spv` 未入库 / gitignored”的注释已过期 | `.github/workflows/ci.yml` 的 `features`、`desktop` 注释与 `.gitignore` 矛盾 | 已确认 |
| 6 个 job 各自执行 `Setup slang + compile shaders` | `features`、`lint`、`desktop`、`user-projects`、`dist-size`、`shaders` | 已确认 |
| `dist-size` 在每次 push 到 `master`/`dev` 时运行 nightly `build-std` 全量发布构建 | `ci.yml` `on.push.branches` + `if: github.event_name == 'push'` | 已确认 |
| `shaders` job 无条件重编译并检查漂移 | `cache_shaders: "false"`，无路径条件 | 已确认 |
| `pr-triage` 只检查正文是否含 `**Result:` 行 | `pr-triage.yml` 的 `grep -qE '^\*\*Result:'` | 已确认 |
| `pr_ready.py` 已输出 `HEAD: \`<short-sha>\``，但无人比对 | `scripts/pr_ready.py` 自检块首行 | 已确认 |
| `pr_ready` 已对 `prism-render` 使用 `--test-threads=1` | `crate_test_command()` 中的 `#847` 处理 | 已确认（不构成缺口） |
| CI 与 `pr_ready` 的 crate 集合不一致 | CI 用固定 `-p` 列表；`pr_ready` 按 touched crates | 已确认；影响见 §7.3 |
| rust-cache 的 key 按 job 分叉 | `-lint`、`-desktop`、`-features`、`-projects`、`-shaders` | 已确认 |
| 没有事件驱动的 agent worker | `.github/workflows/` 只有 CI、triage、notify、cancel | 已确认 |
| `agent/working` 无 TTL，无 stale 清理 | `tools/ci/ensure_labels.sh` 只有标签定义 | 已确认 |

---

## 3. 设计原则

1. **先内后外**：内循环产出不可信时，外循环只会放大垃圾。P0 未完成前不启用 worker。
2. **跳过必须可见**：`skip` 与 `pass` 在输出里必须能区分；不允许把“没跑”当成“通过”。
3. **只读优先**：外循环第一阶段只做标签、评论、摘要，不写代码、不开 PR。
4. **默认关闭**：任何会写仓库的自动化都必须有仓库变量开关，且开关优先于触发条件。
5. **单一事实源**：变更判断只实现一次（`tools/ci/changed_since.sh`），workflow 里不再复制 diff 逻辑。

---

## 4. 验证阶梯

`pr_ready.py --quick` 的本质不是“少做事”，而是用失败概率换反馈速度。把它上升为全流程的分层：

| 层 | 触发 | 内容 | 典型耗时 | 预算定位 |
|---|---|---|---|---|
| **L0 quick** | agent 每轮改完 | `pr_ready.py --quick`：fmt + touched crate clippy | 30–60 s | 高频、token 敏感 |
| **L1 PR** | push / PR | quick + touched crate 单测 + 独立 workspace `--locked` 校验 + 着色器漂移（仅改 shader 时） | 5–8 min | 必需门禁 |
| **L2 nightly** | **有新提交才跑** | `features` + `user-projects` + 全 workspace 测试 | ~20 min | 低频、防漂移 |
| **L3 release** | tag / 版本号变更 | `dist-size` + Android 打包 + `pr_ready --all` | ~40 min | 发布前 |

规则：

- L0 是 agent 的**默认首轮**，写入 `ai-workflow.md`；只有开 PR 前才跑 full。
- L1 的执行集合由变更面决定，不由 job 数量决定。
- L2/L3 不进入 PR 流水线；它们的存在是为了防止“长期不跑而烂掉”，不是每个 PR 的义务。

---

## 5. P0：内循环可信化

### 5.1 自检块绑定 HEAD

**问题**：`pr-triage` 只匹配 `^\*\*Result:`。过期块、手写块、复制别人的块都能通过，导致“绿灯”不可信，人审必须重新验证一遍。

**改法**：在 `pr-triage.yml` 的 self-check 步骤中：

1. 从 PR 正文提取 `HEAD: \`([0-9a-f]{7,40})\``。
2. 与 `${{ github.event.pull_request.head.sha }}` 做**前缀**比对（`pr_ready` 输出短 sha）。
3. 缺失该行或不匹配 → 打 `needs/self-check`，评论固定文案：
   > 自检块不是当前 HEAD 的结果。请在最新提交上重跑 `python scripts/pr_ready.py`，并把完整 Markdown 块重新贴进 PR 正文。
4. 匹配 → 移除 `needs/self-check`。

`pull_request_target` 的 `synchronize` 事件已覆盖每次 push，因此每次新提交都会自动重判，无需新增触发条件。

**边界**：只校验“块属于当前 HEAD”，不校验块内容是否被篡改。后者由 CI 重跑承担；本项的目标是消灭“过期块”，不是替代 CI。

### 5.2 报错码化与替代方案

对 `check_deps.py`、`check_features.py`、`check_comment_only.py`、`check_commit_msg.py` 逐条审计失败信息，要求包含三要素：

1. **原因**：为什么禁止；
2. **替代做法**：应该改成什么；
3. **稳定错误码**：如 `DEP001`、`FEAT002`，对应 `docs/` 中一节说明。

不要求一次改完。判据：同类修复在历史上出现 ≥3 次，才升格为规则或文档；避免为了“写给 agent 看”制造新的维护负担。

### 5.3 可选的本地 hook

新增 `.githooks/pre-push`，执行 `python scripts/pr_ready.py --quick`。仅在执行 `git config core.hooksPath .githooks` 后生效；Termux 按 [`AGENTS.md`](../../AGENTS.md) 的授权降级为 `cargo fmt` + `cargo clippy`。

不加 `pre-commit`：每次提交都触发编译，打断成本高于收益。

### 5.4 P0 验收

- 用旧 HEAD 的块提交 → 被拒；
- 同一 HEAD 重复粘贴 → 通过；
- 周维度 `needs/self-check` 触发次数趋近 0。

---

## 6. P1：变更门禁与只读巡检

### 6.1 共用门禁脚本

新增 `tools/ci/changed_since.sh`：

```text
用法：changed_since.sh <base> <tip> [commits|version]
退出码：0 = 需要运行；1 = 跳过
```

- `commits`（默认）：`base..tip` 中存在 `docs/`、`*.md` 之外的改动，或 `Cargo.toml`、`assets/shaders/**` 有改动 → 运行。
- `version`：仅根 `Cargo.toml` 的 `version =` 行变化 → 运行。

`ci.yml` 现有的 doc-only 判断也改为调用该脚本，避免第三份 diff 逻辑。

### 6.2 guard 的三条硬语义

1. **基线来自上一次成功运行**：`gh run list -w nightly-automation.yml -s success -L1 --json headSha`，不写状态文件、不开额外分支、不需要 `contents: write`。
2. **失败不推进基线**：失败的下次自动重试，不会被 marker 吃掉。
3. **绝不并发**：单一 `concurrency` group + `cancel-in-progress: false`，cron 抖动不会叠加两次运行。

附加语义：`workflow_dispatch` 提供 `force` 输入，人工触发时无条件运行。

### 6.3 工作流骨架

```yaml
name: nightly-automation
on:
  schedule:
    - cron: "0 18 * * *"        # 02:00 CST
  workflow_dispatch:
    inputs:
      force:
        type: boolean
        default: false
      scope:
        type: choice
        options: [commits, version]
        default: commits

concurrency:
  group: nightly-automation
  cancel-in-progress: false

jobs:
  guard:
    runs-on: ubuntu-latest
    outputs:
      run: ${{ steps.gate.outputs.run }}
      base: ${{ steps.gate.outputs.base }}
    steps:
      - uses: actions/checkout@v4
        with:
          fetch-depth: 0
      - id: gate
        env:
          GH_TOKEN: ${{ github.token }}
          FORCE: ${{ inputs.force }}
          SCOPE: ${{ inputs.scope || 'commits' }}
        run: |
          set -euo pipefail
          if [ "${FORCE:-false}" = "true" ]; then
            echo "run=true" >> "$GITHUB_OUTPUT"
            exit 0
          fi
          tip=$(git rev-parse origin/master)
          last=$(gh run list -w nightly-automation.yml -s success -L1 \
                   --json headSha -q '.[0].headSha')
          if [ -z "$last" ] || [ "$last" = "null" ]; then
            echo "run=true" >> "$GITHUB_OUTPUT"
            exit 0
          fi
          if [ "$last" = "$tip" ]; then
            echo "run=false" >> "$GITHUB_OUTPUT"
            exit 0
          fi
          if bash tools/ci/changed_since.sh "$last" "$tip" "$SCOPE"; then
            echo "run=true" >> "$GITHUB_OUTPUT"
            echo "base=$last" >> "$GITHUB_OUTPUT"
          else
            echo "run=false" >> "$GITHUB_OUTPUT"
          fi
```

所有下游 job 一律 `needs: guard` 且 `if: needs.guard.outputs.run == 'true'`。

### 6.4 只读巡检任务

| 任务 | 判定 | 动作 | 写权限 |
|---|---|---|---|
| stale claim | `agent/working` 且 24 h 无新评论 | 评论提醒 + 换 `agent/blocked` | `issues: write` |
| ready digest | `agent/ready` 且无 `agent/working` | 汇总发 Discord（复用现有 webhook） | 无 |
| triage 滞留 | `needs/triage` 超过 72 h | 汇总提醒 | 无 |
| 周报 | 汇总 §10 指标 | 写入 job summary | 无 |

这一层刻意不写代码、不开 PR。先跑两周，用数据判断 worker 是否值得做。

### 6.5 跳过语义

`guard` 判定跳过时，下游 `skipped`，整次运行不产生成功记录。这是有意为之：跳过不算“验证过”。

同时要求在 job summary 里写一行 `skipped: no new commits since <sha>`，避免维护者误以为跑过。

---

## 7. CI 预算治理

### 7.1 结论

**不拆仓库，不拆 crate，不增加 job 数。** 需要拆细的是验证层级（§4）与文档粒度（§8）。GitHub 按 job·分钟计费，拆细只会把 checkout、toolchain、apt、cache restore 这些固定开销复制更多份；并行买到的是墙钟，不是预算。

### 7.2 具体清单

| 位置 | 现状 | 建议 | 收益 |
|---|---|---|---|
| `dist-size` | `push` 到 `master`/`dev` 每次都跑 nightly `build-std` | 改 `tags: v*` + `workflow_dispatch`，或接 §6 的 `version` 门禁 | 最大的一笔 |
| `features` / `lint` / `desktop` / `user-projects` / `dist-size` | 各自 `Setup slang + compile shaders` | 删除；`.spv` 与 reflection 已入库（51 + 21 个文件） | 5 份固定开销 |
| `shaders` | 每次代码变更都重编译 51 个 shader | 仅 `assets/shaders/**`、`crates/xtask/**` 变更或 `master` push 时运行 | 长尾浪费 |
| `features` | 每次代码变更都跑 feature 矩阵 + Windows 交叉 check | `Cargo.toml`/feature 改动时跑，其余走 L2 | 中 |
| `user-projects` | 每次代码变更都构建 `projects/*` | `crates/prism-{app,engine,scene,asset,render}/` 或 `projects/` 变更时跑 | 中 |
| rust-cache key | 五个 job 五份缓存 | `lint` 与 `desktop` 共用同一 key；`features` 保留独立 key | 中 |

### 7.3 与 `pr_ready` 的关系

`pr_ready.py` 已按 touched crates 收敛，方向正确，不拆。但 CI 的 `desktop` job 使用固定 crate 列表，两者集合不一致：

- 这本身可接受：CI 是“底线”，`pr_ready` 是“开发者回路”。
- 但必须在文档中承认二者**不等价**，避免把 PR 里的自检块当作 CI 的替代品。
- 本方案不强行统一：统一需要把 CI 改成调用 `pr_ready`，会丢掉 `--test-threads=1` 的显式控制和 job 级缓存隔离，收益不抵风险。

### 7.4 风险

删除 `lint`/`desktop` 的 `setup-slang` 后，若 PR 改了 `.slang` 却未提交 `.spv`，构建会使用旧字节码。该情况由 `shaders` job 的漂移检查兜底（`cache_shaders: "false"` 且比对 `git status`），不会漏网。需同步修正 `ci.yml` 中“`.spv` gitignored”的过期注释。

---

## 8. 文档与上下文预算

### 8.1 拆分 `ai-workflow.md`

现状 447 行，每个 agent 会话都要读，是最大的固定 token 开销。

- `ai-workflow.md`：硬规则、DoD、必须停下来问人的情况、命令速查，控制在 100 行内。
- `ai-workflow-details.md`：示例、模板、反模式逐条解释、历史案例，按需读取。
- `AGENTS.md` 保持“地图 + 红线”，细节外链；不再增长。

两文件都留在 L3，`docs/README.md` 的拓扑表同步登记。

### 8.2 按 `area/*` 的最小必读集

| area | 必读 | 明确跳过 |
|---|---|---|
| `area/render` | `GEMINI.md` / `AGENTS.md` 坐标系 + Render-graph 规则、`slang-bindless-migration.md` | Android、asset 管线、editor |
| `area/editor` | `editor-egui.md`、`project-module-architecture.md` | Vulkan pass、shader 预算 |
| `area/android` | `android-apk.md`、`android-arm64.md` | render graph、reflect |
| `area/ci` | `integrations.md`、本方案 | 所有子系统文档 |
| `area/scene` | `REFLECT.md`、`GEMINI.md` / `AGENTS.md` 场景/资产分层 | 动画、audio |

该表放进 `integrations.md`，由 `area/*` 标签驱动；agent 认领 issue 时先看标签再决定读什么。

### 8.3 回传纪律

- 测试失败只回传失败用例名与首个断言差异，不回传全量日志。
- `pr_ready.py` 的 `test_summary()` 已是正确做法，保持并推广到 CI 失败摘要。
- agent 不在 PR 里复述大段编译输出；产物放 artifact，PR 只放结论与路径。

---

## 9. 指标与验收

| 指标 | 采集方式 | 目标 |
|---|---|---|
| `needs/self-check` 周触发数 | `gh api` 统计标签事件 | 趋近 0 |
| 常见 PR 实际执行 job 数 | Actions run 的 job 列表 | ≤ 4 |
| agent 单任务往返次数中位数 | 会话记录抽样 | 逐月下降 |
| 首轮 `pr_ready` 通过率 | 会话记录抽样 | 逐月上升 |
| nightly 跳过率 | guard 输出 | 无事可做的日子应为 100% |
| stale `agent/working` 数量 | 每日巡检 | 0 |
| worker PR 首过率（M4 后） | PR + CI 结果 | 只看质量，不看数量 |

---

## 10. 里程碑与落地顺序

| 阶段 | 改动 | 文件 | 回滚 |
|---|---|---|---|
| M0 | 本文评审通过 | 本文件 | 删除文件 |
| M1 | 自检块绑定 HEAD；错误码首批 3 条 | `pr-triage.yml`、`scripts/check_*.py` | revert 单 commit |
| M2 | `changed_since.sh` + `nightly-automation.yml` + 4 个只读任务 | `tools/ci/`、`.github/workflows/` | 关闭 workflow |
| M3 | 拆分 `ai-workflow.md`；`area` 必读集 | `docs/`、`integrations.md` | revert |
| M4 | `dist-size` 降频；删除冗余 setup-slang；缓存 key 合并 | `ci.yml` | revert |
| M5 | worker（`AGENT_WORKER_ENABLED=false`） | `.github/workflows/agent-worker.yml` | 变量置 false |

M1 与 M2 可并行；M4 必须在 M2 之后，因为降频依赖 `changed_since.sh` 的口径。

---

## 11. 明确不做

| 项 | 理由 |
|---|---|
| automerge | 与“AI 产出不代表维护者已审阅”的声明语义冲突；golden frame 无 GPU 时 `skip`，在 GitHub 上显示为绿色；引擎回归多在画面、手感、帧时间，需要人看 diff |
| fuzzing 自动开 issue 的自愈回路 | 自放大回路，缺指纹去重与每日上限时会刷屏；当前没有稳定的崩溃指纹来源 |
| 7×24 常驻 agent 进程 | 上下文污染、记忆腐化、并发冲突；事件驱动 + 一次性沙箱才是正确形态 |
| 拆仓库 / 拆 crate | 跨仓同步、依赖发布、issue 分裂的成本高于收益；24 个 crate 的 UI-free / Vulkan-free 边界是资产 |
| 拆更多 CI job | 固定开销随 job 数线性增长，买到的只是墙钟 |
| worker 直推 `dev` | `dev` 直推是给人的逃生口；自动写入必须走分支 + PR + 自检块 |

---

## 12. 风险与回滚

| 风险 | 影响 | 缓解 |
|---|---|---|
| guard 基线取错，导致长期跳过 | 周期检查失效 | job summary 输出 `skipped since <sha>`；提供 `force` 手动入口 |
| 删除 setup-slang 后用旧 `.spv` 构建 | 测试通过但字节码过期 | `shaders` job 漂移检查兜底；修正过期注释 |
| `dist-size` 降频后体积回归发现变晚 | 发布前才发现超预算 | 发布流水线强制跑；每周一次定时跑 |
| 只读巡检写入误导性评论 | 噪音 | 文案固定、每类只评论一次、带 `<!-- janitor:... -->` 去重标记 |
| worker 与人类同时改同一 issue | 冲突 | 认领标签 + 二次确认 + 单 issue 单写者；违反即取消运行 |
| 标签竞态导致重复运行 | 重复 PR | `concurrency` + 分支名幂等 + open PR 检查 |

所有改动均为新增 YAML/脚本或单点修改，回滚方式统一为 revert 单个 commit；workflow 可在 Actions 页面直接 disable。

---

## 附录 A：guard 实现要点

1. `fetch-depth: 0` 必须开启，否则 `changed_since.sh` 无法比较。
2. `gh run list -w` 使用 workflow 文件名，避免重命名后失效。
3. 定时工作流只在默认分支运行；`cron` 时间以 UTC 计，注释写本地时间。
4. `force` 输入与 `schedule` 分支都要显式处理，不能依赖 `inputs` 为空时的默认值。
5. 下游所有 job 都必须带 `needs: guard` + `if:`，漏一个就会绕过门禁。

## 附录 B：标签与权限

新增标签：

| 标签 | 含义 | 创建位置 |
|---|---|---|
| `agent/fix` | 允许 worker 接手的 issue | `tools/ci/ensure_labels.sh` |
| `automation` | 由周期任务产生或处理的条目 | 同上 |

权限最小化：

| workflow | 权限 |
|---|---|
| `pr-triage` | `contents: read`、`pull-requests: write`、`issues: write` |
| `nightly-automation` | `contents: read`、`issues: write`；只读任务不需要 `contents: write` |
| `agent-worker` | `contents: write`、`pull-requests: write`、`issues: write`，且受仓库变量开关约束 |
