"""建 issue 的公共逻辑（Gitee / GitHub），只用标准库。机器人运行时技能 issue-record 和技能中心 bug-hunt / code-review 各带一份，内容相同。
仓库解析、token 读取、带重试的 HTTP、打码也给 PR 审查（bp_pr.py）复用：bp_pr 直接 import 这里的函数，不另抄一份。

- 平台按仓库地址的 host 选择：gitee.com → Gitee，github.com → GitHub；项目配置 issue_repo 可改成别的 owner/repo。
- token 从环境变量读（默认 GITEE_TOKEN / GITHUB_TOKEN，项目配置 token_env 可改名）。Windows 上优先读用户级环境变量
  （注册表 HKCU\\Environment）的最新值：客户端和 Agent 子进程继承的是客户端启动时的环境，用户换了 token 不重启也能用上；
  没有再读当前进程的环境变量。
- token 绝不打印、不写日志、不进错误信息：所有对外文字都经过 redact()（URL 里的 access_token 也打码）。
- 去重：同标题的 open issue 已存在就追加评论（正文 + 「再次复现」时间），否则新建并打 bug 标签。
- 网络错误（含偶发的 SSL EOF）和 502/503/504 重试 3 次；建 issue 的请求不盲目重发，重试前先按标题再查一次，避免建出两条。

退出码：0 成功；2 参数 / 配置 / 报告格式不对；3 未配置 token（调用方据此降级，只写飞书文档）；4 接口或网络失败。
"""
import argparse
import http.client
import json
import os
import re
import ssl
import sys
import time
import urllib.error
import urllib.parse
import urllib.request
from dataclasses import dataclass
from datetime import datetime
from pathlib import Path

EXIT_OK = 0
EXIT_USAGE = 2
EXIT_NO_TOKEN = 3
EXIT_API = 4

LABEL = "bug"
LABEL_COLOR = "d73a4a"
USER_AGENT = "beetle-issue-record"
ATTEMPTS = 4  # 第一次 + 重试 3 次
TIMEOUT = 20
RETRY_STATUS = {502, 503, 504}

PLATFORMS = {"gitee.com": "gitee", "github.com": "github"}
DEFAULT_TOKEN_ENV = {"gitee": "GITEE_TOKEN", "github": "GITHUB_TOKEN"}
PLATFORM_LABEL = {"gitee": "Gitee", "github": "GitHub"}


class IssueError(Exception):
    def __init__(self, message, code=EXIT_API):
        super().__init__(message)
        self.code = code


class ApiError(IssueError):
    def __init__(self, status, message):
        super().__init__(message, EXIT_API)
        self.status = status


class NetworkError(IssueError):
    pass


# ───────────────────────── 打码 ─────────────────────────

_TOKEN_QS = re.compile(r"(access_token=)[^&\s\"'<>]+", re.I)
_BEARER = re.compile(r"(bearer\s+)[^\s\"'<>]+", re.I)
_URL_CRED = re.compile(r"(://)[^/@\s]+@")


def redact(text, secrets=()):
    """去掉文字里的 token：已知 token 原文、URL 里的 access_token=、Bearer 头、URL 里内嵌的账号口令。"""
    s = str(text)
    for t in secrets:
        if t:
            s = s.replace(t, "***")
    s = _TOKEN_QS.sub(r"\1***", s)
    s = _BEARER.sub(r"\1***", s)
    return _URL_CRED.sub(r"\1", s)


# ───────────────────────── 仓库解析与平台选择 ─────────────────────────

def parse_repo(url):
    """仓库地址 → (host, owner, repo)，解析不了返回 None。

    支持 git@host:owner/repo(.git)、ssh://git@host[:port]/owner/repo.git、https://[user:pass@]host/owner/repo(.git)、host/owner/repo。
    """
    s = (url or "").strip()
    if not s:
        return None
    if "://" in s:
        p = urllib.parse.urlsplit(s)
        host, path = (p.hostname or "").lower(), p.path
    else:
        m = re.match(r"^(?:[^@/\s]+@)?([^:/\s]+):(.+)$", s)  # scp 形式 git@host:owner/repo
        if m:
            host, path = m.group(1).lower(), m.group(2)
        else:
            head, _, path = s.partition("/")
            host = head.lower()
    parts = [x for x in path.strip("/").split("/") if x]
    if not host or len(parts) < 2:
        return None
    owner, repo = parts[0], parts[1].removesuffix(".git")
    if not owner or not repo:
        return None
    return host, owner, repo


