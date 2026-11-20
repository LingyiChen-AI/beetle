//! 飞书接入：长连接收消息 / 回复（transport）、扫码建应用（registration）、
//! 以用户身份调 lark-cli（lark_cli）。
//!
//! 各接口由后续任务（调度、界面）接入，此前部分导出暂未被引用。
#![allow(dead_code)]

pub mod lark_cli;
pub mod registration;
pub mod transport;
