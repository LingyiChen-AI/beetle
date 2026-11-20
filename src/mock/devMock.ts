// 仅开发模式：浏览器里直接打开 vite（没有 Tauri 后端）时，api.ts 把命令和事件转到这里。
// 数据照定稿原型 docs/mockups/option-2-final.html 的示例；r0 是一条「运行中」的记录，每 3 秒推一个步骤。
// URL hash 可带 theme=dark|light|system、accent=green|blue|…（只影响 get_appearance 的初始值）。
import type {
  Appearance,
  Bot,
  BotState,
  BotView,
  CatalogEntry,
  EnvItem,
  Group,
  ProjectsView,
  RunDetail,
  RunRecord,
  SkillInfo,
  SkillTarget,
  StepRecord,
} from "../api";

type Listener = (e: { payload: unknown }) => void;
const listeners = new Map<string, Set<Listener>>();

export function mockListen(event: string, cb: Listener): () => void {
  if (!listeners.has(event)) listeners.set(event, new Set());
  listeners.get(event)!.add(cb);
  return () => listeners.get(event)?.delete(cb);
}
function emit(event: string, payload: unknown) {
  listeners.get(event)?.forEach((cb) => cb({ payload }));
}

// ───────────── 示例数据 ─────────────
const today = new Date();
today.setHours(0, 0, 0, 0);
/** 今天的 HH:MM:SS → 毫秒时间戳 */
const at = (t: string, dayOffset = 0) => {
  const [h, m, s = "0"] = t.split(":");
  return today.getTime() + dayOffset * 86_400_000 + ((+h * 60 + +m) * 60 + +s) * 1000;
};

const G: Record<string, Group> = {
  g1: { chat_id: "oc_8f2a61c0d93e4b7a5c1e", name: "示例项目群", slug: "bug-patrol-d93e4b7a", projects: ["demo-service"], doc_url: "https://example.feishu.cn/docx/demo1", trigger: "all" },
  g2: { chat_id: "oc_31d7e5b2a0c94f86e2d0", name: "测试群", slug: "bug-patrol-c94f86e2", projects: ["demo-service"], doc_url: "https://example.feishu.cn/docx/demo2", trigger: "all" },
  g3: { chat_id: "oc_c05b9e7f12a34d68b9a4", name: "研发群", slug: "bug-patrol-a34d68b9", projects: ["demo-service", "demo-worker"], doc_url: null, trigger: "owner" },
  g4: { chat_id: "oc_77ae0c3d5b21f9e04c86", name: "产品需求评审群", slug: "bug-patrol-21f9e04c", projects: ["demo-service"], doc_url: "https://example.feishu.cn/docx/demo3", trigger: "all" },
};

const bots: Bot[] = [
  {
    id: "b1", name: "BUG纠察队", app_id: "cli_a7f31c9e2b88d00c", engine: "claude", start_with_client: true,
    timeout_minutes: 30, concurrency: 2, template: "bug-hunt",
    skills: ["db-query", "issue-record", "feishu-doc-record", "chat-context", "code-sync"], extra_deny: [], auto_onboard: true,
    groups: [G.g1, G.g2, G.g3],
  },
  {
    id: "b2", name: "需求答疑助手", app_id: "cli_a6e02d14f7c9a01b", engine: "codex", start_with_client: false,
    timeout_minutes: 20, concurrency: 1, template: "qa", skills: ["feishu-doc-record", "code-sync"], extra_deny: [],
    auto_onboard: false, groups: [G.g4],
  },
  {
    id: "b3", name: "代码审查", app_id: "cli_a5c19e08b3d7f42e", engine: "claude", start_with_client: false,
    timeout_minutes: 30, concurrency: 2, template: "code-review", skills: ["pr-review", "code-sync"], extra_deny: [],
    auto_onboard: false, groups: [G.g1], trigger_mode: "pr_link",
  },
];
const states: Record<string, BotState> = { b1: { kind: "running" }, b2: { kind: "stopped" }, b3: { kind: "stopped" } };

const projects: ProjectsView = {
  projects: {
    "demo-service": { repo: "gitee.com/example/demo-service.git", branch_glob: "release/*", rules_file: "AGENTS.md", description: "示例配置后台（Go 后端 + Vue3 前端）", dbs: ["demo_config", "demo_test"] },
    "demo-worker": { repo: "gitee.com/example/demo-worker.git", branch_glob: "release/*", rules_file: "AGENTS.md", description: "示例任务处理客户端与云端", dbs: ["demo_worker"] },
  },
  dbs: {
    demo_config: {
      kind: "mysql", description: "配置后台主库（店铺、规则配置）", access: "direct",
      host: "127.0.0.1", port: 3306, user: "readonly", database: "demo_config", ssh: null, timeout_secs: 5, max_rows: 200,
    },
    demo_test: { kind: "mysql", description: "测试环境业务库，经自己的查询技能审批后查", access: "skill", skill: "my-sql", database: "demo_test", note: "库名写 demo_test；结果在话题里回复" },
    demo_worker: {
      kind: "postgres", description: "任务处理云端库", access: "direct",
      host: "10.0.3.12", port: 5432, user: "readonly", database: "demo_worker",
      ssh: { host: "jump.example.com", port: 22, user: "root", key_path: "C:/Users/me/.ssh/id_ed25519" }, timeout_secs: 8, max_rows: 200,
    },
  },
  default_projects: ["demo-service"],
};

const skills: SkillInfo[] = [
  { name: "db-query", display_name: "查库（只读）", description: "按项目关联的数据源只读查库：直连的用 beetle --db-query，技能方式的调用指定技能", requires: [], builtin: true },
  { name: "feishu-doc-record", display_name: "飞书文档记录", description: "把修复建议追加到群的记录文档", requires: ["lark_cli"], builtin: true },
  { name: "issue-record", display_name: "建 issue", description: "把 bug 分析建成项目仓库（Gitee / GitHub）的 issue，同标题已存在就追加评论", requires: ["python"], builtin: true },
  { name: "chat-context", display_name: "读取私聊上下文", description: "读群主点名的私聊（需群主授权）", requires: ["lark_cli"], builtin: true },
  { name: "code-sync", display_name: "代码同步", description: "拉取项目最新 release 分支到代码缓存", requires: ["git"], builtin: true },
  { name: "pr-review", display_name: "PR 审查", description: "拉取 Gitee / GitHub PR 的元数据和 diff、读写每个 PR 上一轮的审查结果（增量复审）", requires: ["python"], builtin: true },
];

