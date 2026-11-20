"""PR 审查的公共逻辑（Gitee / GitHub），只用标准库。机器人运行时技能 pr-review 和技能中心 code-review 各带一份，内容相同。

- 拉 PR 元数据、改动文件清单和 diff：Gitee GET /api/v5/repos/{o}/{r}/pulls/{n} 与 /pulls/{n}/files；
  GitHub GET /repos/{o}/{r}/pulls/{n}、/pulls/{n}/files（分页），diff 用 Accept: application/vnd.github.diff 取整段，取不到再按文件拼。
- 仓库解析、token 读取（Windows 优先读用户级环境变量）、带重试的 HTTP、打码都直接用 bp_issue.py 里的，不另抄一份。
- token 可选：公开仓库不带 token 也能读；PR 所在仓库和运行时配置里某个项目一致时，用该项目配置的 token 环境变量名。
- 有 --out 时 PR 描述（标题、分支、正文）另写成 <key>.body.md（输出 JSON 的 body_file），给 ocr delegate 的 -B 当需求背景。
- 大 diff 按文件整段截断（不把一个文件的 diff 截成半截），被省略的文件列在 omitted_files 里；lock 文件、生成代码、压缩产物、二进制不进 diff。
- 审查结果可带 coverage（ocr 覆盖率，可缺省），save 时校验后原样保存。
- 每轮审查结果按 PR 存成 <review_state_dir>/<平台>/<owner>/<repo>/<编号>.json（owner / repo 小写），和客户端（review_card.rs）同一个位置、同一个格式。
- token 绝不打印：所有对外文字都经过 bp_issue.redact()。

退出码：0 成功；2 参数不对（不是 PR 链接等）；3 读不到 PR 且没配 token（多半是私有仓库）；4 接口或网络失败。
"""
import json
import os
import re
import sys
import urllib.parse
from dataclasses import dataclass
from datetime import datetime
from pathlib import Path

import bp_issue as gi

EXIT_OK = 0
EXIT_USAGE = 2
EXIT_NO_TOKEN = 3
EXIT_API = 4

USER_AGENT = "beetle-pr-review"
DEFAULT_MAX_DIFF_CHARS = 200_000
MAX_FILE_PAGES = 30  # GitHub 每页 100 个文件，最多 3000 个
NO_PREVIOUS = "无上一轮审查记录"

_PR_URL = re.compile(
    r"^https?://(?:www\.)?(gitee\.com|github\.com)/([A-Za-z0-9_.\-]+)/([A-Za-z0-9_.\-]+)/(pulls|pull)/([0-9]{1,9})(?:[/?#].*)?$",
    re.I,
)


class PrError(Exception):
    def __init__(self, message, code=EXIT_API):
        super().__init__(message)
        self.code = code


@dataclass
class PrRef:
    platform: str
    owner: str
    repo: str
    number: int

    @property
    def full(self):
        return f"{self.owner}/{self.repo}"

    @property
    def url(self):
        if self.platform == "gitee":
            return f"https://gitee.com/{self.owner}/{self.repo}/pulls/{self.number}"
        return f"https://github.com/{self.owner}/{self.repo}/pull/{self.number}"


def parse_pr_url(url):
    """PR 链接 → PrRef。Gitee 必须是 /pulls/，GitHub 必须是 /pull/；带查询串、锚点、末尾斜杠、子页面都认。"""
    m = _PR_URL.match((url or "").strip())
    if not m:
        raise PrError(f"不是 Gitee / GitHub 的 PR 链接：{gi.redact(url)}", EXIT_USAGE)
    host, owner, repo, kind, number = m.groups()
    platform = gi.platform_of_host(host)
    if (platform == "gitee") != (kind.lower() == "pulls") or owner in (".", "..") or repo in (".", ".."):
        raise PrError(f"不是 Gitee / GitHub 的 PR 链接：{gi.redact(url)}", EXIT_USAGE)
    n = int(number)
    if n <= 0:
        raise PrError(f"PR 编号不合法：{number}", EXIT_USAGE)
    return PrRef(platform, owner, repo, n)


def match_project(rt, pr):
    """在运行时配置的 projects 里找仓库和 PR 一致的项目（平台 + owner/repo，不区分大小写）。返回 (项目名, 项目配置) 或 (None, None)。"""
    for name, p in (rt.get("projects") or {}).items():
        parsed = gi.parse_repo((p or {}).get("repo") or "")
        if not parsed:
            continue
        host, owner, repo = parsed
        if gi.platform_of_host(host) == pr.platform and owner.lower() == pr.owner.lower() and repo.lower() == pr.repo.lower():
            return name, p
    return None, None


