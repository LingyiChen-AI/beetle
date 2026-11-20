"""把项目的 release 分支同步到代码缓存（只读分析用）。项目在运行时配置的 projects 里登记。

  python sync_code.py my-service
  python sync_code.py my-service --branch release/V3.2
  python sync_code.py my-service --pr 29 --sha 1a2b3c4    # 代码审查：检出 PR #29 的最新提交（独立 worktree）

目录结构（多个群 / 多个机器人并发运行时互不干扰）：
  <code_root>/<项目>/                 克隆（只用来 fetch，不在这里看代码）
  <code_root>/<项目>@<分支>/          每个分支一个独立的 git worktree，脚本输出的「目录」就是它
  <code_root>/<项目>@pr-<编号>/        代码审查用：每个 PR 一个独立的 git worktree，检出 PR 的 head（refs/pull/<编号>/head）
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

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "_shared"))
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


def _project(project):
    rt = load_runtime()
    projects = rt.get("projects") or {}
    if project not in projects:
        raise SystemExit(f"未登记的项目 {project}，可用：{sorted(projects)}")
    root = expand(rt["code_root"])
    root.mkdir(parents=True, exist_ok=True)
    return projects[project], root, root / project


def _ensure_clone(p, root, repo_dir):
    if not (repo_dir / ".git").exists():
        print(f"首次克隆 {p['repo']} → {repo_dir}")
        git(["clone", "--no-checkout", "--", p["repo"], str(repo_dir)], cwd=root, timeout=CLONE_TIMEOUT)
    git(["fetch", "origin", "--prune", "--quiet"], cwd=repo_dir)


def _checkout(repo_dir, work, ref):
    """把 work（独立 worktree）强制切到 ref 并清掉本地改动，返回提交信息。"""
    if work.exists() and not (work / ".git").exists():
        # 上次建 worktree 中途失败留下的半截目录（只是缓存，删掉重建）
        if sys.version_info >= (3, 12):
            shutil.rmtree(work, onexc=_force_remove)
        else:
            shutil.rmtree(work, onerror=_force_remove)
    if not (work / ".git").exists():
        git(["worktree", "prune"], cwd=repo_dir)
        git(["worktree", "add", "--force", "--detach", str(work), ref], cwd=repo_dir)
    else:
        git(["checkout", "--force", "--detach", ref], cwd=work)
    git(["clean", "-fdq"], cwd=work)
    return git(["log", "-1", "--format=%h %ci %an %s"], cwd=work)


def sync(project, branch=None):
    p, root, repo_dir = _project(project)
    with ProjectLock(root, project):
        _ensure_clone(p, root, repo_dir)
        branch = branch or latest_branch(repo_dir, p.get("branch_glob", "release/*"))
        if branch.startswith("-"):
            raise SystemExit(f"分支名不合法：{branch}")
        if not git(["rev-parse", "--verify", "--quiet", f"refs/remotes/origin/{branch}"], cwd=repo_dir, check=False):
            raise SystemExit(f"远端没有分支 {branch}")
        info = _checkout(repo_dir, root / branch_dir_name(project, branch), f"origin/{branch}")
    return root / branch_dir_name(project, branch), branch, info


def pr_dir_name(project, number):
    return f"{project}@pr-{number}"


def sync_pr(project, number, sha=None):
    """检出 PR #number 的 head（Gitee / GitHub 都提供 refs/pull/<编号>/head）到 <项目>@pr-<编号>。

    指定 sha 时核对：远端 PR 已经有更新的提交、sha 本地也有，就按 sha 检出（和 pr_fetch 拿到的 diff 对得上）；
    sha 不在本地就按 PR 最新提交检出并给出提示。返回 (目录, 显示用的分支名, 提交信息, 提示或 None)。
    """
    try:
        n = int(str(number).strip().lstrip("#"))
    except ValueError:
        raise SystemExit(f"PR 编号不合法：{number}")
    if n <= 0:
        raise SystemExit(f"PR 编号不合法：{number}")
    if sha is not None and not re.fullmatch(r"[0-9a-fA-F]{7,40}", sha):
        raise SystemExit(f"提交号不合法：{sha}")
    p, root, repo_dir = _project(project)
    ref = f"refs/remotes/origin/pr/{n}"
    note = None
    with ProjectLock(root, project):
        _ensure_clone(p, root, repo_dir)
        git(["fetch", "origin", "--quiet", f"+refs/pull/{n}/head:{ref}"], cwd=repo_dir)
        target = ref
        if sha:
            head = git(["rev-parse", ref], cwd=repo_dir)
            if not head.lower().startswith(sha.lower()):
                if git(["rev-parse", "--verify", "--quiet", f"{sha}^{{commit}}"], cwd=repo_dir, check=False):
                    target = sha
                    note = f"PR 在拉取元数据之后又有新提交（最新 {head[:10]}），按指定的 {sha[:10]} 检出"
                else:
                    note = f"指定的提交 {sha[:10]} 不在 PR 里（PR 可能已更新），按 PR 最新提交 {head[:10]} 检出"
        work = root / pr_dir_name(project, n)
        info = _checkout(repo_dir, work, target)
    return work, f"PR #{n}", info, note


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("project")
    ap.add_argument("--branch", help="指定分支，默认按 branch_glob 取版本号最大的")
    ap.add_argument("--pr", help="代码审查用：检出这个 PR 的最新提交（独立 worktree <项目>@pr-<编号>）")
    ap.add_argument("--sha", help="和 --pr 一起用：PR 的 head 提交（pr_fetch.py 输出的 head_sha），用来核对")
    a = ap.parse_args()
    note = None
    if a.pr:
        if a.branch:
            raise SystemExit("--pr 和 --branch 只能用一个")
        work, branch, info, note = sync_pr(a.project, a.pr, a.sha)
    else:
        work, branch, info = sync(a.project, a.branch)
    rules = load_runtime()["projects"][a.project].get("rules_file")
    print(f"项目: {a.project}")
    print(f"目录: {work.as_posix()}")
    print(f"分支: {branch}")
    print(f"提交: {info}")
    if note:
        print(f"提示: {note}")
    if rules:
        print(f"先读: {(work / rules).as_posix()}")


if __name__ == "__main__":
    main()
