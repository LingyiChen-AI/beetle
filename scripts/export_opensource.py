"""把当前提交导出到开源仓库目录：按 .opensource-exclude 剔除内部文件，同步增删改后在目标仓库提交。

用法：python scripts/export_opensource.py <目标仓库目录> [--message "提交说明"]

- 只导出 git 跟踪的文件（HEAD 版本，不含未提交改动），保证导出的就是已提交的代码。
- 目标目录里除 .git 外的文件以导出结果为准：多余的删除、变化的覆盖。
- 导出前对内容做敏感词检查（见 BLOCK_PATTERNS），命中就中止，不写目标目录。
"""

import argparse
import fnmatch
import re
import shutil
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
EXCLUDE_FILE = ROOT / ".opensource-exclude"
# 内部信息兜底检查：公司名、内部仓库组织、内部文档链接
BLOCK_PATTERNS = [
    re.compile(p, re.I)
    for p in [r"freecall", r"chengduzhihuige", r"RD-Department3", r"feishu\.cn/(docx|wiki|sheets)/[A-Za-z0-9]{10,}"]
]
# 依赖锁文件里的第三方包名可能碰巧命中，不检查
SKIP_CHECK = {"src-tauri/Cargo.lock", "pnpm-lock.yaml", "scripts/export_opensource.py"}


def git(*args: str, cwd: Path = ROOT, binary: bool = False):
    out = subprocess.run(["git", *args], cwd=cwd, capture_output=True, check=True)
    return out.stdout if binary else out.stdout.decode("utf-8")


def load_excludes() -> list[str]:
    lines = EXCLUDE_FILE.read_text(encoding="utf-8").splitlines()
    return [l.strip() for l in lines if l.strip() and not l.strip().startswith("#")]


def excluded(path: str, patterns: list[str]) -> bool:
    for p in patterns:
        if p.endswith("/"):
            if path.startswith(p):
                return True
        elif fnmatch.fnmatch(path, p) or path == p:
            return True
    return False


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("target")
    ap.add_argument("--message", default=None)
    a = ap.parse_args()
    target = Path(a.target).resolve()
    if target == ROOT or ROOT in target.parents:
        print("目标目录不能在本仓库里面", file=sys.stderr)
        return 2

    patterns = load_excludes()
    head = git("rev-parse", "--short", "HEAD").strip()
    files = [f for f in git("ls-files", "-z").split("\0") if f and not excluded(f, patterns)]

    contents: dict[str, bytes] = {}
    hits: list[str] = []
    for f in files:
        data = git("show", f"HEAD:{f}", binary=True)
        contents[f] = data
        if f in SKIP_CHECK:
            continue
        try:
            text = data.decode("utf-8")
        except UnicodeDecodeError:
            continue
        for n, line in enumerate(text.splitlines(), 1):
            if any(p.search(line) for p in BLOCK_PATTERNS):
                hits.append(f"{f}:{n}: {line.strip()[:120]}")
    if hits:
        print("发现疑似内部信息，已中止导出：", file=sys.stderr)
        print("\n".join(hits[:50]), file=sys.stderr)
        return 1

    target.mkdir(parents=True, exist_ok=True)
    if not (target / ".git").exists():
        git("init", "-b", "main", cwd=target)
    # 删掉目标里导出结果没有的文件
    for p in sorted(target.rglob("*"), reverse=True):
        rel = p.relative_to(target).as_posix()
        if rel == ".git" or rel.startswith(".git/"):
            continue
        if p.is_file() and rel not in contents:
            p.unlink()
        elif p.is_dir() and not any(p.iterdir()):
            p.rmdir()
    for f, data in contents.items():
        dst = target / f
        dst.parent.mkdir(parents=True, exist_ok=True)
        if not dst.exists() or dst.read_bytes() != data:
            dst.write_bytes(data)

    git("add", "-A", cwd=target)
    if not git("status", "--porcelain", cwd=target).strip():
        print(f"没有变化（源提交 {head}）")
        return 0
    msg = a.message or f"同步源仓库 {head}"
    git("commit", "-q", "-m", msg, cwd=target)
    print(f"已导出 {len(contents)} 个文件并提交：{msg}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