def token_env_for(pr, project):
    return ((project or {}).get("token_env") or "").strip() or gi.DEFAULT_TOKEN_ENV[pr.platform]


# ───────────────────────── 平台接口 ─────────────────────────

def _q(s):
    return urllib.parse.quote(str(s), safe="")


class GitHubPr:
    API = "https://api.github.com"

    def __init__(self, pr, token, http):
        self.pr, self.token, self.http = pr, token, http

    def _headers(self, accept="application/vnd.github+json"):
        h = {"Accept": accept, "X-GitHub-Api-Version": "2022-11-28"}
        if self.token:
            h["Authorization"] = f"Bearer {self.token}"
        return h

    def _path(self):
        return f"/repos/{_q(self.pr.owner)}/{_q(self.pr.repo)}/pulls/{self.pr.number}"

    def detail(self):
        return self.http.request("GET", self.API + self._path(), headers=self._headers())[1] or {}

    def files(self):
        out = []
        for page in range(1, MAX_FILE_PAGES + 1):
            qs = urllib.parse.urlencode({"per_page": 100, "page": page})
            data = self.http.request("GET", f"{self.API}{self._path()}/files?{qs}", headers=self._headers())[1]
            items = data if isinstance(data, list) else []
            out.extend(items)
            if len(items) < 100:
                break
        return out

    def diff(self):
        """整段 diff；PR 太大时 GitHub 返回 406，这时返回 None，改按文件拼。"""
        try:
            return self.http.request("GET", self.API + self._path(), headers=self._headers("application/vnd.github.diff"), raw=True)[1]
        except gi.ApiError as e:
            if e.status in (406, 422):
                return None
            raise


class GiteePr:
    API = "https://gitee.com/api/v5"

    def __init__(self, pr, token, http):
        self.pr, self.token, self.http = pr, token, http

    def _get(self, path):
        qs = f"?{urllib.parse.urlencode({'access_token': self.token})}" if self.token else ""
        return self.http.request("GET", f"{self.API}{path}{qs}")[1]

    def _path(self):
        return f"/repos/{_q(self.pr.owner)}/{_q(self.pr.repo)}/pulls/{self.pr.number}"

    def detail(self):
        return self._get(self._path()) or {}

    def files(self):
        data = self._get(self._path() + "/files")
        return data if isinstance(data, list) else []

    def diff(self):
        """Gitee 的文件接口里已经带了每个文件的 diff，按文件拼。"""
        return None


CLIENTS = {"gitee": GiteePr, "github": GitHubPr}


def build_client(pr, token, http):
    return CLIENTS[pr.platform](pr, token, http)


# ───────────────────────── 整理元数据与 diff ─────────────────────────

_SKIP_NAMES = {
    "package-lock.json", "pnpm-lock.yaml", "yarn.lock", "npm-shrinkwrap.json", "cargo.lock", "go.sum",
    "poetry.lock", "pipfile.lock", "composer.lock", "gemfile.lock", "podfile.lock", "uv.lock", "bun.lockb",
}
# 只跳过几乎不会放手写代码的目录；build/ target/ out/ 之类可能是源码包名，不跳
_SKIP_DIRS = ("node_modules/", "vendor/", "dist/", "__pycache__/", ".next/")
_BINARY_EXT = {
    ".png", ".jpg", ".jpeg", ".gif", ".bmp", ".ico", ".webp", ".svgz", ".psd", ".pdf", ".zip", ".gz", ".tgz", ".7z", ".rar",
    ".jar", ".war", ".class", ".exe", ".dll", ".so", ".dylib", ".a", ".o", ".bin", ".woff", ".woff2", ".ttf", ".otf", ".eot",
    ".mp3", ".mp4", ".mov", ".avi", ".wav", ".xlsx", ".xls", ".docx", ".doc", ".pptx",
}


