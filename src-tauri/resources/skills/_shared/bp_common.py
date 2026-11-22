"""Beetle 内置技能公共函数：读运行时配置、以用户身份调 lark-cli。

运行时配置文件路径由环境变量 BEETLE_RUNTIME 给出（旧名 BUG_PATROL_RUNTIME 也认，客户端两个都设），客户端每次运行前按群生成。
字段：chat_id、group_name、doc_url、owner_open_id、code_root、projects（本次可查的项目）、dbs（这些项目关联的数据源，对象数组）、
preferred_projects（本群常用项目）、all_projects（true = 全部项目可查，由 AI 按问题定位项目；私聊固定为 true），
以及本次触发的 trigger_message_id、trigger_sender_open_id（机器人应用的 open_id，和 owner_open_id 不可比，只作记录）、
trigger_is_owner（客户端按 union_id 判定的「触发人是机器人主人」，p2p_context 要求为 true）。
owner_open_id 是 lark-cli 应用下的 open_id，只能和 lark-cli 读回的 id 比较。
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


RUNTIME_ENVS = ("BEETLE_RUNTIME", "BUG_PATROL_RUNTIME")


def load_runtime():
    path = next((os.environ[k] for k in RUNTIME_ENVS if os.environ.get(k)), None)
    if not path:
        raise SystemExit("缺少环境变量 BEETLE_RUNTIME（旧名 BUG_PATROL_RUNTIME）：本脚本只能由 Beetle 客户端拉起的运行调用")
    p = Path(path)
    if not p.exists():
        raise SystemExit(f"找不到运行时配置 {p}，请让机器人主人在客户端里重新触发")
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
