"""「代码审查」技能（技能中心独立版）公共函数：读运行时配置（与「查 bug」独立版同一份写法）。

运行时配置：先读环境变量 BEETLE_RUNTIME（旧名 BUG_PATROL_RUNTIME）指向的文件；没有该变量时读本技能目录下的
beetle.runtime.json（Beetle 客户端安装技能、点「同步配置」时写入；改名前装的是 bug-patrol.runtime.json，也认）。
字段：owner_open_id、code_root、projects、dbs（数据源，对象数组）、review_state_dir。独立版没有 chat_id、doc_url、trigger_* 字段，
群和文档由调用方用 --chat-id / --doc 参数给出。
"""
import json
import os
import shutil
import subprocess
import sys
from pathlib import Path

for _s in (sys.stdout, sys.stderr):
    try:
        _s.reconfigure(encoding="utf-8")
    except Exception:
        pass

# 机器人身份相关的环境变量：读群历史、写文档要用机器人主人本人的用户身份，
# 调用时去掉这些变量，回落到本机默认（用户身份）配置。
_BRIDGE_ENV_KEYS = ("LARK_CHANNEL", "LARK_CHANNEL_HOME", "LARK_CHANNEL_PROFILE", "LARKSUITE_CLI_CONFIG_DIR")


SKILL_DIR = Path(__file__).resolve().parents[1]


RUNTIME_ENVS = ("BEETLE_RUNTIME", "BUG_PATROL_RUNTIME")
RUNTIME_FILES = ("beetle.runtime.json", "bug-patrol.runtime.json")


def runtime_path():
    """环境变量 BEETLE_RUNTIME / 旧名 BUG_PATROL_RUNTIME 优先，否则用本技能目录下的 beetle.runtime.json（旧名 bug-patrol.runtime.json）。"""
    for k in RUNTIME_ENVS:
        env = os.environ.get(k)
        if env:
            return Path(env)
    for name in RUNTIME_FILES:
        if (SKILL_DIR / name).exists():
            return SKILL_DIR / name
    return SKILL_DIR / RUNTIME_FILES[0]


def load_runtime():
    p = runtime_path()
    if not p.exists():
        raise SystemExit(f"找不到运行时配置 {p}：请在 Beetle 客户端「技能中心」重新安装本技能，或点「同步配置」")
    try:
        return json.loads(p.read_text(encoding="utf-8-sig"))
    except json.JSONDecodeError as e:
        raise SystemExit(f"运行时配置 {p} 不是合法 JSON：{e}")


def expand(p):
    return Path(os.path.expanduser(p))


def lark_cli(args, cwd=None):
    """以用户身份调用 lark-cli，绕开 Windows 的 .cmd 壳（多行参数会被截断）。返回 (code, stdout, stderr)。"""
    lark = shutil.which("lark-cli")
    if not lark:
        raise SystemExit("找不到 lark-cli")
    run_js = os.path.join(os.path.dirname(lark), "node_modules", "@larksuite", "cli", "scripts", "run.js")
    env = {k: v for k, v in os.environ.items() if k not in _BRIDGE_ENV_KEYS}
    p = subprocess.run(["node", run_js, *args], capture_output=True, encoding="utf-8", env=env, cwd=cwd)
    return p.returncode, p.stdout or "", p.stderr or ""


def lark_json(args, cwd=None):
    code, out, err = lark_cli(args, cwd=cwd)
    if code != 0:
        raise SystemExit(f"lark-cli {' '.join(args[:2])} 失败 code={code}\n{err[:800]}\n{out[:800]}")
    return json.loads(out)