def skip_reason(path):
    """不需要审的文件：返回原因；要审返回 None。"""
    p = (path or "").replace("\\", "/")
    low = p.lower()
    name = low.rsplit("/", 1)[-1]
    if name in _SKIP_NAMES:
        return "lock 文件"
    if any(low.startswith(d) or f"/{d}" in low for d in _SKIP_DIRS):
        return "依赖或构建产物目录"
    if re.search(r"\.min\.(js|css)$", name) or name.endswith(".map"):
        return "压缩过的前端产物"
    if re.search(r"(_pb2\.py|\.pb\.go|\.generated\.\w+|_generated\.\w+)$", name):
        return "生成的代码"
    if os.path.splitext(name)[1] in _BINARY_EXT:
        return "二进制文件"
    return None


def _int(v):
    try:
        return int(str(v).strip() or 0)
    except (TypeError, ValueError):
        return 0


def normalize_file(platform, f):
    """平台返回的单个文件 → {path, previous_path, status, additions, deletions, patch}。patch 可能为 None（二进制 / 太大）。"""
    f = f or {}
    path = f.get("filename") or ""
    previous = f.get("previous_filename")
    status = f.get("status") or "modified"
    patch = f.get("patch")
    if platform == "gitee" and isinstance(patch, dict):
        path = path or patch.get("new_path") or ""
        if patch.get("old_path") and patch.get("old_path") != path:
            previous = patch.get("old_path")
        if patch.get("new_file"):
            status = "added"
        elif patch.get("deleted_file"):
            status = "removed"
        elif patch.get("renamed_file"):
            status = "renamed"
        patch = None if patch.get("too_large") else patch.get("diff")
    if not isinstance(patch, str) or not patch.strip():
        patch = None
    return {
        "path": path,
        "previous_path": previous,
        "status": status,
        "additions": _int(f.get("additions")),
        "deletions": _int(f.get("deletions")),
        "patch": patch,
    }


def file_chunk(f):
    """单个文件的 diff 段（带 diff --git 头）；没有 patch 返回 None。"""
    if not f.get("patch"):
        return None
    old = f.get("previous_path") or f["path"]
    a = "/dev/null" if f["status"] == "added" else f"a/{old}"
    b = "/dev/null" if f["status"] == "removed" else f"b/{f['path']}"
    body = f["patch"] if f["patch"].endswith("\n") else f["patch"] + "\n"
    return f"diff --git a/{old} b/{f['path']}\n--- {a}\n+++ {b}\n{body}"


def split_full_diff(text):
    """整段 diff → {路径: 该文件的 diff 段}（路径取 `diff --git a/x b/y` 的 y）。"""
    chunks = {}
    cur_path, cur = None, []
    for line in (text or "").splitlines(keepends=True):
        if line.startswith("diff --git "):
            if cur_path is not None:
                chunks[cur_path] = "".join(cur)
            m = re.match(r"diff --git a/(.+?) b/(.+?)\s*$", line)
            cur_path = m.group(2) if m else line[11:].strip()
            cur = [line]
        elif cur_path is not None:
            cur.append(line)
    if cur_path is not None:
        chunks[cur_path] = "".join(cur)
    return chunks


def build_diff(files, full_diff=None, max_chars=DEFAULT_MAX_DIFF_CHARS):
    """按文件顺序拼 diff：跳过不用审的文件；放不下的整文件省略。返回 (diff 文本, 是否截断, 省略的文件列表)。"""
    by_path = split_full_diff(full_diff) if full_diff else {}
    parts, total, omitted = [], 0, []
    for f in files:
        if f.get("skipped"):
            continue
        chunk = by_path.get(f["path"]) or file_chunk(f)
        if not chunk:
            continue
        if total + len(chunk) > max_chars:
            omitted.append(f["path"])
            continue
        parts.append(chunk)
        total += len(chunk)
    text = "".join(parts)
    if omitted:
        head = "# diff 太大，以下文件没有放进来，请到代码目录里用 git 看：\n" + "".join(f"#   {p}\n" for p in omitted) + "\n"
        text = head + text
    return text, bool(omitted), omitted


def _user(u):
    u = u or {}
    return u.get("name") or u.get("login") or ""


def _pinyin_keys(*names):
    """Gitee / GitHub 的名字和登录名常是拼音（He-mengfei、hemengfei319）：去掉非字母、转小写，当通讯录搜索词。"""
    keys = []
    for n in names:
        k = re.sub(r"[^a-z]", "", (n or "").lower())
        if len(k) >= 4 and k not in keys:
            keys.append(k)
    return keys


