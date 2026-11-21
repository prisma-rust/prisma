# Agent 契约审计：外部模板 vs 本仓库现状

本文件是一份**快照式体检报告**：把一份外部"生产级 `AGENTS.md` 模板"（零注释、验证驱动、Git 自动化熔断）逐条对照
本仓库已经固化的规则，给出三类结论——已超越模板、与现状冲突、仍是空洞——并列出待决策项。

- 审计基线：`dev` @ `76b1068`，工作树干净，`core.hooksPath=.githooks` 已安装
- 审计脚本依据：[../AGENTS.md](../AGENTS.md)、[ai-workflow.md](ai-workflow.md)、[code-style.md](code-style.md)、
  [../.github/pull_request_template.md](../.github/pull_request_template.md)、`scripts/` 实测输出
- 数据均有命令可复现，统计口径写在每节开头；本文件**不描述**未验证的推测

---

## 一、结论摘要

| 模板章节 | 本仓库状态 | 处置 |
|---|---|---|
| §1 零注释 / 清理存量注释 | **方向相反**：规范明确要求保留不变量注释 | 拒绝照抄，改走"注释预算" |
| §2 架构与编码铁律 | 分层/footgun 已用工具锁死；"零屏蔽"未达成 | 部分采纳 |
| §3 验证优先 | 强于模板（单命令门禁 + hook + CI），但缺"用户流程"与 PR 级属性测试 | 补洞，不改结构 |
| §4 PR 交付契约 | **结构冲突**：仓库模板与 pr-triage 机械校验绑定 | 只增补，不替换 |
| §5 Git 卫生与熔断 | 已有更硬规则；"禁止 automerge"无对象 | 改写为 review 熔断 |

一句话：**这份模板的"软约束"部分在本仓库多数已经被工具取代；直接落地会把已有门禁弄坏。**

---

## 二、§1 零注释：不符合，且方向相反

| 模板要求 | 本仓库规则 | 位置 |
|---|---|---|
| 禁止一切非 doc 注释 | 要求保留 `// SAFETY:`、VUID、坐标约定、二进制布局、锁顺序 | [code-style.md](code-style.md) §2 |
| 触碰文件即清理周边注释 | 注释清理必须与逻辑改动**分开**，否则 `check_comment_only.py` 会失败 | [code-style.md](code-style.md) §7 |
| 禁止 TODO/FIXME | 已达成 | 见下 |

**存量实测**（`python scripts/check_comments.py --all`，退出码 1）：

```text
comments: 12541 finding(s) in 401 file(s) (cjk=11626, separator=818, todo=0, allow=97)
```

- `cjk=11626`：代码注释仍是中文，与"注释必须英文"的规范冲突，集中在 `prism-app`、`prism-cooker`、`prism-anim` 等早期 crate
- `separator=818`：装饰性分隔线仍在（`// ----------`、`// Entity`）
- `todo=0`：`TODO/FIXME` 已清零；全仓 `crates/**/*.rs` 命中的 9 处 `XXXX` 全是测试里的垃圾字节字面量（`from_bytes(b"XXXX")`）
- `allow=97`：裸 `#[allow(...)]` 未带 `reason = "..."`

**门禁现状**：[../.github/workflows/ci.yml](../.github/workflows/ci.yml) 与 [pr_ready.py](../scripts/pr_ready.py) 只扫
**新增/修改行**（`--added-lines`），因此存量不阻断，但也意味着**存量永远清不掉**——除非引入预算机制。

**被否掉的建议**：模板配套建议"在 CI 加正则拦截 `//`"。在本仓库会灾难性误伤：`//` 是 `//!` 模块文档和
`// SAFETY:` 的载体；`#` 在 `scripts/*.py` 里是合法行首 token（本文件第二、三节引用的路径都含 `#`）。

---

## 三、§2 架构与编码铁律：一半已固化，一半是口号

### 3.1 已经比模板更强（工具锁死）

| 关注点 | 工具 | 入口 |
|---|---|---|
| 分层与依赖方向 | `check_deps.py`、`check_arch_guards.py`、`cargo tree \| grep -i egui` | [pr_ready.py](../scripts/pr_ready.py) `check_arch_guards` |
| 高危底层 API | `.clippy.toml` 的 `disallowed-types` / `disallowed-methods` | [../.clippy.toml](../.clippy.toml) |
| feature 矩阵与依赖预算 | `check_features.py`、`validate_feature_map.py` | CI `features` job |
| 产物体积 | `dist-size`（nightly build-std） | CI |

