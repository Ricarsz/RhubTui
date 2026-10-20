//! RhubTui — 2.4G 键鼠电量、USB 外设面板与挂载管理。

mod app;
mod battery;
mod cli;
mod config;
mod desc;
mod hidraw;
mod keyboard;
mod mchose;
mod model;
mod mount;
mod scan;
mod theme;
mod ui;

use std::process::ExitCode;

use clap::Parser;

use cli::Cli;
use config::Config;

fn main() -> ExitCode {
    // 管道输出被截断时安静退出，而不是 panic。
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }

    let cli = Cli::parse();

    let config = match Config::load() {
        Ok(config) => config,
        Err(error) => {
            eprintln!("读取配置失败：{error:#}");
            return ExitCode::FAILURE;
        }
    };

    let result = match cli.command {
        None => app::run(config),
        Some(command) => cli::run(command, cli.json),
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("错误：{error:#}");
            ExitCode::FAILURE
        }
    }
}
