"""把项目的 release 分支同步到代码缓存（只读分析用）。项目在运行时配置的 projects 里登记。

  python sync_code.py my-service
  python sync_code.py my-service --branch release/V3.2
  python sync_code.py --list           # 列出已登记的项目（名称、说明、关联数据源）

目录结构（多个群 / 多个机器人并发运行时互不干扰）：
  <code_root>/<项目>/                 克隆（只用来 fetch，不在这里看代码）
  <code_root>/<项目>@<分支>/          每个分支一个独立的 git worktree，脚本输出的「目录」就是它
同一项目的 clone / fetch / worktree 操作用文件锁串行（进程退出锁自动释放）；不同分支各有各的目录，
一个群切到 release/V3.2 不会把另一个群正在看的 release/V3.3 切走。每次同步都强制切到该分支最新提交、丢弃目录里的本地改动。
git 一律非交互（不弹凭据框）并带超时。
"""
import argparse
import fnmatch
import os
import re
import shutil
import stat
import subprocess
import sys
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "_shared"))
from bp_common import expand, load_runtime  # noqa: E402

GIT_TIMEOUT = 300
CLONE_TIMEOUT = 900
LOCK_TIMEOUT = 600


def git_env():
    """不弹凭据输入框 / 凭据管理器窗口，ssh 不交互（与客户端校验仓库时一致）。"""
    env = dict(os.environ)
    env["GIT_TERMINAL_PROMPT"] = "0"
    env["GCM_INTERACTIVE"] = "never"
    env.setdefault("GIT_SSH_COMMAND", "ssh -o BatchMode=yes -o ConnectTimeout=15")
    return env


def git(args, cwd, check=True, timeout=GIT_TIMEOUT):
    try:
        p = subprocess.run(["git", *args], cwd=cwd, capture_output=True, encoding="utf-8",
                           env=git_env(), stdin=subprocess.DEVNULL, timeout=timeout)
    except subprocess.TimeoutExpired:
        raise SystemExit(f"git {' '.join(args[:2])} 超过 {timeout} 秒没有完成（网络或凭据问题？），已放弃")
    if check and p.returncode != 0:
        raise SystemExit(f"git {' '.join(args)} 失败：\n{p.stderr.strip()}")
    return p.stdout.strip()


class ProjectLock:
    """<code_root>/.locks/<项目>.lock 上的进程级排他锁；进程退出（包括被杀）时操作系统自动释放。"""

    def __init__(self, root, project, timeout=LOCK_TIMEOUT):
        self.path = root / ".locks" / f"{project}.lock"
        self.timeout = timeout
        self.f = None

    def _try_lock(self):
        if os.name == "nt":
            import msvcrt
            self.f.seek(0)
            msvcrt.locking(self.f.fileno(), msvcrt.LK_NBLCK, 1)
        else:
            import fcntl
            fcntl.flock(self.f.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)

    def __enter__(self):
        self.path.parent.mkdir(parents=True, exist_ok=True)
        self.f = open(self.path, "a+")
        deadline = time.monotonic() + self.timeout
        while True:
            try:
                self._try_lock()
                return self
            except OSError:
                if time.monotonic() >= deadline:
                    self.f.close()
                    raise SystemExit(f"项目 {self.path.stem} 正在被别的运行同步，等了 {self.timeout} 秒还没结束，稍后再试")
                time.sleep(1)

    def __exit__(self, *exc):
        try:
            if os.name == "nt":
                import msvcrt
                self.f.seek(0)
                msvcrt.locking(self.f.fileno(), msvcrt.LK_UNLCK, 1)
            else:
                import fcntl
                fcntl.flock(self.f.fileno(), fcntl.LOCK_UN)
        except OSError:
            pass
        self.f.close()


def version_key(branch):
    """release/V3.3 → (3,3,0,0)；release/V3.0-20260824 → (3,0,0,20260824)。不像版本号的排最前。"""
    name = branch.rsplit("/", 1)[-1]
    m = re.match(r"[vV]?(\d+)(?:\.(\d+))?(?:\.(\d+))?(?:[-_](\d{8}))?", name)
    if not m:
        return (-1, -1, -1, -1)
    return tuple(int(x) if x else 0 for x in m.groups())