`.clippy.toml` 已封禁 `std::any::TypeId`、`std::env::current_dir`、`std::env::set_current_dir`，并给出
`#[expect(clippy::disallowed_methods, reason = "...")]` 这个**受控逃生口**。这比模板的"禁止 footgun"更可执行。

### 3.2 不符合

**(a) "零屏蔽"未达成。** 扫描口径 `#!?\[allow\(`：

- `#[allow(...)]` / `#![allow(...)]`：**112 处 / 80 文件**
- `#[expect(...)]`：**9 处 / 9 文件**
- 已归类分布的 71 处中：`dead_code=33`（22 文件）、`clippy::too_many_arguments=24`、
  `clippy::macro_metavars_in_unsafe=3`，其余零散

`#[allow(dead_code)]` 是最值得收紧的一类：它正好掩护"临时 Hack 遗留的死代码"，与"1.0 前零兼容负担"
（[versioning.md](versioning.md)）直接冲突。

**(b) "警告即硬错"只对改动的 crate 成立。**

- [pr_ready.py](../scripts/pr_ready.py) 只对 touched crates 跑 `clippy --all-targets -- -D warnings`
- CI `lint` job 只覆盖 `prism-ecs prism-gfx prism-render prism-engine prism-platform` 五个桌面 crate + `xtask`
- `pr_ready.py` 的 `SKIP_BY_DEFAULT = {"prism-app"}`：靠 `projects/template` 的 check 兜底
- `#![deny(warnings)]` 仅 5 个 crate 有：`prism-ecs`、`prism-engine`、`prism-physics2d`、`prism-platform`、`prism-render`

**(c) "只治本，禁定时器"有现成反例（产品代码，非测试）：**

| 位置 | 现状 | 性质 |
|---|---|---|
| [crates/prism-editor-mcp/src/game.rs](../crates/prism-editor-mcp/src/game.rs) | 200ms sleep + 两次 `metadata().len()` 判定截图写完 | 用时间掩盖文件生命周期边界 |
| [crates/prism-editor-mcp/src/editor_egui.rs](../crates/prism-editor-mcp/src/editor_egui.rs) | 200ms 轮询控制通道就绪 | 竞态用轮询掩盖 |
| [crates/prism-app/src/render_runner.rs](../crates/prism-app/src/render_runner.rs) | 未 present 时 50ms sleep | 合理背压 |
| [crates/prism-app/src/app/mod.rs](../crates/prism-app/src/app/mod.rs) | headless 帧等待 10ms×100 | 合理有界等待 |
| [crates/prism-asset/src/runtime/hot_reload.rs](../crates/prism-asset/src/runtime/hot_reload.rs) | 轮询线程 `thread::sleep(interval)` | 轮询是本模块的设计契约 |

`crates/**/*.rs` 中 sleep 相关命中 53 处 / 18 文件，多数集中在 `#[cfg(test)]`。

**(d) "确定性 / 热路径无界分配"没有任何门禁**，目前只是散文条款。

---

## 四、§3 验证优先：最强项，但"运行时证据"没有牙

**已固化**：`python scripts/pr_ready.py` 一条命令串起 fmt、注释、版本同步、clippy、test、shader-bindgen drift、
editor check、projects check、docs 命名、架构守卫、feature map、依赖预算，输出可直接贴 PR 的 Markdown 块
（`HEAD: <sha>` + `**Result:**`）。`.githooks/pre-push` 在本地强制同一门禁，失败即非零退出。

**不符合：**

1. **门禁不跑"用户流程"**：`pr_ready.py` 只跑单元测试，不跑 headless 驱动、截图或基准。
   `PRISMA_BENCH_REPORT` / `PRISMA_SCREENSHOT` / `prism-app::run_headless` 只作为文字要求写在
   [../AGENTS.md](../AGENTS.md) §1，**没有任何脚本校验 PR 是否真的附了这些产物**。
2. **属性测试只在 nightly**：[nightly-verify.yml](../.github/workflows/nightly-verify.yml) 的
   `proptest_invariants`（`prism-ini`/`prism-physics2d`/`prism-reflect`/`prism-scene`）不在 PR 闭环里。
