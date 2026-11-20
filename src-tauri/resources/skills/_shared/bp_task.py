"""建飞书任务：以机器人主人的用户身份调 lark-cli `task +create`，只用标准库。入口是 issue-record/scripts/create_task.py。

- 标题取报告第一行 `## 标题`，描述取正文（任务描述是纯文本，超长截断并注明）。
- 清单、负责人、截止天数取运行时配置里项目的 `feishu_task` 段（都可不配：不配就建在主人自己名下）。
- 去重：幂等键 = 项目 + 标题 + 当天日期，同一天同一项目重复触发不会建出两条。

退出码：0 成功；2 参数 / 报告格式不对；4 lark-cli 调用失败。
"""
import argparse
import hashlib
import json
import sys
from datetime import datetime, timedelta
from pathlib import Path

from bp_issue import parse_report, IssueError, EXIT_OK, EXIT_USAGE, EXIT_API

DESC_MAX = 3000


def _default_runner(args):
    from bp_common import lark_cli
    return lark_cli(args)


def idempotency_key(project, title, now):
    raw = f"beetle|{project}|{title}|{now:%Y-%m-%d}"
    return "beetle-" + hashlib.sha1(raw.encode("utf-8")).hexdigest()[:32]


def build_args(project, cfg, title, body, now):
    desc = body if len(body) <= DESC_MAX else body[:DESC_MAX] + "\n……（正文过长已截断，完整内容见群里的分析报告）"
    args = ["task", "+create", "--as", "user", "--summary", title, "--description", desc,
            "--idempotency-key", idempotency_key(project, title, now)]
    if cfg.get("assignee_open_id"):
        args += ["--assignee", cfg["assignee_open_id"]]
    if cfg.get("tasklist_guid"):
        args += ["--tasklist-id", cfg["tasklist_guid"]]
    days = int(cfg.get("due_days") or 0)
    if days > 0:
        args += ["--due", f"{(now + timedelta(days=days)):%Y-%m-%d}"]  # lark-cli 1.0.98 实际不认 relative:+Nd
    return args


def task_url(d):
    data = d.get("data") or {}
    task = data.get("task") or data
    return task.get("url") or task.get("app_link") or (f"任务 {task['guid']}" if task.get("guid") else None)


def main(argv=None, *, load_runtime, runner=None, out=None, err=None, now=None):
    out = out or sys.stdout
    err = err or sys.stderr
    ap = argparse.ArgumentParser(description="把 bug / 调研报告建成飞书任务")
    ap.add_argument("--project", required=True, help="项目名（客户端里登记的）")
    ap.add_argument("--file", required=True, help="报告 markdown，第一行 '## <标题>'")
    a = ap.parse_args(argv)
    try:
        projects = load_runtime().get("projects") or {}
        project = projects.get(a.project)
        if project is None:
            raise IssueError(f"未登记的项目：{a.project}（可用：{'、'.join(projects) or '无'}）", EXIT_USAGE)
        try:
            text = Path(a.file).read_text(encoding="utf-8")
        except OSError as e:
            raise IssueError(f"读不了报告文件 {a.file}：{e.strerror or e}", EXIT_USAGE)
        title, body = parse_report(text)
        args = build_args(a.project, project.get("feishu_task") or {}, title, body, now or datetime.now())
        code, o, e = (runner or _default_runner)(args)
        if code != 0:
            raise IssueError(f"lark-cli 建任务失败：{(e or o).strip()[:300]}", EXIT_API)
        try:
            d = json.loads(o)
        except json.JSONDecodeError:
            raise IssueError(f"lark-cli 返回的不是 JSON：{o.strip()[:200]}", EXIT_API)
        if d.get("ok") is False or (d.get("data") or {}).get("error"):
            raise IssueError(f"lark-cli 建任务失败：{json.dumps(d.get('error') or (d.get('data') or {}).get('error') or d, ensure_ascii=False)[:300]}", EXIT_API)
        url = task_url(d)
    except IssueError as e:
        print(f"[失败] {e}", file=err)
        return e.code
    except Exception as e:
        print(f"[失败] {type(e).__name__}: {e}", file=err)
        return EXIT_API
    print(f"已创建飞书任务：{url or '（没有返回链接）'}", file=out)
    return EXIT_OK