def _lark_search_user(query):
    """以机器人主人的用户身份按关键词搜飞书通讯录（只读），返回 users 列表；没装 lark-cli 返回 []。"""
    import shutil
    import subprocess

    lark = shutil.which("lark-cli")
    if not lark:
        return []
    run_js = os.path.join(os.path.dirname(lark), "node_modules", "@larksuite", "cli", "scripts", "run.js")
    cmd = ["node", run_js] if os.path.isfile(run_js) else [lark]
    p = subprocess.run([*cmd, "contact", "+search-user", "--as", "user", "--query", query],
                       capture_output=True, encoding="utf-8", timeout=15)
    if p.returncode != 0:
        return []
    return ((json.loads(p.stdout or "{}").get("data") or {}).get("users")) or []


def feishu_name(names, search=None):
    """把代码平台上的作者换成飞书里的真名：按拼音搜通讯录，**唯一命中**才用；重名、搜不到、出错都返回 None（保持原样）。
    名字本身已经是中文的不搜。"""
    if any(re.search(r"[一-鿿]", n or "") for n in names):
        return None
    search = search or _lark_search_user
    for key in _pinyin_keys(*names):
        try:
            users = search(key)
        except Exception:
            return None
        if len(users) == 1:
            return (users[0].get("localized_name") or users[0].get("name") or "").strip() or None
        if len(users) > 1:
            return None  # 重名不猜
    return None


def normalize_meta(pr, detail, files):
    d = detail or {}
    head, base = d.get("head") or {}, d.get("base") or {}
    state = d.get("state") or ""
    if d.get("merged") or d.get("merged_at"):
        state = "merged"
    adds = sum(f["additions"] for f in files)
    dels = sum(f["deletions"] for f in files)
    return {
        "platform": pr.platform,
        "owner": pr.owner,
        "repo": pr.repo,
        "number": pr.number,
        "url": d.get("html_url") or pr.url,
        "title": d.get("title") or "",
        "author": _user(d.get("user")),
        "author_login": (d.get("user") or {}).get("login") or "",
        "source_branch": head.get("ref") or "",
        "target_branch": base.get("ref") or "",
        "head_sha": head.get("sha") or "",
        "base_sha": base.get("sha") or "",
        "state": state,
        "draft": bool(d.get("draft")),
        # PR 描述：fetch_main 有 --out 时写成 body_file（给 ocr 当需求背景），不放进输出 JSON
        "body": d.get("body") or "",
        # GitHub 详情里有总数（文件列表可能分页没取全）；Gitee 没有，按文件加总
        "additions": _int(d["additions"]) if "additions" in d else adds,
        "deletions": _int(d["deletions"]) if "deletions" in d else dels,
        "changed_files": _int(d["changed_files"]) if "changed_files" in d else len(files),
    }


def fetch(pr, client, max_chars=DEFAULT_MAX_DIFF_CHARS):
    """拉取并整理：返回 (元数据 dict, diff 文本)。元数据里 files 不带 patch。"""
    detail = client.detail()
    files = [normalize_file(pr.platform, f) for f in client.files()]
    for f in files:
        reason = skip_reason(f["path"])
        if reason:
            f["skipped"] = reason
    full = client.diff()
    diff, truncated, omitted = build_diff(files, full, max_chars)
    meta = normalize_meta(pr, detail, files)
    meta["files"] = [
        {k: v for k, v in f.items() if k != "patch" and v is not None} | ({} if f.get("patch") or f.get("skipped") else {"no_patch": True})
        for f in files
    ]
    meta["diff_truncated"] = truncated
    meta["omitted_files"] = omitted
    return meta, diff


MAX_BODY_CHARS = 20_000


def body_markdown(meta, body):
    """PR 标题 + 分支 + 描述，写成 ocr `-B` 用的需求背景文件。描述太长截断；没有描述时写明。"""
    body = (body or "").strip()
    if len(body) > MAX_BODY_CHARS:
        body = body[:MAX_BODY_CHARS] + "\n\n（描述过长，后面省略）"
    return (
        f"# {meta.get('title') or '（无标题）'}\n\n"
        f"PR：{meta.get('url') or ''}（{meta.get('source_branch') or '?'} → {meta.get('target_branch') or '?'}）\n\n"
        f"{body or '（PR 没有描述）'}\n"
    )