def platform_of_host(host):
    return PLATFORMS.get((host or "").lower().removeprefix("www."))


def _parse_override(s):
    """issue_repo：`owner/repo`，或完整仓库地址（此时平台也按它的 host 选）。返回 (platform 或 None, owner, repo)。"""
    s = s.strip()
    parts = [x for x in s.strip("/").split("/") if x]
    if len(parts) == 2 and ":" not in s and "@" not in s:
        return None, parts[0], parts[1].removesuffix(".git")
    parsed = parse_repo(s)
    if not parsed:
        return None
    return platform_of_host(parsed[0]), parsed[1], parsed[2]


@dataclass
class Target:
    platform: str
    owner: str
    repo: str
    token_env: str

    @property
    def full(self):
        return f"{self.owner}/{self.repo}"

    @property
    def label(self):
        return f"{PLATFORM_LABEL[self.platform]} · {self.full}"


def resolve_target(project):
    """项目配置（运行时 JSON 里的一项）→ Target。仓库不是 gitee.com / github.com 时报 IssueError(退出码 2)。"""
    repo_url = project.get("repo") or ""
    parsed = parse_repo(repo_url)
    platform = platform_of_host(parsed[0]) if parsed else None
    owner, repo = (parsed[1], parsed[2]) if parsed else (None, None)
    override = (project.get("issue_repo") or "").strip()
    if override:
        o = _parse_override(override)
        if not o:
            raise IssueError(f"issue_repo 格式不对：{redact(override)}（应为 owner/repo）", EXIT_USAGE)
        platform = o[0] or platform
        owner, repo = o[1], o[2]
    if not platform or not owner:
        raise IssueError(f"仓库地址 {redact(repo_url)} 不是 gitee.com / github.com，无法建 issue", EXIT_USAGE)
    token_env = (project.get("token_env") or "").strip() or DEFAULT_TOKEN_ENV[platform]
    return Target(platform, owner, repo, token_env)


# ───────────────────────── token ─────────────────────────

def _read_user_env(name):
    """Windows 用户级环境变量（HKCU\\Environment）的当前值；其它系统或读不到返回 None。"""
    if sys.platform != "win32":
        return None
    try:
        import winreg

        with winreg.OpenKey(winreg.HKEY_CURRENT_USER, "Environment") as k:
            value, kind = winreg.QueryValueEx(k, name)
    except (OSError, ImportError):
        return None
    if not isinstance(value, str):
        return None
    if kind == getattr(winreg, "REG_EXPAND_SZ", -1):
        value = os.path.expandvars(value)
    return value.strip() or None


def read_token(name):
    """先取用户级环境变量的最新值（Windows），没有再取当前进程环境变量。都没有返回 None。"""
    return _read_user_env(name) or (os.environ.get(name) or "").strip() or None


# ───────────────────────── HTTP ─────────────────────────

def _parse_body(raw):
    if not raw:
        return None
    try:
        return json.loads(raw.decode("utf-8", "replace"))
    except ValueError:
        return None


def _error_message(body):
    if isinstance(body, dict):
        for k in ("message", "error_description", "error"):
            v = body.get(k)
            if isinstance(v, str) and v:
                return v[:200]
    return ""


