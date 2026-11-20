"""把 reports/ 下的 bug 报告建成飞书项目工单；同名工单已存在就追加评论。

  python create_work_item.py --project <项目名> --file reports/<文件>.md
  python create_work_item.py --project <项目名> --plan      # 只看本项目的飞书项目工单配置，不建单

报告第一行 `## 标题` 作为工单名称，其余为描述。空间、类型、固定字段、处理人取运行时配置（BEETLE_RUNTIME）里项目的 meegle 段。
成功打印「已创建飞书项目工单：<链接>」或「已在飞书项目工单 <链接> 追加评论」。
退出码：0 成功；2 参数 / 配置 / 报告格式不对；3 没装或没登录飞书项目 CLI；4 接口或网络失败。逻辑在 _shared/bp_meegle.py。
"""
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "_shared"))
from bp_common import load_runtime  # noqa: E402
import bp_meegle  # noqa: E402

if __name__ == "__main__":
    sys.exit(bp_meegle.main(load_runtime=load_runtime))