// 技能中心：查 bug 已装到 Claude Code（旧版本，有更新），没装到 Codex。
const catalog: CatalogEntry[] = [
  {
    name: "bug-hunt",
    display_name: "查 bug",
    description: "在自己的 Claude Code / Codex 会话里说「帮我查这个 bug」：读群聊上下文、同步最新 release 代码、按关联的数据源只读查库、给出根因与修复建议，可追加到飞书文档",
    version: "1.0.0",
    requires: ["python", "node", "lark_cli", "git"],
    installed: { claude: "0.9.0", codex: null },
    update_available: { claude: true, codex: false },
  },
  {
    name: "code-review",
    display_name: "代码审查",
    description: "在自己的 Claude Code / Codex 会话里说「review 一下这个 PR <链接>」：拉取 PR、同步代码到 PR 最新提交、用 ocr 挑文件和规则逐文件审查，对照上一轮结果增量复审",
    version: "1.0.0",
    requires: ["python", "git", "ocr"],
    installed: { claude: null, codex: null },
    update_available: { claude: false, codex: false },
  },
];

const env: EnvItem[] = [
  { id: "claude", label: "Claude Code CLI", group: "runtime", status: "ok", version: "2.1.42", detail: "已安装，已登录（Pro 账号）" },
  { id: "node", label: "Node.js ≥ 18", group: "runtime", status: "ok", version: "v20.18.0", detail: "C:/Program Files/nodejs/node.exe" },
  { id: "python", label: "Python 3.10+", group: "runtime", status: "error", detail: "PATH 中没有 python.exe，建 issue、代码审查与文档技能依赖它", action: { kind: "install", label: "一键安装（winget）" } },
  { id: "git", label: "Git", group: "runtime", status: "ok", version: "2.47.1", detail: "gitee.com 凭据有效（Git Credential Manager）" },
  { id: "ocr", label: "open-code-review（ocr）", group: "runtime", status: "ok", version: "0.4.2", detail: "代码审查用它挑选可审文件、取按语言的审查规则" },
  { id: "lark_cli", label: "lark-cli", group: "feishu", status: "ok", version: "1.4.2", detail: "已安装" },
  { id: "lark_login", label: "lark-cli 登录", group: "feishu", status: "error", detail: "用户身份未登录，读群聊、写文档都会失败", action: { kind: "login", label: "扫码登录" } },
  { id: "net_feishu", label: "网络 · open.feishu.cn", group: "network", status: "ok", version: "38 ms", detail: "直连可达，长连接正常" },
  { id: "net_anthropic", label: "网络 · Anthropic API", group: "network", status: "ok", version: "412 ms", detail: "经代理 http://127.0.0.1:7890 可达" },
  { id: "system_proxy", label: "系统代理变量", group: "network", status: "warn", version: "HTTPS_PROXY", detail: "检测到系统级 HTTPS_PROXY，会让飞书长连接走代理导致掉线", action: { kind: "fix", label: "查看说明" } },
];

// 技能中心详情：bug-hunt 的示例文件
/** 多行文本（每行末尾补换行）。 */
const L = (...lines: string[]) => lines.join("\n") + "\n";
const CATALOG_FILES: Record<string, string | null> = {
  "_shared/bp_common.py": L(
    "# 公共工具：读取 beetle.runtime.json",
    "import json, os",
    "",
    "",
    "def load_runtime(path: str) -> dict:",
    '    """读运行时配置，缺失时返回空字典"""',
    "    if not os.path.exists(path):",
    "        return {}",
    '    with open(path, encoding="utf-8") as f:',
    "        return json.load(f)",
  ),
  "scripts/sql_query.py": L(
    "import sys",
    "from _shared.bp_common import load_runtime",
    "",
    "MAX_ROWS = 100  # 生产查询一律限制行数",
    "",
    'if __name__ == "__main__":',
    "    cfg = load_runtime(sys.argv[1])",
    '    print(cfg.get("requests_dir", ""))',
  ),
  "scripts/sync_code.py": L('def sync(repo: str, branch_glob: str = "release/*") -> None:', "    # 取最新 release 分支", "    pass"),
  "catalog.json": L(
    "{",
    '  "name": "bug-hunt",',
    '  "display_name": "查 bug",',
    '  "version": "1.0.0",',
    '  "requires": ["python", "node", "lark_cli", "git"],',
    '  "config": "beetle"',
    "}",
  ),
  "SKILL.md": L(
    "---",
    "name: bug-hunt",
    "description: 查 bug：读群聊上下文、同步最新 release 代码、只读查生产，给出根因与修复建议",
    "---",
    "",
    "# 查 bug",
    "",
    "在自己的 **Claude Code / Codex** 会话里说「帮我查这个 bug」即可使用。",
    "",
    "## 流程",
    "",
    "1. 读取群聊上下文（`lark-cli`）",
    "2. 同步项目最新 `release/*` 分支",
    "3. 按项目关联的数据源只读查库（db-query）",
    "4. 输出根因（文件:行号 + 数据证据）和修复建议",
    "",
    "> 生产查询一律加 LIMIT，不做全表扫描。",
    "",
    "| 脚本 | 作用 |",
    "| --- | --- |",
    "| scripts/sql_query.py | 提交只读查询 |",
    "| scripts/sync_code.py | 同步代码 |",
    "",
    "```python",
    "from _shared.bp_common import load_runtime",
    'cfg = load_runtime("beetle.runtime.json")',
    "```",
  ),
  "assets/icon.png": null,
};

// ───────────── 运行记录与步骤 ─────────────
type RawStep = [string, string, string, string, string];

const KIND_BY_TITLE: [RegExp, string][] = [
  [/^收到/, "received"], [/^准备/, "prepare"], [/^启动/, "launch"], [/^读群聊/, "readchat"], [/^同步/, "synccode"],
  [/^查库/, "sql"], [/^写记录/, "writedoc"], [/^回复/, "reply"],
];
const toMs = (d: string) => (!d ? null : d.includes("m") ? (() => { const [m, s] = d.replace("s", "").split("m"); return (+m * 60 + +s) * 1000; })() : Math.round(parseFloat(d) * 1000));
function toSteps(runId: string, raw: RawStep[], dayOffset = 0): StepRecord[] {
  return raw.map(([t, k, title, detail, dur], i) => ({
    run_id: runId,
    seq: i + 1,
    at: at(t, dayOffset),
    kind: k === "err" ? "error" : (KIND_BY_TITLE.find(([re]) => re.test(title))?.[1] ?? (k === "sql" ? "sql" : "tool")),
    title,
    detail,
    duration_ms: toMs(dur),
  }));
}