def _api_hint(e, pr, token_env, has_token):
    if not has_token and e.status in (401, 403, 404):
        return EXIT_NO_TOKEN, (f"读不到 {pr.full} 的 PR #{pr.number}（接口返回 {e.status}）：仓库可能是私有的，"
                               f"请在系统环境变量里设置 {gi.PLATFORM_LABEL[pr.platform]} 私人令牌 {token_env} 后重试")
    return EXIT_API, {
        401: f"{token_env} 无效或已过期（接口返回 401）",
        403: f"{token_env} 没有权限读取 {pr.full}，或接口被限流（接口返回 403）",
        404: f"PR 不存在，或 {token_env} 无权访问 {pr.full}（接口返回 404）",
    }.get(e.status, str(e))


# ───────────────────────── 上一轮审查结果 ─────────────────────────

def state_root(rt):
    """存审查结果的根目录：运行时配置的 review_state_dir（客户端写入），没有就用 ~/.beetle/pr-reviews。"""
    d = (rt or {}).get("review_state_dir")
    return Path(os.path.expanduser(d)) if d else Path.home() / ".beetle" / "pr-reviews"


def state_path(rt, pr):
    return state_root(rt) / pr.platform / pr.owner.lower() / pr.repo.lower() / f"{pr.number}.json"


def load_state(rt, pr):
    """上一轮的结果 dict；没有或文件坏了返回 None。"""
    p = state_path(rt, pr)
    try:
        return json.loads(p.read_text(encoding="utf-8-sig"))
    except (OSError, ValueError):
        return None


def validate_result(data):
    if not isinstance(data, dict):
        raise PrError("审查结果必须是 JSON 对象", EXIT_USAGE)
    if not str(data.get("repo") or "").strip() or _int(data.get("pr")) <= 0:
        raise PrError("审查结果缺少 repo 或 pr", EXIT_USAGE)
    findings = data.get("findings")
    if not isinstance(findings, list):
        raise PrError("审查结果的 findings 必须是数组", EXIT_USAGE)
    for f in findings:
        if not isinstance(f, dict) or f.get("severity") not in ("high", "medium", "low") or not str(f.get("text") or "").strip():
            raise PrError("每个发现都要有 severity（high / medium / low）和 text", EXIT_USAGE)
        if f.get("status", "new") not in ("new", "carried", "resolved"):
            raise PrError("发现的 status 只能是 new / carried / resolved", EXIT_USAGE)
    validate_coverage(data.get("coverage"))


def validate_coverage(cov):
    """coverage 可缺省；有就必须是 {"total": N, "reviewed": N, "skipped": [{"path", "reason"}]}，reviewed ≤ total。"""
    if cov is None:
        return
    if not isinstance(cov, dict):
        raise PrError("coverage 必须是对象：{\"total\", \"reviewed\", \"skipped\"}", EXIT_USAGE)
    total, reviewed = cov.get("total"), cov.get("reviewed")
    if not all(isinstance(v, int) and not isinstance(v, bool) and v >= 0 for v in (total, reviewed)):
        raise PrError("coverage 的 total / reviewed 必须是非负整数", EXIT_USAGE)
    if reviewed > total:
        raise PrError("coverage 的 reviewed 不能大于 total", EXIT_USAGE)
    skipped = cov.get("skipped", [])
    if not isinstance(skipped, list) or any(not isinstance(s, dict) or not str(s.get("path") or "").strip() for s in skipped):
        raise PrError("coverage 的 skipped 必须是 [{\"path\": ..., \"reason\": ...}] 数组", EXIT_USAGE)


def save_state(rt, pr, data, now=None):
    """保存本轮结果（先写临时文件再改名），返回路径。"""
    validate_result(data)
    out = dict(data)
    out["saved_at"] = (now or datetime.now()).astimezone().isoformat()
    out["pr_url"] = pr.url
    p = state_path(rt, pr)
    p.parent.mkdir(parents=True, exist_ok=True)
    tmp = p.with_name(f"{p.name}.{os.getpid()}.tmp")
    tmp.write_text(json.dumps(out, ensure_ascii=False, indent=2), encoding="utf-8")
    os.replace(tmp, p)
    return p


# ───────────────────────── 命令行入口 ─────────────────────────

def _optional_runtime(load_runtime):
    try:
        return load_runtime() or {}
    except SystemExit:
        return {}


