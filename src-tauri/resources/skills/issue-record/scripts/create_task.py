"""把 reports/ 下的报告建成飞书任务（以机器人主人的用户身份）。

  python create_task.py --project <项目名> --file reports/<文件>.md

报告第一行 `## 标题` 作为任务标题，其余为描述。清单、负责人、截止天数取运行时配置（BEETLE_RUNTIME）里项目的 feishu_task 段。
成功打印「已创建飞书任务：<链接>」。退出码：0 成功；2 参数 / 报告格式不对；4 lark-cli 调用失败。逻辑在 _shared/bp_task.py。
"""
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "_shared"))
from bp_common import load_runtime  # noqa: E402
import bp_task  # noqa: E402

if __name__ == "__main__":
    sys.exit(bp_task.main(load_runtime=load_runtime))