3. 自愈闭环已达成：门禁失败即 `exit 1`，且 AGENTS §1 明令"never edit the check"。

---

## 五、§4 PR 交付契约：结构冲突

| 模板要求 | 仓库实际 |
|---|---|
| `## Intent & State Change` / `## Verification Artifacts` / `## Blast Radius & Risk` | `What / Why`、`Change Type`、`Verification Evidence`、`Self-check`、`AI Attribution`、`Risks & Rollback` |
| 无 AI 声明要求 | [ai-workflow.md](ai-workflow.md) §4 强制首行 AI attribution + `🤖 Agent:` 前缀 |
| 未提自检块 | [pr-triage.yml](../.github/workflows/pr-triage.yml) 机械校验 body 须含 `**Result:` 且 `HEAD: \`<sha>\`` 匹配当前 head，否则打 `needs/self-check` |

若按模板替换 PR 模板，**pr-triage 会立即失守**，并丢掉 AI 归属合规项。

模板里真正值得新增、且仓库目前完全没有机器校验的是 **Blast Radius（改动模块清单）** 与 checklist 勾选：
`Verification Evidence` 留空同样能过 triage。

---

## 六、§5 Git 卫生与熔断：两条不适用 + 一处反向缺口

- **已固化**：Conventional Commits 与分支命名（`check_commit_msg.py` 同时挂在 `commit-msg` hook 和 pr-triage）、
  原子提交、`fetch + rebase + --force-with-lease`。
- **"阻止 Automerge"不适用**：全仓搜索 `automerge|auto-merge|gh pr merge --auto` **0 命中**，仓库没有自动合并。
  熔断语义需改写为"缺少必需 approve 即不可合并"。
- **"每个改动都走 PR"与现状冲突**：[../AGENTS.md](../AGENTS.md) §7 允许 `dev` 直接原子提交推送，只有
  `dev → master` 走 PR。
- **升级触发条件无覆盖**：[../.github/CODEOWNERS](../.github/CODEOWNERS) 只覆盖 `/.github/`、`/tools/ci/`、
  `scripts/pr_ready.py`、`scripts/check_commit_msg.py`，且注明"Enforced once branch protection enables
  CODEOWNERS review"。改 RSCN / RTTIBin / 公开 API **不会**自动拉人。
- **反向缺口**：模板没有本仓库已有的一条更硬规则——[../AGENTS.md](../AGENTS.md) §7 的"破坏性命令逐条人工批准 +
  未提交工作不可恢复"。

---

## 七、已决策

1. **分支与门禁粒度**：保留 `dev` 直推。`dev` 上每次提交只跑内环 `--quick`（fmt + clippy），
   完整门禁只在 `master` 提交或开 `dev → master` PR 前跑。落点：[../AGENTS.md](../AGENTS.md) §1、
   [ai-workflow.md](ai-workflow.md) §1/§2、[../.githooks/pre-push](../.githooks/pre-push)。
2. **注释策略**：改为"触碰即清理"——改动某个文件时，顺手清掉改动区域及周边的注释垃圾（过时注释、
   复述代码、被注释掉的旧代码、装饰分隔线、中文注释改成英文）。**全仓**注释清扫仍是独立的注释 PR，
   由 `check_comment_only.py` 证明零代码变更。落点：[../AGENTS.md](../AGENTS.md) §8、
   [code-style.md](code-style.md) §7。

仍然待决策：

1. 是否为 `check_comments.py --all` 引入存量预算（当前存量 12541 条，只做新增行门禁等于永不收敛）？
2. 是否把 `#[allow(dead_code)]` 的 33 处转成 `#[expect(..., reason)]` 或直接删代码？

其余可执行项（按性价比排序）：

1. 给 `check_comments.py --all` 建基线文件，`cjk` / `separator` 只减不增
2. 33 处 `#[allow(dead_code)]` 转 `#[expect(..., reason)]` 或直接删代码；门禁增加"新增 `#[allow(` 必须带 reason"
3. pr-triage 校验 `## Verification Evidence` 非空且含命令或日志路径
4. CODEOWNERS 补齐 save format / 公开 API / `Cargo.toml` 路径，用 review 代替不存在的 automerge
5. `game.rs` 的截图等待换成进程退出 + 文件稳定事件；`editor_egui.rs` 换成控制端口就绪信号
6. PR 级补 `proptest_invariants`（目标 crate 已在 nightly 验证过可跑）