const RAW: Record<string, RawStep[]> = {
  r1: [
    ["14:02:03", "ok", "收到 @ 消息", "李四：@BUG纠察队 订单状态不刷新", ""],
    ["14:02:04", "ok", "准备工作目录", "groups/oc_8f2a61c0d93e4b7a5c1e · 生成 CLAUDE.md（查 bug 模板）", "0.6s"],
    ["14:02:05", "ok", "启动 Claude Code", "claude -p · 引擎 2.1.42 · pid 18244", "1.1s"],
    ["14:02:09", "ok", "读群聊", "最近 50 条，命中上下文 9 条，含 2 张截图", "4.2s"],
    ["14:02:31", "ok", "同步代码", "demo-service release/2026.11.18 → 3f9a2c1（缓存命中，增量 12 个提交）", "21.7s"],
    ["14:03:12", "sql", "查库 #1", "demo_config · job_task WHERE shop_id=1001 ORDER BY id DESC LIMIT 5 → 5 行", "8.3s"],
    ["14:03:40", "sql", "查库 #2", "demo_test · order_tag WHERE batch_id=88213 LIMIT 20 → 20 行", "6.1s"],
    ["14:04:22", "sql", "查库 #3", "demo_config · rule WHERE shop_id=1001 LIMIT 50 → 7 行", "5.4s"],
    ["14:05:30", "sql", "查库 #4", "demo_test · order_tag 统计（按 batch_id 收窄）LIMIT 10 → 1 行", "9.8s"],
    ["14:06:51", "ok", "写记录文档", "追加到「示例项目群 · 问题记录」第 23 条", "3.2s"],
    ["14:08:15", "ok", "回复群里", "已发送 · 消息 om_x100b4e2f9c", "0.8s"],
  ],
  r2: [
    ["13:20:41", "ok", "收到 @ 消息", "王五：@BUG纠察队 导出报表为空", ""],
    ["13:20:42", "ok", "准备工作目录", "groups/oc_31d7e5b2a0c94f86e2d0 · 生成 CLAUDE.md", "0.5s"],
    ["13:20:43", "err", "启动 Claude Code", "spawn claude: write EPIPE · 进程 0.4 秒后退出（exit code 1）", "0.4s"],
    ["13:20:44", "err", "运行失败", "stderr：Error: Invalid API key · Please run /login", ""],
    ["13:20:44", "ok", "回复群里", "已发送失败通知", "0.7s"],
  ],
  r3: [
    ["11:47:10", "ok", "收到 @ 消息", "钱七：@BUG纠察队 工单分类配置保存提示「名称重复」", ""],
    ["11:47:11", "ok", "准备工作目录", "groups/oc_8f2a61c0d93e4b7a5c1e", "0.5s"],
    ["11:47:12", "ok", "启动 Claude Code", "claude -p · pid 9932", "1.0s"],
    ["11:47:16", "ok", "读群聊", "最近 50 条，命中上下文 4 条", "3.9s"],
    ["11:47:24", "ok", "同步代码", "demo-service release/2026.11.18 → 3f9a2c1（无更新）", "2.4s"],
    ["11:48:02", "sql", "查库 #1", "demo_config · ticket_category WHERE shop_id=5108 LIMIT 50 → 31 行", "4.7s"],
    ["11:48:40", "sql", "查库 #2", "demo_config · ticket_category WHERE deleted=1 AND shop_id=5108 LIMIT 20 → 2 行", "4.1s"],
    ["11:50:21", "ok", "写记录文档", "追加到「示例项目群 · 问题记录」第 22 条", "2.9s"],
    ["11:50:50", "ok", "回复群里", "已发送", "0.6s"],
  ],
  r4: [
    ["10:05:02", "ok", "收到 @ 消息", "孙八：@BUG纠察队 历史数据回刷任务卡在 0%", ""],
    ["10:05:03", "ok", "准备工作目录", "groups/oc_8f2a61c0d93e4b7a5c1e", "0.5s"],
    ["10:05:04", "ok", "启动 Claude Code", "claude -p · pid 6120", "1.2s"],
    ["10:05:09", "ok", "读群聊", "最近 50 条，命中上下文 6 条", "4.6s"],
    ["10:05:40", "ok", "同步代码", "demo-service release/2026.11.18 → 3f9a2c1", "28.9s"],
    ["10:07:12", "sql", "查库 #1", "demo_config · backfill_job ORDER BY id DESC LIMIT 5 → 5 行", "7.2s"],
    ["10:09:55", "sql", "查库 #2", "demo_test · 调用技能 my-sql，等待查询结果…", "4m12s"],
    ["10:16:30", "sql", "查库 #3", "demo_test · backfill_progress LIMIT 20 → 0 行", "6.0s"],
    ["10:35:02", "err", "超时终止", "超过运行超时 30 分钟，已结束进程树（pid 6120）", ""],
    ["10:35:03", "ok", "回复群里", "已发送超时通知", "0.6s"],
  ],
};
const LIVE: RawStep[] = [
  ["14:35:08", "ok", "收到 @ 消息", "赵六：@BUG纠察队 规则保存后测试环境不生效", ""],
  ["14:35:09", "ok", "准备工作目录", "groups/oc_8f2a61c0d93e4b7a5c1e · 生成 CLAUDE.md", "0.5s"],
  ["14:35:10", "ok", "启动 Claude Code", "claude -p · 引擎 2.1.42 · pid 21508", "1.0s"],
  ["14:35:15", "ok", "读群聊", "最近 50 条，命中上下文 7 条，含 1 张截图", "4.4s"],
  ["14:35:22", "ok", "同步代码", "demo-service release/2026.11.18 → 3f9a2c1（无更新）", "2.6s"],
  ["14:36:01", "sql", "查库 #1", "demo_config · rule WHERE id=40217 LIMIT 1 → 1 行", "5.2s"],
  ["14:36:37", "sql", "查库 #2", "demo_config · rule_version WHERE rule_id=40217 LIMIT 10 → 3 行", "4.8s"],
  ["14:37:15", "sql", "查库 #3", "demo_test · cache_refresh_log ORDER BY id DESC LIMIT 20", ""],
];

function rec(id: string, time: string, g: Group, who: string, text: string, status: RunRecord["status"], durS: number | null, sql: number, extra: Partial<RunRecord> = {}): RunRecord {
  const started = at(time);
  return {
    id, bot_id: "b1", chat_id: g.chat_id, group_name: g.name, trigger_message_id: `om_${id}`, sender_name: who, text,
    engine: "claude", engine_version: "2.1.42", session_id: null, status, started_at: started,
    finished_at: durS == null ? null : started + durS * 1000, final_reply: null, error: null, sql_count: sql, reply_message_id: null,
    ...extra,
  };
}

