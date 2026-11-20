#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::io::Read;

fn main() {
    // 迁移 / 运维用：beetle --import-secret <app_id>，从标准输入读 App Secret 写入系统凭据库，不启动界面。
    let args: Vec<String> = std::env::args().collect();
    // 直连数据源只读查询：beetle --db-query <别名>，查询 JSON 从标准输入读，结果 JSON 写标准输出，不启动界面。
    if args.get(1).map(String::as_str) == Some("--db-query") {
        std::process::exit(beetle_lib::dbquery::cli_main(&args[2..]));
    }
    if args.get(1).map(String::as_str) == Some("--import-secret") {
        let Some(app_id) = args.get(2) else {
            eprintln!("用法：beetle --import-secret <app_id>（密钥从标准输入读入）");
            std::process::exit(2);
        };
        let mut secret = String::new();
        if std::io::stdin().read_to_string(&mut secret).is_err() || secret.trim().is_empty() {
            eprintln!("没有从标准输入读到密钥");
            std::process::exit(1);
        }
        match beetle_lib::store::secrets::set_app_secret(app_id, secret.trim()) {
            Ok(()) => println!("已写入 {app_id} 的 App Secret"),
            Err(e) => {
                eprintln!("{e}");
                std::process::exit(1);
            }
        }
        return;
    }
    beetle_lib::run()
}
