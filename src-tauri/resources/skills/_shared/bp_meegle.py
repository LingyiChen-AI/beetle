"""建飞书项目工单的公共逻辑：调用飞书项目 CLI（meegle），只用标准库。入口是 issue-record/scripts/create_work_item.py。

- 空间、工单类型、固定带上的字段（必填下拉等）、默认处理人和角色，都来自运行时配置里项目的 `meegle` 段
  （客户端项目页登录 meegle 后从 CLI 拉出来让用户选的），这里不猜。
- 名称取报告标题，描述取报告正文；人员类必填字段（如「报告人」）填当前登录的飞书项目用户。
- 配置里 `ai_fields` 的字段（优先级、涉及端等）由机器人按问题判断，用 `--field 字段名=选项名` 传进来，这里换成选项 id；
  没传的用空间默认值兜底，连默认值也没有就报错（退出码 2），错误信息里列出可选项。
- 去重：同空间同类型里已有同名工单就追加评论（正文 + 「再次复现」时间），否则新建。
- Windows 上 `meegle` 是 npm 的 .cmd 壳，中文和 JSON 参数经它转发不可靠，直接 `node <包>/bin/meegle.js` 调用。

退出码：0 成功；2 参数 / 配置 / 报告格式不对；3 没装或没登录飞书项目 CLI；4 接口或网络失败。
"""
import argparse
import json
import os
import shutil
import subprocess
import sys
from datetime import datetime
from pathlib import Path

from bp_issue import parse_report, IssueError, EXIT_OK, EXIT_USAGE, EXIT_API

EXIT_NO_AUTH = 3
TIMEOUT = 60
DEFAULT_HOST = "project.feishu.cn"
USER_TYPES = ("user", "multi_user")


class MeegleError(IssueError):
    pass


def meegle_cmd():
    """返回调用前缀：Windows 上 [node, meegle.js]，其它平台 [meegle]；没装返回 None。"""
    bin_ = shutil.which("meegle")
    if not bin_:
        return None
    script = Path(bin_).parent / "node_modules" / "@lark-project" / "meegle" / "bin" / "meegle.js"
    if os.name == "nt" and script.is_file():
        return [shutil.which("node") or "node", str(script)]
    return [bin_]


def _parse(out):
    i = min([x for x in (out.find("{"), out.find("[")) if x >= 0], default=-1)
    if i < 0:
        return None
    try:
        return json.loads(out[i:])
    except json.JSONDecodeError:
        return None


def _looks_unauthed(text):
    t = text.lower()
    return any(k in t for k in ("not logged", "unauthenticated", "auth login", "token expired"))


def run_cli(args):
    """执行 meegle 子命令，返回解析后的 JSON。"""
    base = meegle_cmd()
    if not base:
        raise MeegleError("没有安装飞书项目 CLI（meegle）：请在客户端「环境检测」里安装并登录", EXIT_NO_AUTH)
    try:
        p = subprocess.run([*base, *args], capture_output=True, timeout=TIMEOUT)
    except subprocess.TimeoutExpired:
        raise MeegleError(f"飞书项目 CLI 超过 {TIMEOUT} 秒没有返回", EXIT_API)
    out = (p.stdout or b"").decode("utf-8", "replace")
    err = (p.stderr or b"").decode("utf-8", "replace")
    if p.returncode != 0:
        msg = (err.strip() or out.strip())[:300]
        if _looks_unauthed(msg):
            raise MeegleError("飞书项目 CLI 未登录或登录已过期：请在客户端「环境检测」里点「去登录」", EXIT_NO_AUTH)
        raise MeegleError(f"飞书项目 CLI 执行失败：{msg or f'退出码 {p.returncode}'}", EXIT_API)
    d = _parse(out)
    if d is None:
        raise MeegleError(f"飞书项目 CLI 返回的不是 JSON：{out.strip()[:200]}", EXIT_API)
    return d


def _items(d, *keys):
    if isinstance(d, list):
        return d
    for k in keys:
        v = (d or {}).get(k)
        if isinstance(v, list):
            return v
    return []