class Http:
    """最小 HTTP 客户端：request() 返回 (状态码, JSON 或 None)；raw=True 时返回 (状态码, 响应文本)。

    网络错误（URLError、SSL EOF、连接被重置、超时）和 502/503/504 最多重试 3 次；retry=False 时只发一次。
    错误信息里不带 URL、不带 token。
    """

    def __init__(self, secrets=(), sleep=time.sleep, opener=None, user_agent=USER_AGENT):
        self.secrets = [s for s in secrets if s]
        self.sleep = sleep
        self.opener = opener or urllib.request.urlopen
        self.user_agent = user_agent

    def request(self, method, url, *, headers=None, form=None, json_body=None, retry=True, raw=False):
        hdrs = {"User-Agent": self.user_agent, "Accept": "application/json"}
        hdrs.update(headers or {})
        data = None
        if form is not None:
            data = urllib.parse.urlencode(form).encode("utf-8")
            hdrs["Content-Type"] = "application/x-www-form-urlencoded"
        elif json_body is not None:
            data = json.dumps(json_body, ensure_ascii=False).encode("utf-8")
            hdrs["Content-Type"] = "application/json; charset=utf-8"
        attempts = ATTEMPTS if retry else 1
        last = None
        for i in range(attempts):
            if i:
                self.sleep(i)
            req = urllib.request.Request(url, data=data, headers=hdrs, method=method)
            try:
                with self.opener(req, timeout=TIMEOUT) as resp:
                    body = resp.read()
                    if raw:
                        return resp.status, (body or b"").decode("utf-8", "replace")
                    return resp.status, _parse_body(body)
            except urllib.error.HTTPError as e:
                try:
                    body = _parse_body(e.read())
                except Exception:
                    body = None
                if e.code in RETRY_STATUS and i + 1 < attempts:
                    last = e
                    continue
                msg = _error_message(body)
                raise ApiError(e.code, redact(f"接口返回 {e.code}" + (f"：{msg}" if msg else ""), self.secrets))
            except (urllib.error.URLError, ssl.SSLError, http.client.HTTPException, ConnectionError, TimeoutError, OSError) as e:
                last = e
        reason = getattr(last, "reason", None) or last
        raise NetworkError(redact(f"网络错误（已重试 {attempts - 1} 次）：{type(last).__name__}: {reason}", self.secrets))


# ───────────────────────── 平台实现 ─────────────────────────

def _q(s):
    return urllib.parse.quote(str(s), safe="")


def _same_open(item, title):
    return (
        isinstance(item, dict)
        and (item.get("title") or "").strip() == title.strip()
        and (item.get("state") or "open") == "open"
        and "pull_request" not in item
    )


def _issue(item):
    item = item or {}
    return {"number": str(item.get("number") or ""), "url": item.get("html_url") or ""}


class GitHub:
    """GitHub REST：JSON 请求体，Authorization: Bearer。搜索有几秒索引延迟，去重同时用 bug 标签列表比对标题。"""

    API = "https://api.github.com"

    def __init__(self, target, token, http):
        self.t, self.token, self.http = target, token, http

    def _req(self, method, path, body=None, retry=True):
        headers = {
            "Authorization": f"Bearer {self.token}",
            "Accept": "application/vnd.github+json",
            "X-GitHub-Api-Version": "2022-11-28",
        }
        return self.http.request(method, self.API + path, headers=headers, json_body=body, retry=retry)[1]

    def _repo_path(self):
        return f"/repos/{_q(self.t.owner)}/{_q(self.t.repo)}"

    def whoami(self):
        return (self._req("GET", "/user") or {}).get("login")

    def find_open_issue(self, title):
        phrase = title.replace('"', " ")
        q = f'repo:{self.t.full} is:issue is:open in:title "{phrase}"'
        try:
            data = self._req("GET", "/search/issues?" + urllib.parse.urlencode({"q": q, "per_page": 50}))
            for it in (data or {}).get("items") or []:
                if _same_open(it, title):
                    return _issue(it)
        except ApiError:
            pass  # 搜索被限流 / 语法不支持时只靠列表比对
        # 列表接口刚建完也可能有几秒缓存：第一次没找到就等一下再查一次
        for attempt in range(2):
            data = self._req("GET", f"{self._repo_path()}/issues?" + urllib.parse.urlencode({"state": "open", "labels": LABEL, "per_page": 100}))
            for it in data if isinstance(data, list) else []:
                if _same_open(it, title):
                    return _issue(it)
            if attempt == 0:
                self.http.sleep(3)
        return None

    def ensure_label(self):
        """GitHub 建 issue 时不存在的标签会自动创建，不用先建。"""

    def create_issue(self, title, body):
        return _issue(self._req("POST", f"{self._repo_path()}/issues", {"title": title, "body": body, "labels": [LABEL]}, retry=False))

    def comment(self, issue, body):
        self._req("POST", f"{self._repo_path()}/issues/{_q(issue['number'])}/comments", {"body": body})

    def close_issue(self, number):
        self._req("PATCH", f"{self._repo_path()}/issues/{_q(number)}", {"state": "closed"})

    def labels_of(self, number):
        detail = self._req("GET", f"{self._repo_path()}/issues/{_q(number)}") or {}
        return [x.get("name") for x in detail.get("labels") or [] if isinstance(x, dict)]