def fetch_main(argv=None, *, load_runtime, make_client=None, out=None, err=None, name_lookup=None):
    import argparse

    out = out or sys.stdout
    err = err or sys.stderr
    ap = argparse.ArgumentParser(description="拉取 Gitee / GitHub PR 的元数据、改动文件和 diff（只读）")
    ap.add_argument("--url", required=True, help="PR 链接")
    ap.add_argument("--out", help="diff 写到这个目录下的文件里（输出 JSON 的 diff_file）；不给就把 diff 放进输出 JSON 的 diff 字段")
    ap.add_argument("--max-diff-chars", type=int, default=DEFAULT_MAX_DIFF_CHARS, help="diff 最多多少字符，超出按文件整段省略")
    a = ap.parse_args(argv)

    token = None
    try:
        pr = parse_pr_url(a.url)
        rt = _optional_runtime(load_runtime)
        name, project = match_project(rt, pr)
        token_env = token_env_for(pr, project)
        token = gi.read_token(token_env)
        http = gi.Http(secrets=[token], user_agent=USER_AGENT)
        client = (make_client or build_client)(pr, token, http)
        try:
            meta, diff = fetch(pr, client, max(10_000, a.max_diff_chars))
        except gi.ApiError as e:
            code, msg = _api_hint(e, pr, token_env, bool(token))
            raise PrError(msg, code)
        except gi.IssueError as e:  # 网络错误
            raise PrError(str(e), EXIT_API)
        meta["project"] = name
        meta["token_env"] = token_env
        # 作者换成飞书里的真名（通讯录唯一命中才换），原名留在 author_login 旁边
        real = (name_lookup or feishu_name)([meta["author"], meta["author_login"]])
        if real:
            meta["author_platform_name"] = meta["author"]
            meta["author"] = real
        if a.out:
            d = Path(a.out)
            d.mkdir(parents=True, exist_ok=True)
            key = f"{pr.platform}-{pr.owner}-{pr.repo}-{pr.number}"
            f = d / f"{key}.diff"
            f.write_text(gi.redact(diff, [token]), encoding="utf-8")
            meta["diff_file"] = f.resolve().as_posix()
            bf = d / f"{key}.body.md"
            bf.write_text(gi.redact(body_markdown(meta, meta.pop("body")), [token]), encoding="utf-8")
            meta["body_file"] = bf.resolve().as_posix()
        else:
            meta["diff"] = diff
        text = json.dumps(meta, ensure_ascii=False, indent=2)
    except PrError as e:
        print(gi.redact(f"[失败] {e}", [token]), file=err)
        return e.code
    except Exception as e:  # 兜底：任何意外都不能把 token 打出去
        print(gi.redact(f"[失败] {type(e).__name__}: {e}", [token]), file=err)
        return EXIT_API
    print(gi.redact(text, [token]), file=out)
    return EXIT_OK


def state_main(argv=None, *, load_runtime, out=None, err=None):
    import argparse

    out = out or sys.stdout
    err = err or sys.stderr
    ap = argparse.ArgumentParser(description="读取 / 保存某个 PR 上一轮的代码审查结果")
    sub = ap.add_subparsers(dest="cmd", required=True)
    g = sub.add_parser("get", help="打印上一轮结果 JSON；没有打印「无上一轮审查记录」")
    g.add_argument("--url", required=True)
    s = sub.add_parser("save", help="保存本轮结果（机器人运行里由客户端保存，不用调）")
    s.add_argument("--url", required=True)
    s.add_argument("--file", required=True, help="本轮结果 JSON 文件")
    pth = sub.add_parser("path", help="打印结果文件的位置")
    pth.add_argument("--url", required=True)
    a = ap.parse_args(argv)
    try:
        pr = parse_pr_url(a.url)
        rt = _optional_runtime(load_runtime)
        if a.cmd == "get":
            prev = load_state(rt, pr)
            print(NO_PREVIOUS if prev is None else json.dumps(prev, ensure_ascii=False, indent=2), file=out)
        elif a.cmd == "path":
            print(state_path(rt, pr).as_posix(), file=out)
        else:
            try:
                text = Path(a.file).read_text(encoding="utf-8-sig")
            except OSError as e:
                raise PrError(f"读不了 {a.file}：{e.strerror or e}", EXIT_USAGE)
            try:
                data = json.loads(text)
            except ValueError as e:
                raise PrError(f"{a.file} 不是合法 JSON：{e}", EXIT_USAGE)
            p = save_state(rt, pr, data)
            print(f"已保存：{p.as_posix()}", file=out)
    except PrError as e:
        print(gi.redact(f"[失败] {e}"), file=err)
        return e.code
    return EXIT_OK