class Client:
    """对 meegle 子命令的薄封装；`run` 可注入（单测用假的）。"""

    def __init__(self, run=run_cli):
        self.run = run

    def host(self):
        try:
            d = self.run(["auth", "status"])
        except MeegleError as e:
            if e.code == EXIT_NO_AUTH:
                raise
            return DEFAULT_HOST
        if not d.get("authenticated"):
            raise MeegleError("飞书项目 CLI 未登录或登录已过期：请在客户端「环境检测」里点「去登录」", EXIT_NO_AUTH)
        return d.get("host") or DEFAULT_HOST

    def me(self):
        d = self.run(["user", "me"])
        key = d.get("user_key")
        if not key:
            raise MeegleError("取不到当前飞书项目用户（user me 没有 user_key）", EXIT_API)
        return str(key)

    def required_fields(self, pk, wt):
        d = self.run(["workitem", "meta-create-fields", "--project-key", pk, "--work-item-type", wt])
        return _items(d, "FieldConfList", "list")

    def role_ids(self, pk, wt):
        d = self.run(["workitem", "meta-roles", "--project-key", pk, "--work-item-type", wt])
        return [r.get("role_id") for r in _items(d, "list") if r.get("role_id")]

    def find_same_name(self, pk, wt, title):
        """同名工单的 id；标题里有引号 / 反斜杠时不查（MQL 转义规则不明，宁可新建）。"""
        if any(c in title for c in "'`\\"):
            return None
        mql = f"SELECT `工作项id`, `名称` FROM `{pk}`.`{wt}` WHERE `名称` = '{title}' LIMIT 5"
        try:
            d = self.run(["workitem", "query", "--project-key", pk, "--mql", mql])
        except MeegleError as e:
            if e.code == EXIT_NO_AUTH:
                raise
            return None  # 查不了就当没有，直接建
        for rows in (d.get("data") or {}).values():
            for row in rows or []:
                for f in row.get("moql_field_list") or []:
                    if f.get("key") == "work_item_id":
                        v = (f.get("value") or {}).get("long_value")
                        if v:
                            return str(v)
        return None

    def create(self, pk, wt, fields):
        params = json.dumps({"fields": fields}, ensure_ascii=False)
        d = self.run(["workitem", "create", "--project-key", pk, "--work-item-type", wt, "--params", params])
        wid = d.get("work_item_id") or (d.get("data") or {}).get("work_item_id") or d.get("id")
        if not wid:
            raise MeegleError(f"建单没有返回工单 id：{json.dumps(d, ensure_ascii=False)[:300]}", EXIT_API)
        return str(wid)

    def comment(self, pk, wid, content):
        self.run(["comment", "add", "--project-key", pk, "--work-item-id", wid, "--content", content])


def parse_field_args(items):
    """`--field 名称=值` 列表 → {名称: 值}。"""
    out = {}
    for it in items or []:
        k, sep, v = it.partition("=")
        if not sep or not k.strip() or not v.strip():
            raise MeegleError(f"--field 格式应为 字段名=选项名：{it}", EXIT_USAGE)
        out[k.strip()] = v.strip()
    return out


def _is_multi(ty):
    return (ty or "").startswith("multi") and ty != "multi_text"


def resolve_ai_fields(cfg, given):
    """AI 判断的字段：按字段名 / key 取机器人给的值，换成选项 id；没给的用默认值；都没有的列进错误。"""
    values, problems = {}, []
    for f in cfg.get("ai_fields") or []:
        key, name = f.get("field_key"), f.get("field_name") or f.get("field_key")
        opts = f.get("options") or []
        raw = given.get(name, given.get(key))
        if raw is None:
            if f.get("fallback"):
                values[key] = json.dumps([f["fallback"]]) if _is_multi(f.get("field_type")) else f["fallback"]
            else:
                problems.append(f"{name} 没有给值（可选：{'/'.join(o['name'] for o in opts) or '任意文字'}）")
            continue
        if not opts:
            values[key] = raw
            continue
        hit = next((o for o in opts if raw.lower() in (o["name"].lower(), str(o["id"]).lower())), None)
        if not hit:
            problems.append(f"{name}={raw} 不是可选项（可选：{'/'.join(o['name'] for o in opts)}）")
            continue
        values[key] = json.dumps([hit["id"]]) if _is_multi(f.get("field_type")) else hit["id"]
    unknown = [k for k in given if not any(k in (f.get("field_name"), f.get("field_key")) for f in cfg.get("ai_fields") or [])]
    if unknown:
        problems.append(f"这些字段不需要传：{'、'.join(unknown)}")
    if problems:
        raise MeegleError("--field 有问题：" + "；".join(problems), EXIT_USAGE)
    return values


def build_fields(cfg, title, body, me_key, required, role_ids, given=None):
    """建单字段：名称 / 描述 + 配置里固定的字段 + AI 判断的字段 + 人员类必填填自己 + 处理人角色。缺配置的必填字段报错（退出码 2）。"""
    values = {"name": title, "description": body}
    for f in cfg.get("fields") or []:
        if f.get("field_key") and f.get("value") not in (None, ""):
            values[f["field_key"]] = f["value"]
    values.update(resolve_ai_fields(cfg, given or {}))
    missing = []
    for rf in required:
        key = rf.get("field_key")
        if not key or key in values:
            continue
        ty = rf.get("field_type_key") or ""
        if ty in USER_TYPES:
            values[key] = json.dumps([me_key]) if ty == "multi_user" else me_key
        else:
            missing.append(rf.get("field_name") or key)
    if missing:
        raise MeegleError(
            f"必填字段没有配置：{'、'.join(missing)}。请到客户端「项目」页重新打开这个项目的飞书项目工单配置，选好后保存",
            EXIT_USAGE,
        )
    owners = []
    assignee = (cfg.get("assignee") or {}).get("user_key")
    role = (cfg.get("role") or {}).get("role_id")
    if assignee and role:
        owners.append({"role": role, "owners": [assignee]})
    if "reporter" in role_ids and role != "reporter":
        owners.append({"role": "reporter", "owners": [me_key]})
    fields = [{"field_key": k, "field_value": v} for k, v in values.items()]
    if owners:
        fields.append({"field_key": "role_owners", "field_value": json.dumps(owners)})
    return fields