/** 运行中那条：开始于 2 分 31 秒前（原型里的示例时长）。 */
const R0_START = Date.now() - 151_000;
const runs: RunRecord[] = [
  { ...rec("r0", "14:35:08", G.g1, "赵六", "规则保存后测试环境不生效", "running", null, 2), started_at: R0_START },
  rec("r1", "14:02:03", G.g1, "李四", "订单状态不刷新", "replied", 372, 4, {
    final_reply:
      "结论：状态更新本身成功了，是列表读的从库还没同步，看起来像「没刷新」。\n证据：demo_config 中 1001 最近一次状态更新 14:01:37 完成 128 条；demo_test 同批订单状态已写入，从库延迟约 1–3 秒。\n代码：demo-service release/2026.11.18 · OrderListService.List() 走 ReadOnly 数据源。\n建议：更新完成后的那次刷新改读主库，或前端完成提示后延迟 2 秒再拉列表。\n@李四 详细过程已写到 示例项目群 · 问题记录 #23",
  }),
  rec("r2", "13:20:41", G.g2, "王五", "导出报表为空", "failed", 3, 0, { error: "Claude 进程启动后立即退出（EPIPE）" }),
  rec("r3", "11:47:10", G.g1, "钱七", "工单分类配置保存提示「名称重复」", "replied", 220, 2, {
    final_reply: "结论：同名分类之前被软删除过，唯一索引没排除 deleted=1 的行，所以提示重复。\n建议：唯一性校验加上 deleted=0 条件；临时可先把旧分类改名。\n详细过程已写到 示例项目群 · 问题记录 #22",
  }),
  rec("r4", "10:05:02", G.g1, "孙八", "历史数据回刷任务卡在 0%", "timeout", 1800, 3, {
    error: "超过运行超时 30 分钟，已终止 Claude 进程",
    final_reply: "查了 30 分钟还没出结论，先停下来了。目前已知：回刷任务 job#5521 状态是 pending，进度表里没有记录，像是没被调度起来。\n@孙八 可以补充一下任务是什么时候提交的吗？我再接着查。",
  }),
  rec("r5", "09:40:00", G.g2, "周九", "统计数和明细对不上", "replied", 178, 3, { final_reply: "结论：已定位原因，详见记录文档。\n建议：按记录文档里的修复步骤处理。" }),
  rec("r6", "09:12:00", G.g1, "李四", "新店铺接入后看板没数据", "replied", 245, 2, { final_reply: "结论：已定位原因，详见记录文档。\n建议：按记录文档里的修复步骤处理。" }),
  { ...rec("r7", "18:44:00", G.g3, "张三", "demo-worker 售后工单重复领取", "replied", 301, 2, { final_reply: "结论：判重查询走了从库，延迟导致重复领取。" }), started_at: at("18:44:00", -1), finished_at: at("18:49:01", -1) },
  // 代码审查机器人：群里贴 PR 链接触发，final_reply 是解析后的审查结果（运行日志按卡片样式预览）
  rec("r8", "13:05:00", G.g1, "张三", "https://gitee.com/example/demo-service/pulls/29", "replied", 412, 0, {
    bot_id: "b3",
    final_reply: JSON.stringify({
      repo: "demo-service",
      pr: 29,
      title: "feat: 售后工单自动领取",
      conclusion: "整体思路没问题，但领取接口有并发重复领取的风险，判重查询还走着从库，建议修完再合并。",
      findings: [
        { severity: "high", status: "new", text: "ClaimService.Claim() 先查后改没有加锁，两台机器同时领取会领到同一单 service/claim.go:88" },
        { severity: "medium", status: "carried", text: "判重查询走 ReadOnly 数据源，写后立读可能读不到刚领取的记录 repo/ticket.go:40" },
        { severity: "low", status: "resolved", text: "领取失败没有打日志 service/claim.go:120" },
      ],
      additions: 1822,
      deletions: 109,
      files: 23,
      author: "张三",
      url: "https://gitee.com/example/demo-service/pulls/29",
    }),
  }),
];

function genericSteps(r: RunRecord): StepRecord[] {
  const t0 = r.started_at;
  const mk = (sec: number, kind: string, title: string, detail: string, dur: number | null): Omit<StepRecord, "seq"> => ({ run_id: r.id, at: t0 + sec * 1000, kind, title, detail, duration_ms: dur });
  return [
    mk(0, "received", "收到 @ 消息", `${r.sender_name}：@BUG纠察队 ${r.text}`, null),
    mk(1, "prepare", "准备工作目录", `groups/${r.chat_id}`, 500),
    mk(2, "launch", "启动 Claude Code", "claude -p", 1000),
    mk(7, "readchat", "读群聊", "最近 50 条", 4000),
    mk(25, "synccode", "同步代码", "demo-service release/2026.11.18", 3100),
    mk(70, "sql", `查库 ×${r.sql_count}`, "demo_config 直连只读", 18400),
    mk(150, "writedoc", "写记录文档", "已追加", 2800),
    mk(170, "reply", "回复群里", "已发送", 600),
  ].map((s, i) => ({ ...s, seq: i + 1 }));
}

const liveSteps = toSteps("r0", LIVE).map((s) => ({ ...s, at: s.at - at("14:35:08") + R0_START }));
let liveShown = 6;
const stepsOf = (id: string): StepRecord[] => {
  if (id === "r0") return liveSteps.slice(0, liveShown);
  if (RAW[id]) return toSteps(id, RAW[id]);
  const r = runs.find((x) => x.id === id);
  return r ? genericSteps(r) : [];
};

// 运行中那条每 3 秒推一个步骤（run-step），查库时同步 sql_count（run-updated）
let liveTimer: ReturnType<typeof setInterval> | null = null;
function ensureLive() {
  if (liveTimer) return;
  liveTimer = setInterval(() => {
    const r0 = runs[0];
    if (r0.status !== "running" || liveShown >= liveSteps.length) return;
    const step = { ...liveSteps[liveShown], at: Date.now() };
    liveSteps[liveShown] = step;
    liveShown++;
    emit("run-step", { step });
    if (step.kind === "sql") {
      r0.sql_count++;
      emit("run-updated", { run: { ...r0 } });
    }
  }, 3000);
}