def latest_branch(repo_dir, glob):
    refs = git(["for-each-ref", "--format=%(refname:lstrip=3)", "refs/remotes/origin/"], cwd=repo_dir).split()
    refs = [r for r in refs if fnmatch.fnmatch(r, glob)]
    if not refs:
        raise SystemExit(f"远端没有匹配 {glob} 的分支")
    return max(refs, key=version_key)


def _force_remove(func, path, _exc):
    """Windows 上 git 对象是只读文件，去掉只读再删。"""
    os.chmod(path, stat.S_IWRITE)
    func(path)


def branch_dir_name(project, branch):
    return f"{project}@{re.sub(r'[^A-Za-z0-9._-]', '_', branch)}"


def sync(project, branch=None):
    rt = load_runtime()
    projects = rt.get("projects") or {}
    if project not in projects:
        raise SystemExit(f"未登记的项目 {project}，可用：{sorted(projects)}")
    p = projects[project]
    root = expand(rt["code_root"])
    root.mkdir(parents=True, exist_ok=True)
    repo_dir = root / project
    with ProjectLock(root, project):
        if not (repo_dir / ".git").exists():
            print(f"首次克隆 {p['repo']} → {repo_dir}")
            git(["clone", "--no-checkout", "--", p["repo"], str(repo_dir)], cwd=root, timeout=CLONE_TIMEOUT)
        git(["fetch", "origin", "--prune", "--quiet"], cwd=repo_dir)
        branch = branch or latest_branch(repo_dir, p.get("branch_glob", "release/*"))
        if branch.startswith("-"):
            raise SystemExit(f"分支名不合法：{branch}")
        if not git(["rev-parse", "--verify", "--quiet", f"refs/remotes/origin/{branch}"], cwd=repo_dir, check=False):
            raise SystemExit(f"远端没有分支 {branch}")
        work = root / branch_dir_name(project, branch)
        if work.exists() and not (work / ".git").exists():
            # 上次建 worktree 中途失败留下的半截目录（只是缓存，删掉重建）
            if sys.version_info >= (3, 12):
                shutil.rmtree(work, onexc=_force_remove)
            else:
                shutil.rmtree(work, onerror=_force_remove)
        if not (work / ".git").exists():
            git(["worktree", "prune"], cwd=repo_dir)
            git(["worktree", "add", "--force", "--detach", str(work), f"origin/{branch}"], cwd=repo_dir)
        else:
            git(["checkout", "--force", "--detach", f"origin/{branch}"], cwd=work)
        git(["clean", "-fdq"], cwd=work)
        info = git(["log", "-1", "--format=%h %ci %an %s"], cwd=work)
    return work, branch, info


def db_desc(d):
    """运行时 dbs 里的一项 → 一句说明。新格式是对象（alias / kind / access / skill / database / note），旧格式是别名字符串。"""
    if not isinstance(d, dict):
        return str(d)
    head = f"{d.get('alias', '?')}（{d.get('kind', '?')}"
    if d.get("access") == "direct":
        return f"{head}，直连，用 db-query 查）"
    if d.get("access") == "skill":
        extra = f"，库 {d['database']}" if d.get("database") else ""
        note = f"，{d['note']}" if d.get("note") else ""
        return f"{head}，用技能 {d.get('skill', '?')} 查{extra}{note}）"
    return f"{head}）"


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("project", nargs="?")
    ap.add_argument("--branch", help="指定分支，默认按 branch_glob 取版本号最大的")
    ap.add_argument("--list", action="store_true", help="列出已登记的项目")
    a = ap.parse_args()
    if a.list or not a.project:
        for name, p in sorted((load_runtime().get("projects") or {}).items()):
            print(f"{name}\t{p.get('description', '')}\t数据源={'；'.join(db_desc(d) for d in (p.get('dbs') or [])) or '无'}")
        if not a.list:
            raise SystemExit("缺少项目名：python sync_code.py <项目名>（上面是已登记的项目）")
        return
    work, branch, info = sync(a.project, a.branch)
    rules = load_runtime()["projects"][a.project].get("rules_file")
    print(f"项目: {a.project}")
    print(f"目录: {work.as_posix()}")
    print(f"分支: {branch}")
    print(f"提交: {info}")
    if rules:
        print(f"先读: {(work / rules).as_posix()}")


if __name__ == "__main__":
    main()