def detail_url(host, cfg, wid):
    space = cfg.get("simple_name") or cfg["project_key"]
    return f"https://{host}/{space}/{cfg.get('work_item_type') or 'issue'}/detail/{wid}"


def record_work_item(client, cfg, title, body, now=None, given=None):
    """同名工单存在 → 追加评论；否则新建。返回 ("created" | "commented", url)。"""
    pk, wt = cfg["project_key"], cfg.get("work_item_type") or "issue"
    host = client.host()
    resolve_ai_fields(cfg, given or {})  # 先校验 --field，追加评论前也别放过写错的参数
    existing = client.find_same_name(pk, wt, title)
    if existing:
        client.comment(pk, existing, f"{body}\n\n---\n再次复现：{(now or datetime.now()):%Y-%m-%d %H:%M}")
        return "commented", detail_url(host, cfg, existing)
    me_key = client.me()
    fields = build_fields(cfg, title, body, me_key, client.required_fields(pk, wt), client.role_ids(pk, wt), given)
    return "created", detail_url(host, cfg, client.create(pk, wt, fields))


def describe_plan(name, project):
    rec = project.get("record") or {}
    cfg = project.get("meegle") or {}
    if not rec.get("meegle"):
        return f"项目 {name} 没有勾选「建飞书项目工单」"
    if not cfg.get("project_key"):
        return f"项目 {name} 勾选了「建飞书项目工单」，但还没选空间"
    who = (cfg.get("assignee") or {}).get("name") or "不指定"
    role = (cfg.get("role") or {}).get("role_name") or "—"
    fields = "、".join(f"{f.get('field_name') or f.get('field_key')}={f.get('label') or f.get('value')}" for f in cfg.get("fields") or [])
    judge = "；".join(
        f"--field {f.get('field_name') or f.get('field_key')}=<{'/'.join(o['name'] for o in f.get('options') or []) or '文字'}>"
        for f in cfg.get("ai_fields") or [])
    return (f"项目 {name}：飞书项目空间 {cfg.get('space_name') or cfg['project_key']}，"
            f"类型 {cfg.get('work_item_type_name') or cfg.get('work_item_type') or 'issue'}，处理人 {who}（{role}）"
            + (f"，固定字段 {fields}" if fields else "")
            + (f"。建单时按问题判断并传：{judge}" if judge else ""))


def main(argv=None, *, load_runtime, client=None, out=None, err=None, now=None):
    out = out or sys.stdout
    err = err or sys.stderr
    ap = argparse.ArgumentParser(description="把 bug 报告建成飞书项目工单；同名工单已存在就追加评论")
    ap.add_argument("--project", required=True, help="项目名（客户端里登记的）")
    ap.add_argument("--file", help="报告 markdown，第一行 '## <标题>'")
    ap.add_argument("--plan", action="store_true", help="只打印本项目的飞书项目工单配置，不建单")
    ap.add_argument("--field", action="append", default=[], help="AI 判断的字段，格式 字段名=选项名，可重复")
    a = ap.parse_args(argv)
    try:
        rt = load_runtime()
        projects = rt.get("projects") or {}
        project = projects.get(a.project)
        if project is None:
            raise MeegleError(f"未登记的项目：{a.project}（可用：{'、'.join(projects) or '无'}）", EXIT_USAGE)
        if a.plan:
            print(describe_plan(a.project, project), file=out)
            return EXIT_OK
        cfg = project.get("meegle") or {}
        if not cfg.get("project_key"):
            raise MeegleError("这个项目还没配置飞书项目空间：请到客户端「项目」页选好空间和工单类型", EXIT_USAGE)
        if not a.file:
            raise MeegleError("缺少 --file <报告 markdown>", EXIT_USAGE)
        try:
            text = Path(a.file).read_text(encoding="utf-8")
        except OSError as e:
            raise MeegleError(f"读不了报告文件 {a.file}：{e.strerror or e}", EXIT_USAGE)
        title, body = parse_report(text)
        given = parse_field_args(a.field)
        action, url = record_work_item(client or Client(), cfg, title, body, now=now, given=given)
    except IssueError as e:
        print(f"[失败] {e}", file=err)
        return e.code
    except Exception as e:
        print(f"[失败] {type(e).__name__}: {e}", file=err)
        return EXIT_API
    if action == "created":
        print(f"已创建飞书项目工单：{url}", file=out)
    else:
        print(f"已在飞书项目工单 {url} 追加评论", file=out)
    return EXIT_OK