// ───────────── 模板 / 技能设置 / 旧版 ─────────────
const templateText: Record<string, string> = {
  "bug-hunt": `# 工作说明：查 bug（BUG纠察队）

你在飞书群「{{GROUP_NAME}}」里被 @ 了。按下面的顺序查，查完把结论回复到群里。

## 1. 读群聊
- 读取 @ 消息前后 50 条，找出：现象、店铺 / 账号、发生时间、截图里的报错。

## 2. 同步代码
{{PROJECTS_BLOCK}}
- 先读仓库根目录的 AGENTS.md，按里面的约定定位代码。

## 3. 查生产库（只读）
- 可用库：{{DBS_LIST}}；按 db-query 技能的规则查，每条查询必须带 LIMIT。

## 4. 写记录文档
- 追加到 {{DOC_URL}}：现象 / 根因 / 证据 / 修复建议 / 涉及文件。

## 5. 回复群里
- 先给一句话结论，再列证据和建议；不确定就直说不确定。`,
  qa: `# 工作说明：需求答疑

有人在群里问「现在这个功能是怎么做的」时：
1. 读群聊确认问的是哪个功能。
2. 同步负责项目最新 release 分支，定位相关代码：
{{PROJECTS_BLOCK}}
3. 用产品能听懂的话回答：入口在哪、规则是什么、有什么限制。`,
  "code-review": `# 代码审查机器人（{{GROUP_NAME}}）

群里有人贴 Gitee / GitHub 的 PR 链接时触发：
1. pr_fetch.py 拉 PR 元数据和 diff。
2. sync_code.py --pr 同步代码到 PR 最新提交（独立只读 worktree）：
{{PROJECTS_BLOCK}}
3. pr_review_state.py get 读上一轮结果，标注已处理 / 未处理 / 新发现。
4. 只报真实问题，按严重度给出 文件:行号 和理由，最终输出结构化 JSON。`,
};
const templateModified = new Set<string>(["bug-hunt"]);
const skillSettings: Record<string, Record<string, unknown>> = {};
const SKILL_FIELDS: Record<string, unknown[]> = {};
/** 本机已装的技能（list_local_skills） */
const localSkills = [
  { name: "my-sql", location: "claude", description: "经公司审批流程只读查生产库" },
  { name: "my-sql", location: "codex", description: "经公司审批流程只读查生产库" },
  { name: "mongo-reader", location: "claude", description: "只读查 MongoDB 集合" },
  { name: "bug-hunt", location: "claude", description: "查 bug" },
];

/** 模拟安装：逐行推 env-install-log，最后发 env-install-done 并把该项改成正常。 */
function simulateFix(id: string) {
  const lines =
    id === "lark_login"
      ? ["已打开终端窗口：lark-cli auth login，请在窗口里完成登录，这里会自动检测", "还在等待登录完成…（已等 15 秒）", "已检测到登录完成"]
      : ["winget install Python.Python.3.12", "正在下载 25.4 MB", "正在安装…", "已安装 Python 3.12.7"];
  lines.forEach((line, i) => setTimeout(() => emit("env-install-log", { id, line }), 400 * (i + 1)));
  setTimeout(() => {
    const it = env.find((e) => e.id === id);
    if (it) {
      it.status = "ok";
      it.version = id === "python" ? "3.12.7" : it.version;
      it.detail = id === "python" ? "winget 安装完成，已加入 PATH" : "已用用户身份登录（张三）";
      delete it.action;
    }
    emit("env-install-done", { id, ok: true, message: id === "lark_login" ? "登录完成" : lines[lines.length - 1] });
  }, 400 * (lines.length + 1));
}