class Gitee:
    """Gitee v5：access_token 放表单 / 查询参数；issue 编号是字母串（如 IKWMBW）。

    建 issue 是 POST /repos/{owner}/issues（路径里没有 repo，repo 放表单），labels 为逗号分隔字符串；创建返回里 labels 为空，
    所以建完 GET 详情核对，没打上就补打（补打接口要 JSON 数组体）。标签要先建，已存在返回 409 视为成功。
    """

    API = "https://gitee.com/api/v5"

    def __init__(self, target, token, http):
        self.t, self.token, self.http = target, token, http

    def _get(self, path, params=None):
        qs = urllib.parse.urlencode({**(params or {}), "access_token": self.token})
        return self.http.request("GET", f"{self.API}{path}?{qs}")[1]

    def _form(self, method, path, form, retry=True):
        return self.http.request(method, self.API + path, form={**form, "access_token": self.token}, retry=retry)[1]

    def _json(self, method, path, body):
        qs = urllib.parse.urlencode({"access_token": self.token})
        return self.http.request(method, f"{self.API}{path}?{qs}", json_body=body)[1]

    def _repo_path(self):
        return f"/repos/{_q(self.t.owner)}/{_q(self.t.repo)}"

    def whoami(self):
        return (self._get("/user") or {}).get("login")

    def find_open_issue(self, title):
        try:
            data = self._get("/search/issues", {"q": title, "repo": self.t.full, "state": "open", "per_page": 50})
            for it in data if isinstance(data, list) else []:
                if _same_open(it, title):
                    return _issue(it)
        except ApiError:
            pass
        data = self._get(f"{self._repo_path()}/issues", {"state": "open", "labels": LABEL, "per_page": 100})
        for it in data if isinstance(data, list) else []:
            if _same_open(it, title):
                return _issue(it)
        return None

    def ensure_label(self):
        try:
            self._form("POST", f"{self._repo_path()}/labels", {"name": LABEL, "color": LABEL_COLOR})
        except ApiError as e:
            if e.status != 409:  # 409 = 标签已存在
                raise

    def create_issue(self, title, body):
        issue = _issue(self._form("POST", f"/repos/{_q(self.t.owner)}/issues", {"repo": self.t.repo, "title": title, "body": body, "labels": LABEL}, retry=False))
        self._ensure_issue_label(issue)
        return issue

    def _ensure_issue_label(self, issue):
        try:
            detail = self._get(f"{self._repo_path()}/issues/{_q(issue['number'])}") or {}
            names = [x.get("name") for x in detail.get("labels") or [] if isinstance(x, dict)]
            if LABEL not in names:
                self._json("POST", f"{self._repo_path()}/issues/{_q(issue['number'])}/labels", [LABEL])
        except IssueError as e:
            print(redact(f"[提示] issue 已创建，但补打 bug 标签失败：{e}", [self.token]), file=sys.stderr)

    def comment(self, issue, body):
        self._form("POST", f"{self._repo_path()}/issues/{_q(issue['number'])}/comments", {"body": body})

    def close_issue(self, number):
        self._form("PATCH", f"/repos/{_q(self.t.owner)}/issues/{_q(number)}", {"repo": self.t.repo, "state": "closed"})

    def labels_of(self, number):
        detail = self._get(f"{self._repo_path()}/issues/{_q(number)}") or {}
        return [x.get("name") for x in detail.get("labels") or [] if isinstance(x, dict)]


PROVIDERS = {"gitee": Gitee, "github": GitHub}


def build_provider(target, token, http):
    return PROVIDERS[target.platform](target, token, http)


# ───────────────────────── 流程 ─────────────────────────

def parse_report(text):
    """报告 markdown → (标题, 正文)。第一行必须是 `## 标题`，其余是正文。"""
    text = (text or "").lstrip("﻿").strip()
    lines = text.splitlines()
    first = lines[0].strip() if lines else ""
    if not first.startswith("## ") or not first[3:].strip():
        raise IssueError("第一行必须是 '## <bug 标题>'", EXIT_USAGE)
    body = "\n".join(lines[1:]).strip()
    if not body:
        raise IssueError("报告正文为空", EXIT_USAGE)
    return first[3:].strip(), body