// ───────────── 命令 ─────────────
let appearance: Appearance = (() => {
  const q = new URLSearchParams(window.location.hash.replace(/^#/, ""));
  return {
    mode: (["dark", "light", "system"].includes(q.get("theme") ?? "") ? q.get("theme") : "dark") as Appearance["mode"],
    accent: (["vermilion", "green", "blue", "indigo", "purple", "orange", "teal"].includes(q.get("accent") ?? "") ? q.get("accent") : "vermilion") as Appearance["accent"],
  };
})();

const delay = <T>(v: T, ms = 60) => new Promise<T>((r) => setTimeout(() => r(v), ms));
const runtimeSettings: { proxy: string | null; code_cache_dir: string | null; log_retention_days: number } = {
  proxy: null,
  code_cache_dir: null,
  log_retention_days: 30,
};
const isToday = (ms: number) => ms >= today.getTime();

// ───────────── 数据统计示例：按固定种子生成近 60 天的运行，统计口径与后端 commands_stats.rs 一致 ─────────────
interface MockStatRun {
  bot_id: string;
  chat_id: string;
  group_name: string;
  sender: string;
  engine: "claude" | "codex";
  status: "replied" | "failed" | "timeout" | "cancelled";
  started_at: number;
  dur: number;
  sql: number;
  error: string | null;
}
const MOCK_SENDERS = ["张三", "李四", "王五", "赵六", "钱七", "孙八", "周九", "吴十", "郑一", "冯二", "陈三", "褚四"];
const MOCK_ERRORS = [
  "lark-cli 未登录：用户身份已过期，请重新扫码登录",
  "查库失败：连接 demo_config 超时（5 秒）",
  "git fetch 失败：无法连接 gitee.com（检查代理设置）",
  "Agent 进程异常退出（exit code 1）",
  "找不到项目 demo-worker 的 release 分支",
];
let mockStatRuns: MockStatRun[] | null = null;
function statRuns(): MockStatRun[] {
  if (mockStatRuns) return mockStatRuns;
  let seed = 20261119;
  const rnd = () => ((seed = (seed * 1103515245 + 12345) % 2147483648) / 2147483648);
  const out: MockStatRun[] = [];
  const groups = Object.values(G);
  for (let d = 59; d >= 0; d--) {
    const n = Math.round(6 + rnd() * 10 + (d % 7 === 1 || d % 7 === 2 ? -4 : 0));
    for (let i = 0; i < n; i++) {
      // 上午 10 点和下午 3 点是高峰
      const hour = Math.min(23, Math.max(0, Math.round((rnd() < 0.55 ? 10 : 15) + (rnd() - 0.5) * 6)));
      const g = groups[Math.floor(rnd() * groups.length)];
      const bot = bots.find((b) => b.groups.some((x) => x.chat_id === g.chat_id)) ?? bots[0];
      const x = rnd();
      const status: MockStatRun["status"] = x < 0.8 ? "replied" : x < 0.9 ? "failed" : x < 0.96 ? "timeout" : "cancelled";
      out.push({
        bot_id: bot.id,
        chat_id: g.chat_id,
        group_name: g.name,
        sender: MOCK_SENDERS[Math.floor(Math.pow(rnd(), 1.6) * MOCK_SENDERS.length)],
        engine: bot.engine,
        status,
        started_at: at(`${hour}:${Math.floor(rnd() * 60)}`, -d),
        dur: status === "timeout" ? bot.timeout_minutes * 60_000 : Math.round(60_000 + rnd() * 420_000),
        sql: status === "cancelled" ? 0 : Math.floor(rnd() * 6),
        error: status === "failed" ? MOCK_ERRORS[Math.floor(Math.pow(rnd(), 1.4) * MOCK_ERRORS.length)] : null,
      });
    }
  }
  mockStatRuns = out;
  return out;
}

function mockTotals(rs: MockStatRun[]) {
  const c = (st: MockStatRun["status"]) => rs.filter((r) => r.status === st).length;
  const replied = c("replied");
  const failed = c("failed");
  const timeout = c("timeout");
  const settled = replied + failed + timeout;
  const durs = rs.filter((r) => r.status === "replied").map((r) => r.dur).sort((x, y) => x - y);
  return {
    runs: rs.length,
    replied,
    failed,
    timeout,
    cancelled: c("cancelled"),
    active: 0,
    settled,
    success_rate: settled ? replied / settled : 0,
    avg_duration_ms: durs.length ? Math.round(durs.reduce((s, x) => s + x, 0) / durs.length) : 0,
    p90_duration_ms: durs.length ? durs[Math.max(0, Math.ceil(durs.length * 0.9) - 1)] : 0,
    sql_count: rs.reduce((s, r) => s + r.sql, 0),
  };
}

function mockStats(range: "today" | "7d" | "30d", botId: string | null) {
  const days = range === "today" ? 1 : range === "7d" ? 7 : 30;
  const now = Date.now();
  const start = today.getTime() - (days - 1) * 86_400_000;
  const shift = days * 86_400_000;
  const all = statRuns().filter((r) => !botId || r.bot_id === botId);
  const cur = all.filter((r) => r.started_at >= start && r.started_at <= now);
  const prev = all.filter((r) => r.started_at >= start - shift && r.started_at < now - shift);
  const top = (key: (r: MockStatRun) => [string, string]) => {
    const m = new Map<string, { key: string; name: string; runs: number }>();
    cur.forEach((r) => {
      const [k, name] = key(r);
      const e = m.get(k) ?? { key: k, name, runs: 0 };
      e.runs++;
      m.set(k, e);
    });
    return [...m.values()].sort((x, y) => y.runs - x.runs || x.name.localeCompare(y.name)).slice(0, 10);
  };
  const reasons = new Map<string, number>();
  cur.filter((r) => r.status === "failed" || r.status === "timeout").forEach((r) => {
    const k = r.error ?? "运行超时";
    reasons.set(k, (reasons.get(k) ?? 0) + 1);
  });
  const hourly = Array.from({ length: 24 }, () => 0);
  cur.forEach((r) => hourly[new Date(r.started_at).getHours()]++);
  return {
    range,
    bot_id: botId,
    start_ms: start,
    end_ms: now,
    totals: mockTotals(cur),
    previous: mockTotals(prev),
    daily: Array.from({ length: days }, (_, i) => {
      const s0 = start + i * 86_400_000;
      const rs = cur.filter((r) => r.started_at >= s0 && r.started_at < s0 + 86_400_000);
      const d = new Date(s0);
      return {
        day: `${String(d.getMonth() + 1).padStart(2, "0")}-${String(d.getDate()).padStart(2, "0")}`,
        start_ms: s0,
        runs: rs.length,
        replied: rs.filter((r) => r.status === "replied").length,
        failed: rs.filter((r) => r.status === "failed").length,
        timeout: rs.filter((r) => r.status === "timeout").length,
      };
    }),
    by_bot: bots
      .map((b) => {
        const t = mockTotals(cur.filter((r) => r.bot_id === b.id));
        return { bot_id: b.id, name: b.name, runs: t.runs, replied: t.replied, success_rate: t.success_rate, avg_duration_ms: t.avg_duration_ms, sql_count: t.sql_count };
      })
      .filter((b) => b.runs > 0)
      .sort((x, y) => y.runs - x.runs),
    by_group: top((r) => [r.chat_id, r.group_name]),
    by_sender: top((r) => [`name:${r.sender}`, r.sender]),
    by_engine: (["claude", "codex"] as const)
      .map((e) => ({ engine: e, runs: cur.filter((r) => r.engine === e).length }))
      .filter((e) => e.runs > 0)
      .sort((x, y) => y.runs - x.runs),
    failure_reasons: [...reasons.entries()]
      .map(([reason, count]) => ({ reason, count }))
      .sort((x, y) => y.count - x.count)
      .slice(0, 5),
    hourly,
  };
}

function botViews(): BotView[] {
  return bots.map((b) => ({
    bot: structuredClone(b),
    state: states[b.id] ?? { kind: "stopped" },
    today_runs: runs.filter((r) => r.bot_id === b.id && isToday(r.started_at)).length,
    alive: ["running", "connecting"].includes((states[b.id] ?? { kind: "stopped" }).kind),
  }));
}

function setState(id: string, s: BotState) {
  states[id] = s;
  emit("bot-state", { bot_id: id, state: s });
}

export async function mockInvoke(cmd: string, a: Record<string, unknown>): Promise<unknown> {
  switch (cmd) {
    case "list_bots":
      return delay(new URLSearchParams(window.location.hash.replace(/^#/, "")).has("empty") ? [] : botViews());
    case "start_bot": {
      const id = a.id as string;
      setState(id, { kind: "connecting" });
      setTimeout(() => setState(id, { kind: "running" }), 1400);
      return delay(undefined);
    }
    case "stop_bot": {
      const id = a.id as string;
      setTimeout(() => setState(id, { kind: "stopped" }), 900);
      return delay(undefined);
    }
    case "set_start_with_client": {
      const b = bots.find((x) => x.id === a.id);
      if (b) b.start_with_client = a.on as boolean;
      return delay(undefined);
    }
    case "save_bot": {
      const bot = a.bot as Bot;
      if (!bot.id) bot.id = `b${bots.length + 1}`;
      const i = bots.findIndex((x) => x.id === bot.id);
      if (i >= 0) bots[i] = { ...bot, groups: bots[i].groups };
      else bots.push(bot);
      return delay(structuredClone(bots.find((x) => x.id === bot.id)));
    }
    case "delete_bot": {
      const i = bots.findIndex((x) => x.id === a.id);
      if (i >= 0) bots.splice(i, 1);
      return delay(undefined);
    }
    case "save_group": {
      const b = bots.find((x) => x.id === a.botId)!;
      const g = a.group as Group;
      const i = b.groups.findIndex((x) => x.chat_id === g.chat_id);
      if (i >= 0) b.groups[i] = g;
      else b.groups.push(g);
      return delay(undefined);
    }
    case "remove_group": {
      const b = bots.find((x) => x.id === a.botId)!;
      b.groups = b.groups.filter((g) => g.chat_id !== a.chatId);
      return delay(undefined);
    }
    case "list_projects":
      return delay(structuredClone(projects));
    case "list_skills":
      return delay(skills);
    case "list_runs": {
      ensureLive();
      const list = runs.filter((r) => !a.botId || r.bot_id === a.botId).slice(0, a.limit as number);
      return delay(list.map((r) => ({ ...r })));
    }
    case "get_run": {
      const r = runs.find((x) => x.id === a.id);
      if (!r) throw "运行记录不存在";
      return delay<RunDetail>({ run: { ...r }, steps: stepsOf(r.id) });
    }
    case "cancel_run": {
      const r = runs.find((x) => x.id === a.id);
      if (r) {
        r.status = "cancelled";
        r.finished_at = Date.now();
        r.error = "手动取消，已结束 Claude 进程树";
        emit("run-updated", { run: { ...r } });
      }
      return delay(undefined);
    }
    case "retry_run":
      return delay(undefined);
    case "get_raw_log": {
      const id = a.runId as string;
      const st = stepsOf(id);
      const stdout = [
        `{"type":"system","subtype":"init","session_id":"5f0c9a2e-${id}","model":"claude-sonnet","cwd":"groups/oc_8f2a61c0d93e4b7a5c1e"}`,
        ...st.map((s) => JSON.stringify({ type: "assistant", message: { content: [{ type: "tool_use", name: s.kind, input: { title: s.title, detail: s.detail } }] } })),
      ].join("\n");
      const stderr = id === "r2" ? "Error: Invalid API key · Please run /login\n" : "";
      return delay({ stdout, stderr }, 150);
    }
    case "list_templates":
      return delay(Object.keys(templateText).map((name) => ({ name, builtin: ["bug-hunt", "qa", "code-review"].includes(name), modified: templateModified.has(name) })));
    case "get_template": {
      const t = templateText[a.name as string];
      if (t == null) throw `模板不存在：${a.name}`;
      return delay(t);
    }
    case "save_template": {
      const content = a.content as string;
      const bad = content.match(/\{\{([A-Z_]+)\}\}/g)?.find((v) => !["GROUP_NAME", "CHAT_ID", "PROJECTS_BLOCK", "DBS_LIST", "DOC_URL", "SKILLS_DIR", "PYTHON", "NODE"].includes(v.slice(2, -2)));
      if (bad) throw `模板里有未替换的变量：${bad}`;
      templateText[a.name as string] = content;
      templateModified.add(a.name as string);
      return delay(undefined);
    }
    case "reset_template":
      templateModified.delete(a.name as string);
      return delay(undefined);
    case "get_skill_settings":
      return delay({ fields: SKILL_FIELDS[a.name as string] ?? [], value: skillSettings[a.name as string] ?? {} });
    case "save_skill_settings":
      skillSettings[a.name as string] = a.value as Record<string, unknown>;
      return delay(undefined);
    case "catalog_list":
      return delay(structuredClone(catalog));
    case "catalog_install": {
      const c = catalog.find((x) => x.name === a.name);
      if (!c) throw `技能中心里没有技能 ${a.name}`;
      const targets = a.targets as SkillTarget[];
      if (targets.length === 0) throw "请至少选择一个安装位置（Claude Code 或 Codex）";
      for (const t of targets) {
        c.installed[t] = c.version;
        c.update_available[t] = false;
      }
      return delay(undefined, 600);
    }
    case "catalog_uninstall": {
      const c = catalog.find((x) => x.name === a.name);
      if (c) {
        c.installed[a.target as SkillTarget] = null;
        c.update_available[a.target as SkillTarget] = false;
      }
      return delay(undefined);
    }
    case "catalog_files": {
      const paths = Object.keys(CATALOG_FILES);
      const dirs = [...new Set(paths.filter((x) => x.includes("/")).map((x) => x.split("/")[0]))].sort();
      const out: { path: string; name: string; size: number; is_dir: boolean; depth: number }[] = [];
      for (const d of dirs) {
        out.push({ path: d, name: d, size: 0, is_dir: true, depth: 0 });
        paths
          .filter((x) => x.startsWith(`${d}/`))
          .sort()
          .forEach((x) => out.push({ path: x, name: x.slice(d.length + 1), size: (CATALOG_FILES[x] ?? "xxxxxxxx").length, is_dir: false, depth: 1 }));
      }
      paths
        .filter((x) => !x.includes("/"))
        .sort((x, y) => x.toLowerCase().localeCompare(y.toLowerCase()))
        .forEach((x) => out.push({ path: x, name: x, size: (CATALOG_FILES[x] ?? "").length, is_dir: false, depth: 0 }));
      return delay(out);
    }
    case "catalog_read_file": {
      const path = String(a.path);
      if (!(path in CATALOG_FILES)) throw `文件不存在：${path}`;
      const content = CATALOG_FILES[path];
      return delay({ path, size: content?.length ?? 2048, content, binary: content === null, too_large: false });
    }
    case "catalog_refresh_config":
      return delay(catalog.reduce((n, c) => n + (c.installed.claude ? 1 : 0) + (c.installed.codex ? 1 : 0), 0));
    case "import_local_skill": {
      const name = String(a.dir).split(/[\\/]/).pop() || "local-skill";
      const s = { name, display_name: name, description: "从本地目录导入的技能", requires: [], builtin: false };
      skills.push(s);
      return delay(s);
    }
    case "meegle_status":
      return delay({ installed: true, logged_in: true, host: "project.feishu.cn", user: "张三", error: null });
    case "meegle_spaces":
      return delay([
        { project_key: "pk_demo", simple_name: "demo", name: "示例空间" },
        { project_key: "pk_ops", simple_name: "ops", name: "运维空间" },
      ]);
    case "meegle_types":
      return delay([
        { type_key: "issue", name: "缺陷" },
        { type_key: "story", name: "需求" },
      ]);
    case "meegle_form":
      return delay({
        fields: [
          { field_key: "name", field_name: "名称", field_type: "text", options: [], default: null, auto: "自动填：bug 标题" },
          { field_key: "description", field_name: "缺陷描述", field_type: "multi_text", options: [], default: null, auto: "自动填：bug 分析报告" },
          { field_key: "priority", field_name: "优先级", field_type: "select", options: [{ id: "0", name: "P0" }, { id: "1", name: "P1" }, { id: "2", name: "P2" }], default: "2", auto: null },
          { field_key: "issue_reporter", field_name: "报告人", field_type: "multi_user", options: [], default: null, auto: "自动填：当前登录的飞书项目用户" },
          { field_key: "bug_classification", field_name: "涉及端", field_type: "select", options: [{ id: "c1", name: "客户端" }, { id: "c2", name: "服务端" }], default: null, auto: null },
          { field_key: "template", field_name: "缺陷类型", field_type: "work_item_template", options: [{ id: "t1", name: "用户反馈缺陷" }, { id: "t2", name: "测试反馈缺陷" }], default: null, auto: null },
        ],
        roles: [
          { role_id: "reporter", role_name: "报告人" },
          { role_id: "r_fix", role_name: "缺陷处理人" },
        ],
        suggested_role: "r_fix",
      });
    case "meegle_users":
      return delay(String(a.query).trim() ? [{ user_key: "u_1", name: "李四", email: "lisi@example.com" }] : []);
    case "lark_tasklists":
      return delay([
        { guid: "tl_1", name: "研发待办" },
        { guid: "tl_2", name: "调研" },
      ]);
    case "lark_users":
      return delay(String(a.query).trim() ? [{ open_id: "ou_1", name: "王五", department: "研发部" }] : []);
    case "check_issue_token": {
      const p = a.project as ProjectsView["projects"][string];
      const github = p.repo.includes("github.com");
      const env = p.token_env || (github ? "GITHUB_TOKEN" : "GITEE_TOKEN");
      const repo = p.issue_repo || p.repo.replace(/\.git$/, "").split(/[/:]/).slice(-2).join("/");
      return delay({ platform: github ? "github" : "gitee", platform_label: github ? "GitHub" : "Gitee", repo, token_env: env, detected: !github, user: github ? null : "mock-user", error: github ? `未检测到环境变量 ${env}` : null });
    }
    case "save_project":
      projects.projects[a.name as string] = a.project as ProjectsView["projects"][string];
      return delay(undefined);
    case "remove_project":
      delete projects.projects[a.name as string];
      return delay(undefined);
    case "save_datasource":
      projects.dbs[a.alias as string] = a.ds as ProjectsView["dbs"][string];
      return delay(undefined, 300);
    case "delete_datasource": {
      const alias = a.alias as string;
      const used = Object.keys(projects.projects).filter((n) => projects.projects[n].dbs.includes(alias));
      if (used.length) return Promise.reject(`数据源 ${alias} 被项目 ${used.join("、")} 使用，先在项目里取消关联`);
      delete projects.dbs[alias];
      return delay(undefined, 200);
    }
    case "test_datasource": {
      const ds = a.ds as ProjectsView["dbs"][string];
      if (ds.access !== "direct") return Promise.reject("技能方式的数据源由 Agent 调用技能查询，不能在这里测试");
      if (ds.host.startsWith("10.") && !ds.ssh) return new Promise((_, rej) => setTimeout(() => rej(`连接 ${ds.host}:${ds.port} 超时（${ds.timeout_secs} 秒）`), 900));
      return delay(`连接成功，耗时 ${ds.ssh ? 412 : 23} ms`, ds.ssh ? 1200 : 500);
    }
    case "list_local_skills":
      return delay(structuredClone(localSkills), 200);
    case "set_default_projects":
      projects.default_projects = a.names as string[];
      return delay(undefined);
    case "env_fix":
      simulateFix(a.id as string);
      return delay(undefined);
    case "export_config":
      return delay(undefined, 300);
    case "preview_import":
      return delay({ projects: ["demo-service", "demo-worker"], dbs: ["demo_config", "demo_worker"], templates: ["qa"], skills: [], bots: [{ name: "需求答疑助手", app_id: "cli_a6e02d14f7c9a01b" }] }, 300);
    case "apply_import":
      return delay(undefined, 300);
    case "detect_legacy":
      return delay({
        found: true,
        app_id: "cli_a7f31c9e2b88d00c",
        groups: [
          { chat_id: G.g1.chat_id, name: G.g1.name, doc_url: G.g1.doc_url },
          { chat_id: G.g2.chat_id, name: G.g2.name, doc_url: G.g2.doc_url },
        ],
        projects: ["demo-service", "demo-worker"],
        bridge_running: true,
        task_enabled: true,
      }, 300);
    case "migrate_legacy":
      return delay({ bot_id: "b1", warnings: a.stopBridge ? ["禁用计划任务需要管理员权限，请手动在「任务计划程序」里禁用 lark-channel-bridge"] : [] }, 600);
    case "feishu_cancel_create_app":
      return delay(undefined);
    case "get_autostart":
      return delay(true);
    case "get_runtime_settings":
      return delay({
        ...runtimeSettings,
        default_code_cache_dir: "C:/Users/me/AppData/Local/Beetle/code",
        min_retention_days: 1,
        max_retention_days: 3650,
      });
    case "set_runtime_settings":
      Object.assign(runtimeSettings, a.settings as object);
      return delay(undefined);
    case "set_autostart":
      return delay(undefined);
    case "update_status":
    case "check_update":
      return delay({ current: "0.1.0", available: "0.2.0", notes: "数据源独立、代码审查改用 ocr", checked_at: Date.now(), error: null }, 300);
    case "install_update":
      return delay(undefined, 1500);
    case "get_appearance":
      return delay(appearance, 0);
    case "set_appearance":
      appearance = a.appearance as Appearance;
      return delay(undefined, 0);
    case "env_check":
      return delay(structuredClone(env));
    case "env_requirements": {
      // 与后端 envcheck::requirements 同一规则（基础项 + 所选引擎 + 启用技能的 requires）
      const base = ["node", "python", "git", "lark_cli", "lark_login", "net_feishu"];
      return delay({
        base,
        engines: ["claude", "codex"],
        bots: Object.fromEntries(
          bots.map((b): [string, string[]] => [b.id, [b.engine, ...base, ...(b.skills.includes("pr-review") || b.template === "code-review" ? ["ocr"] : [])]]),
        ),
      });
    }
    case "stats_overview":
      return delay(mockStats((a.range as "today" | "7d" | "30d") ?? "7d", (a.botId as string | null) ?? null), 200);
    case "bot_stats": {
      const mine = runs.filter((r) => r.bot_id === a.botId && isToday(r.started_at));
      const done = mine.filter((r) => r.status !== "running" && r.status !== "queued");
      const replied = mine.filter((r) => r.status === "replied");
      return delay({
        today_runs: mine.length,
        today_replied: replied.length,
        success_rate: done.length ? replied.length / done.length : 0,
        avg_duration_ms: replied.length ? replied.reduce((s, r) => s + ((r.finished_at ?? r.started_at) - r.started_at), 0) / replied.length : 0,
      });
    }
    default:
      throw `开发 mock 没有实现命令 ${cmd}`;
  }
}

/** 扫码创建飞书应用：0.3 秒后给授权页地址，4 秒后「创建成功」。 */
export function mockCreateFeishuApp(onUrl: (url: string) => void): Promise<{ appId: string; botName: string | null }> {
  setTimeout(() => onUrl("https://open.feishu.cn/page/launcher?from=beetle-demo"), 300);
  return new Promise((resolve) => setTimeout(() => resolve({ appId: "cli_a8c41e07d2f5b913", botName: "线上问题助手" }), 4000));
}