def comment_body(body, now):
    return f"{body}\n\n---\n再次复现：{now:%Y-%m-%d %H:%M}"


def record_issue(provider, title, body, now=None, sleep=time.sleep):
    """同标题 open issue 存在 → 追加评论；否则新建。返回 ("created" | "commented", {"number", "url"})。"""
    existing = provider.find_open_issue(title)
    if existing:
        provider.comment(existing, comment_body(body, now or datetime.now()))
        return "commented", existing
    provider.ensure_label()
    last = None
    for i in range(ATTEMPTS):
        if i:
            sleep(i)
            # 上一次可能已经建成功、只是响应没收到（如 SSL EOF）：先查再决定要不要重发
            found = provider.find_open_issue(title)
            if found:
                return "created", found
        try:
            return "created", provider.create_issue(title, body)
        except NetworkError as e:
            last = e
    raise last


def _api_hint(e, target):
    return {
        401: f"{target.token_env} 无效或已过期（接口返回 401）",
        403: f"{target.token_env} 没有权限操作仓库 {target.full}（接口返回 403）",
        404: f"仓库 {target.full} 不存在，或 {target.token_env} 无权访问（接口返回 404）",
    }.get(e.status, str(e))


def describe_plan(name, project):
    """`--plan`：只读配置、只看 token 有没有，不联网。"""
    rec = project.get("record") or {}
    parts = []
    if rec.get("issue"):
        try:
            t = resolve_target(project)
            seen = "已检测到" if read_token(t.token_env) else "未检测到"
            parts.append(f"建 issue=是（{t.label}，token 环境变量 {t.token_env}：{seen}）")
        except IssueError as e:
            parts.append(f"建 issue=是，但{e}")
    else:
        parts.append("建 issue=否")
    parts.append("写飞书文档=" + ("是" if rec.get("feishu_doc", True) else "否"))
    return f"项目 {name} 记录方式：" + "；".join(parts)


def main(argv=None, *, load_runtime, make_provider=None, out=None, err=None, now=None):
    out = out or sys.stdout
    err = err or sys.stderr
    ap = argparse.ArgumentParser(description="把 bug 报告建成 Gitee / GitHub issue；同标题的 open issue 已存在就追加评论")
    ap.add_argument("--project", required=True, help="项目名（客户端里登记的）")
    ap.add_argument("--file", help="报告 markdown，第一行 '## <标题>'")
    ap.add_argument("--plan", action="store_true", help="只打印本项目的记录方式和 token 是否已配置，不建 issue")
    a = ap.parse_args(argv)

    token = None
    target = None
    try:
        rt = load_runtime()
        projects = rt.get("projects") or {}
        project = projects.get(a.project)
        if project is None:
            raise IssueError(f"未登记的项目：{a.project}（可用：{'、'.join(projects) or '无'}）", EXIT_USAGE)
        if a.plan:
            print(describe_plan(a.project, project), file=out)
            return EXIT_OK
        if not a.file:
            raise IssueError("缺少 --file <报告 markdown>", EXIT_USAGE)
        target = resolve_target(project)
        try:
            text = Path(a.file).read_text(encoding="utf-8")
        except OSError as e:
            raise IssueError(f"读不了报告文件 {a.file}：{e.strerror or e}", EXIT_USAGE)
        title, body = parse_report(text)
        token = read_token(target.token_env)
        if not token:
            raise IssueError(
                f"未配置 {target.token_env}：请在系统环境变量里设置 {PLATFORM_LABEL[target.platform]} 私人令牌（{target.token_env}）后重试",
                EXIT_NO_TOKEN,
            )
        provider = (make_provider or build_provider)(target, token, Http(secrets=[token]))
        action, issue = record_issue(provider, title, body, now=now)
    except ApiError as e:
        print(redact(f"[失败] {_api_hint(e, target) if target else e}", [token]), file=err)
        return EXIT_API
    except IssueError as e:
        print(redact(f"[失败] {e}", [token]), file=err)
        return e.code
    except Exception as e:  # 兜底：任何意外都不能把 token 打出去
        print(redact(f"[失败] {type(e).__name__}: {e}", [token]), file=err)
        return EXIT_API
    if action == "created":
        print(f"已创建 issue：{issue['url']}", file=out)
    else:
        print(f"已在 {issue['url']} 追加评论", file=out)
    return EXIT_OK
